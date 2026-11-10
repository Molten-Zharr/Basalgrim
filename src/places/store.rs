use super::*;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
};

pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path.parent().context("Нет папки хранилища мест")?;
        fs::create_dir_all(parent)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.uid() == rustix::process::getuid().as_raw(),
            "Хранилище мест должно быть вашим обычным файлом"
        );
        let db = Connection::open(path)?;
        let current = fs::symlink_metadata(path)?;
        ensure!(
            current.dev() == meta.dev() && current.ino() == meta.ino(),
            "Хранилище мест изменилось при открытии"
        );
        db.busy_timeout(Duration::from_millis(500))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            (0..=2).contains(&version),
            "Неподдерживаемая версия хранилища мест"
        );
        let application: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        ensure!(
            if version == 0 {
                application == 0
            } else {
                application == 0x4253504c
            },
            "Посторонняя база данных мест; файл сохранён"
        );
        if version == 0 {
            let tables: i64 = db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            ensure!(tables == 0, "Посторонняя база данных мест; файл сохранён");
            db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE places(section INTEGER NOT NULL,path BLOB NOT NULL,label TEXT NOT NULL,visited INTEGER NOT NULL,PRIMARY KEY(section,path)); CREATE TABLE dismissed(path BLOB PRIMARY KEY,visited INTEGER NOT NULL); PRAGMA application_id=1112756300; PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE; CREATE TABLE IF NOT EXISTS hidden_bookmarks(path BLOB PRIMARY KEY NOT NULL); PRAGMA user_version=2; COMMIT;")?;
        }
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { db })
    }
    pub fn apply(&mut self, command: Command) -> Result<()> {
        if matches!(command, Command::Refresh | Command::Resolve { .. }) {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp();
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let insert = |section: Section, item: Item| -> Result<()> {
            valid_path(&item.path)?;
            valid_label(&item.label)?;
            tx.execute("INSERT INTO places(section,path,label,visited) VALUES(?1,?2,?3,?4) ON CONFLICT(section,path) DO UPDATE SET visited=excluded.visited", params![section.key(), item.path.as_os_str().as_encoded_bytes(), item.label, now])?;
            Ok(())
        };
        match command {
            Command::Refresh | Command::Resolve { .. } => return Ok(()),
            Command::Bookmark(path) => {
                ensure!(
                    fs::metadata(&path)?.is_dir(),
                    "В закладки можно добавить папку"
                );
                insert(Section::Bookmarks, Item::new(path.clone()))?;
                tx.execute(
                    "DELETE FROM hidden_bookmarks WHERE path=?1",
                    [path.as_os_str().as_encoded_bytes()],
                )?;
            }
            Command::Favorite { paths, add } => {
                ensure!(paths.len() <= MAX_ITEMS, "Слишком много выбранных файлов");
                for path in paths {
                    valid_path(&path)?;
                    if add {
                        fs::symlink_metadata(&path)?;
                        insert(Section::Favorites, Item::new(path))?;
                    } else {
                        tx.execute(
                            "DELETE FROM places WHERE section=1 AND path=?1",
                            [path.as_os_str().as_encoded_bytes()],
                        )?;
                    }
                }
            }
            Command::Remove { section, path } => {
                valid_path(&path)?;
                tx.execute(
                    "DELETE FROM places WHERE section=?1 AND path=?2",
                    params![section.key(), path.as_os_str().as_encoded_bytes()],
                )?;
                if section == Section::Recent {
                    tx.execute("INSERT INTO dismissed(path,visited) VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET visited=excluded.visited", params![path.as_os_str().as_encoded_bytes(), now])?;
                } else if section == Section::Bookmarks {
                    // Suppress the import too, including when it was covered by a personal label.
                    tx.execute(
                        "INSERT OR IGNORE INTO hidden_bookmarks(path) VALUES(?1)",
                        [path.as_os_str().as_encoded_bytes()],
                    )?;
                }
            }
            Command::Rename {
                section,
                path,
                label,
            } => {
                valid_label(&label)?;
                ensure!(
                    section != Section::Recent,
                    "Недавние записи не переименовываются"
                );
                ensure!(
                    tx.execute(
                        "UPDATE places SET label=?1 WHERE section=?2 AND path=?3",
                        params![label, section.key(), path.as_os_str().as_encoded_bytes()]
                    )? == 1,
                    "Запись больше не существует"
                );
            }
            Command::Recent(path) => {
                valid_path(&path)?;
                insert(Section::Recent, Item::new(path.clone()))?;
                tx.execute(
                    "DELETE FROM dismissed WHERE path=?1",
                    [path.as_os_str().as_encoded_bytes()],
                )?;
                tx.execute("DELETE FROM places WHERE section=2 AND path NOT IN (SELECT path FROM places WHERE section=2 ORDER BY visited DESC,rowid DESC LIMIT ?1)", [RECENT_LIMIT as i64])?;
            }
        }
        let count: i64 = tx.query_row("SELECT count(*) FROM places WHERE section<>2", [], |r| {
            r.get(0)
        })?;
        ensure!(
            count <= MAX_ITEMS as i64,
            "Можно сохранить до {MAX_ITEMS} закладок и избранных файлов"
        );
        let bytes: i64 = tx.query_row(
            "SELECT coalesce(sum(length(path)+length(CAST(label AS BLOB))),0) FROM places",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            bytes <= 8 * 1024 * 1024,
            "Сохранённые места превышают 8 МиБ"
        );
        tx.execute("DELETE FROM dismissed WHERE rowid NOT IN (SELECT rowid FROM dismissed ORDER BY visited DESC LIMIT 10000)", [])?;
        Self::check_hidden_bookmarks(&tx)?;
        tx.commit()?;
        Ok(())
    }
    fn check_hidden_bookmarks(db: &Connection) -> Result<()> {
        let (count, path, bytes): (i64, i64, i64) = db.query_row(
            "SELECT count(*),coalesce(max(length(path)),0),coalesce(sum(length(path)),0) FROM hidden_bookmarks",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        ensure!(
            count <= MAX_ITEMS as i64 && path <= 65536 && bytes <= 8 * 1024 * 1024,
            "Список скрытых закладок превышает ограничения"
        );
        Ok(())
    }
    pub fn snapshot(&self, config: &Config) -> Result<Snapshot> {
        let (count, path, label, bytes): (i64,i64,i64,i64) = self.db.query_row("SELECT count(*),coalesce(max(length(path)),0),coalesce(max(length(CAST(label AS BLOB))),0),coalesce(sum(length(path)+length(CAST(label AS BLOB))),0) FROM places", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        ensure!(
            count <= (MAX_ITEMS + RECENT_LIMIT) as i64
                && path <= 65536
                && label <= 1024
                && bytes <= 8 * 1024 * 1024,
            "Хранилище мест превышает ограничения"
        );
        let mut snapshot = Snapshot::default();
        let mut query = self
            .db
            .prepare("SELECT section,path,label,visited FROM places ORDER BY rowid")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let section: i64 = row.get(0)?;
            let path: Vec<u8> = row.get(1)?;
            let item = Item {
                path: std::ffi::OsString::from_vec(path).into(),
                label: row.get(2)?,
                visited: row.get(3)?,
                desktop: false,
            };
            valid_path(&item.path)?;
            valid_label(&item.label)?;
            match section {
                0 => snapshot.bookmarks.push(item),
                1 => snapshot.favorites.push(item),
                2 => snapshot.recent.push(item),
                _ => anyhow::bail!("Некорректный раздел мест"),
            }
            ensure!(
                snapshot.bookmarks.len() + snapshot.favorites.len() + snapshot.recent.len()
                    <= MAX_ITEMS + RECENT_LIMIT,
                "Слишком много сохранённых мест"
            );
        }
        let bookmarks = config
            .bookmarks
            .try_exists()
            .map_err(anyhow::Error::from)
            .and_then(|modern| {
                desktop::bookmarks(if modern {
                    &config.bookmarks
                } else {
                    &config.legacy_bookmarks
                })
            });
        let mut seen: std::collections::BTreeSet<_> =
            snapshot.bookmarks.iter().map(|i| i.path.clone()).collect();
        Self::check_hidden_bookmarks(&self.db)?;
        let mut hidden_bookmarks = BTreeSet::new();
        let mut query = self.db.prepare("SELECT path FROM hidden_bookmarks")?;
        for row in query.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            let path: PathBuf = std::ffi::OsString::from_vec(row?).into();
            valid_path(&path)?;
            hidden_bookmarks.insert(path);
        }
        match bookmarks {
            Ok(items) => {
                for item in items {
                    if !hidden_bookmarks.contains(&item.path) && seen.insert(item.path.clone()) {
                        snapshot.bookmarks.push(item);
                    }
                }
            }
            Err(e) => snapshot.warnings.push(format!("GTK-закладки: {e:#}")),
        }
        let (count, path, bytes): (i64, i64, i64) = self.db.query_row(
            "SELECT count(*),coalesce(max(length(path)),0),coalesce(sum(length(path)),0) FROM dismissed", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        ensure!(
            count <= MAX_ITEMS as i64 && path <= 65536 && bytes <= 8 * 1024 * 1024,
            "Список скрытых недавних записей превышает ограничения"
        );
        let mut hidden = BTreeMap::<Vec<u8>, i64>::new();
        let mut query = self
            .db
            .prepare("SELECT path,visited FROM dismissed LIMIT 10001")?;
        for row in query.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (p, t) = row?;
            hidden.insert(p, t);
        }
        ensure!(
            hidden.len() <= MAX_ITEMS,
            "Слишком много скрытых недавних записей"
        );
        let mut recent: BTreeMap<_, _> = snapshot
            .recent
            .drain(..)
            .map(|i| (i.path.clone(), i))
            .collect();
        match desktop::recent(&config.recent) {
            Ok(items) => {
                for item in items {
                    if hidden
                        .get(item.path.as_os_str().as_encoded_bytes())
                        .is_some_and(|t| *t >= item.visited)
                    {
                        continue;
                    }
                    if let Some(ours) = recent.get_mut(&item.path) {
                        ours.visited = ours.visited.max(item.visited);
                    } else {
                        recent.insert(item.path.clone(), item);
                    }
                }
            }
            Err(e) => snapshot.warnings.push(format!("Недавние GTK: {e:#}")),
        }
        snapshot.recent = recent.into_values().collect();
        snapshot
            .recent
            .sort_by_key(|i| std::cmp::Reverse(i.visited));
        snapshot.recent.truncate(RECENT_LIMIT);
        Ok(snapshot)
    }
}

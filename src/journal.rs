//! Durable write-ahead filesystem history. No inverse action is executed here.
mod model;
mod recorder;
use anyhow::{Context, Result, ensure};
pub use model::{Identity, Inverse, Item, Receipt, Stored, bytes, path};
use model::{Spec, Summary, now, specs};
pub use recorder::{Recorder, tracked_rename};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const APPLICATION_ID: i64 = 0x4253474a;
const SCHEMA: &str = "
PRAGMA application_id=1112753994;
PRAGMA user_version=1;
CREATE TABLE owners(id TEXT PRIMARY KEY,started_ms INTEGER NOT NULL,ended_ms INTEGER);
CREATE TABLE operations(id INTEGER PRIMARY KEY AUTOINCREMENT,owner TEXT NOT NULL REFERENCES owners(id),runtime_id INTEGER NOT NULL,title TEXT NOT NULL,destination BLOB,started_ms INTEGER NOT NULL,finished_ms INTEGER,elapsed_ms INTEGER NOT NULL DEFAULT 0,state TEXT NOT NULL,total INTEGER NOT NULL,summary BLOB);
CREATE TABLE items(operation_id INTEGER NOT NULL REFERENCES operations(id),sequence INTEGER NOT NULL,kind TEXT NOT NULL,source BLOB NOT NULL,requested BLOB,destination BLOB,before BLOB,after BLOB,state TEXT NOT NULL,inverse BLOB,issue TEXT,PRIMARY KEY(operation_id,sequence));
CREATE TABLE steps(id INTEGER PRIMARY KEY AUTOINCREMENT,operation_id INTEGER NOT NULL REFERENCES operations(id),item INTEGER NOT NULL,kind TEXT NOT NULL,source BLOB,destination BLOB,before_source BLOB,before_destination BLOB,after_destination BLOB,state TEXT NOT NULL,issue TEXT);
CREATE INDEX operations_recent ON operations(id DESC);
CREATE INDEX steps_item ON steps(operation_id,item,id);
";

pub struct Database {
    connection: Connection,
    owner: String,
    _lock: tempfile::NamedTempFile,
}
impl Database {
    pub fn path() -> Option<PathBuf> {
        dirs::state_dir().map(|p| p.join("molten-zharr/basalgrim/operations.sqlite3"))
    }
    pub fn open(path: &Path) -> Result<Self> {
        let parent = path.parent().context("Нет папки журнала")?;
        fs::create_dir_all(parent)?;
        if !path.try_exists()? {
            let temp = tempfile::NamedTempFile::new_in(parent)?;
            let mut initial = Connection::open(temp.path())?;
            initial.pragma_update(None, "synchronous", "FULL")?;
            {
                let tx = initial.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute_batch(SCHEMA)?;
                tx.commit()?;
            }
            initial.close().map_err(|(_, e)| e)?;
            temp.as_file().sync_all()?;
            match crate::fs::rename_new(temp.path(), path) {
                Ok(()) => {
                    File::open(parent)?.sync_all()?;
                }
                Err(e)
                    if e.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::EXIST) => {
                }
                Err(e) => return Err(e),
            }
        }
        let fd = rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let mut file = File::from(fd);
        ensure!(
            file.metadata()?.is_file(),
            "Журнал должен быть обычным файлом"
        );
        let mut header = [0u8; 16];
        file.read_exact(&mut header)?;
        ensure!(
            &header == b"SQLite format 3\0",
            "Файл журнала не является SQLite"
        );
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let preflight = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        preflight.busy_timeout(Duration::from_secs(2))?;
        ensure!(
            preflight.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))?
                == APPLICATION_ID,
            "Неизвестный формат журнала"
        );
        ensure!(
            preflight.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? == 1,
            "Версия журнала не поддерживается"
        );
        ensure!(
            preflight.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0))? == "ok",
            "Журнал повреждён"
        );
        drop(preflight);
        let mut connection = Connection::open_with_flags(path, flags)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        ensure!(
            connection.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))?
                == APPLICATION_ID
                && connection.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? == 1,
            "Формат журнала изменился во время открытия"
        );
        connection.prepare("SELECT operation_id,sequence,source,destination,before,after,inverse,issue FROM items LIMIT 0")?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        ensure!(mode == "wal", "Не удалось включить WAL журнала");
        connection.pragma_update(None, "synchronous", "FULL")?;
        let locks = parent.join("journal-locks");
        fs::create_dir_all(&locks)?;
        let lock = tempfile::Builder::new()
            .prefix("owner-")
            .tempfile_in(&locks)?;
        rustix::fs::flock(
            lock.as_file(),
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )?;
        let owner = lock
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        recover(&mut connection, &locks)?;
        connection.execute(
            "INSERT INTO owners(id,started_ms) VALUES (?1,?2)",
            params![owner, now()],
        )?;
        Ok(Self {
            connection,
            owner,
            _lock: lock,
        })
    }
    pub fn begin(&mut self, operation: &crate::fs::Operation, token: u64) -> Result<Recorder<'_>> {
        let requested = specs(operation);
        ensure!(
            requested.iter().all(
                |s| s.source.is_absolute() && s.target.as_ref().is_none_or(|p| p.is_absolute())
            ),
            "Для журнала требуются абсолютные пути"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO operations(owner,runtime_id,title,destination,started_ms,state,total) VALUES (?1,?2,?3,?4,?5,'running',?6)",params![self.owner,token as i64,operation.title(),operation.destination().as_deref().map(bytes),now(),operation.total() as i64])?;
        let id = tx.last_insert_rowid();
        for (index, spec) in requested.iter().enumerate() {
            tx.execute("INSERT INTO items(operation_id,sequence,kind,source,requested,state) VALUES (?1,?2,?3,?4,?5,'pending')",params![id,index as i64,spec.kind,bytes(&spec.source),spec.target.as_deref().map(bytes)])?;
        }
        tx.commit()?;
        Ok(Recorder::new(self, id, requested))
    }
    pub fn history(&mut self) -> Result<Vec<Stored>> {
        recover(&mut self.connection, self._lock.path().parent().unwrap())?;
        let mut stmt=self.connection.prepare("SELECT id,title,destination,started_ms,elapsed_ms,state,total,summary FROM operations ORDER BY id DESC LIMIT 20")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<Vec<u8>>>(7)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (id, title, destination, started_ms, elapsed_ms, state, total, data) = row?;
            let elapsed_ms = u64::try_from(elapsed_ms).context("Неверная длительность журнала")?;
            let total = usize::try_from(total).context("Неверное число элементов журнала")?;
            let summary = data
                .as_deref()
                .map(|d| {
                    ensure!(d.len() <= 4 * 1024 * 1024, "Запись журнала слишком большая");
                    Ok::<_, anyhow::Error>(serde_json::from_slice::<Summary>(d)?)
                })
                .transpose()?;
            result.push(Stored {
                id,
                title,
                destination,
                started_ms,
                elapsed_ms,
                state,
                total,
                summary,
                items: self.items(id)?,
                completed: self.connection.query_row(
                    "SELECT COUNT(*) FROM items WHERE operation_id=?1 AND state='done'",
                    [id],
                    |r| r.get::<_, u32>(0),
                )? as usize,
                skipped: self.connection.query_row(
                    "SELECT COUNT(*) FROM items WHERE operation_id=?1 AND state='skipped'",
                    [id],
                    |r| r.get::<_, u32>(0),
                )? as usize,
            });
        }
        Ok(result)
    }
    pub fn items(&self, id: i64) -> Result<Vec<Item>> {
        let mut stmt=self.connection.prepare("SELECT sequence,kind,source,COALESCE(destination,requested),before,after,state,inverse,issue FROM items WHERE operation_id=?1 ORDER BY sequence LIMIT 200")?;
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?,
                r.get::<_, Option<Vec<u8>>>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<Vec<u8>>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (index, kind, source, target, before, after, state, inverse, issue) = row?;
            let recovery_paths = if matches!(state.as_str(), "uncertain" | "partial" | "failed") {
                let mut stmt=self.connection.prepare("SELECT destination FROM steps WHERE operation_id=?1 AND item=?2 AND kind IN ('claim_source','replace_claim','staging') ORDER BY id DESC LIMIT 20")?;
                stmt.query_map(params![id, index], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            } else {
                Vec::new()
            };
            result.push(Item {
                index: usize::try_from(index).context("Неверный индекс журнала")?,
                kind,
                source,
                target,
                before: before
                    .map(|v| serde_json::from_slice::<Option<Identity>>(&v))
                    .transpose()?
                    .flatten(),
                after: after
                    .map(|v| serde_json::from_slice::<Option<Identity>>(&v))
                    .transpose()?
                    .flatten(),
                state,
                inverse: inverse.map(|v| serde_json::from_slice(&v)).transpose()?,
                issue,
                recovery_paths,
            });
        }
        Ok(result)
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.connection.execute(
            "UPDATE owners SET ended_ms=?2 WHERE id=?1",
            params![self.owner, now()],
        );
    }
}

fn recover(connection: &mut Connection, locks: &Path) -> Result<()> {
    let old: Vec<String> = {
        let mut stmt = connection.prepare("SELECT id FROM owners WHERE ended_ms IS NULL OR EXISTS (SELECT 1 FROM operations WHERE owner=owners.id AND state='running')")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for id in old {
        ensure!(
            id.starts_with("owner-") && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'),
            "Неверный владелец журнала"
        );
        let candidate = locks.join(&id);
        let alive = match rustix::fs::open(
            &candidate,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => {
                match rustix::fs::flock(&fd, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => false,
                    Err(rustix::io::Errno::WOULDBLOCK) => true,
                    Err(e) => return Err(e.into()),
                }
            }
            Err(rustix::io::Errno::NOENT) => false,
            Err(e) => return Err(e.into()),
        };
        if !alive {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE steps SET state='uncertain' WHERE state='planned' AND operation_id IN (SELECT id FROM operations WHERE owner=?1 AND state='running')",[&id])?;
            tx.execute("UPDATE items SET state='uncertain',issue='Работа прервана; результат требует проверки' WHERE state='running' AND operation_id IN (SELECT id FROM operations WHERE owner=?1 AND state='running')",[&id])?;
            tx.execute("UPDATE operations SET state='interrupted',finished_ms=?2 WHERE owner=?1 AND state='running'",params![id,now()])?;
            tx.execute(
                "UPDATE owners SET ended_ms=?2 WHERE id=?1",
                params![id, now()],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

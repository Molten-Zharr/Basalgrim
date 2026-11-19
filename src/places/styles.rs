use super::Store;
use crate::folder_style::{Change, Decoration, FolderId, FolderStyle, import_icon};
use crate::preview::ImagePreview;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{collections::BTreeMap, path::Path, sync::Arc};

pub(super) fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch("BEGIN IMMEDIATE;
        CREATE TABLE IF NOT EXISTS folder_styles(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            folder BLOB NOT NULL UNIQUE CHECK(length(folder) BETWEEN 17 AND 65553),
            style TEXT NOT NULL CHECK(length(CAST(style AS BLOB)) <= 32768),
            width INTEGER NOT NULL, height INTEGER NOT NULL, pixels BLOB NOT NULL,
            CHECK(width BETWEEN 0 AND 64 AND height BETWEEN 0 AND 64 AND length(pixels)=width*height*4)
        );
        PRAGMA user_version=3; COMMIT;")?;
    Ok(())
}

fn check_size(db: &Connection) -> Result<()> {
    let (rows, bytes, largest): (i64,i64,i64) = db.query_row(
        "SELECT count(*), coalesce(sum(length(folder)+length(CAST(style AS BLOB))+length(pixels)),0),
         coalesce(max(length(pixels)),0) FROM folder_styles", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    ensure!(
        rows <= super::MAX_ITEMS as i64 && bytes <= 32 * 1024 * 1024 && largest <= 64 * 64 * 4,
        "Хранилище оформления папок заполнено"
    );
    Ok(())
}
impl Store {
    pub(super) fn apply_style(&mut self, change: Change) -> Result<()> {
        super::valid_path(&change.path)?;
        change.style.validate()?;
        let current: Option<(i64, String)> = self
            .db
            .query_row(
                "SELECT id,style FROM folder_styles WHERE folder=?1",
                [&change.folder.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        ensure!(
            current.as_ref().map(|p| p.0) == change.expected,
            "Оформление изменено в другом окне. Откройте свойства снова"
        );
        let old = current
            .map(|(_, s)| serde_json::from_str::<FolderStyle>(&s))
            .transpose()?;
        let retain =
            change.style.image_icon() && old.as_ref().is_some_and(|s| s.icon == change.style.icon);
        let image = if change.style.image_icon() && !retain {
            Some(import_icon(Path::new(&change.style.icon))?)
        } else {
            None
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = tx
            .query_row(
                "SELECT id FROM folder_styles WHERE folder=?1",
                [&change.folder.0],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        ensure!(
            revision == change.expected,
            "Оформление изменено в другом окне. Откройте свойства снова"
        );
        let metadata =
            std::fs::symlink_metadata(&change.path).context("Папка больше недоступна")?;
        ensure!(
            FolderId::capture(&change.path, &metadata).as_ref() == Some(&change.folder),
            "Папка была заменена"
        );
        let style = serde_json::to_string(&change.style)?;
        if change.style == FolderStyle::default() {
            tx.execute(
                "DELETE FROM folder_styles WHERE folder=?1",
                [&change.folder.0],
            )?;
        } else if retain {
            tx.execute(
                "INSERT OR REPLACE INTO folder_styles(folder,style,width,height,pixels)
                SELECT folder,?2,width,height,pixels FROM folder_styles WHERE folder=?1",
                params![change.folder.0, style],
            )?;
        } else {
            let (w, h, pixels) = image.as_ref().map_or((0, 0, &[][..]), |i| {
                (i.width, i.height, i.pixels.as_slice())
            });
            tx.execute("INSERT OR REPLACE INTO folder_styles(folder,style,width,height,pixels) VALUES(?1,?2,?3,?4,?5)",params![change.folder.0,style,w,h,pixels])?;
        }
        check_size(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn styles(&self) -> Result<BTreeMap<FolderId, Decoration>> {
        check_size(&self.db)?;
        let mut query = self
            .db
            .prepare("SELECT id,folder,style,width,height FROM folder_styles")?;
        let mut rows = query.query([])?;
        let mut next = BTreeMap::new();
        let mut cache = self.style_images.borrow_mut();
        let mut live = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let revision: i64 = row.get(0)?;
            let id = FolderId(row.get(1)?);
            let style: FolderStyle = serde_json::from_str(&row.get::<_, String>(2)?)?;
            style.validate()?;
            let image = if style.image_icon() {
                let image = if let Some(frame) = cache.get(&revision) {
                    Arc::clone(frame)
                } else {
                    let pixels = self.db.query_row(
                        "SELECT pixels FROM folder_styles WHERE id=?1 AND length(pixels)<=16384",
                        [revision],
                        |r| r.get(0),
                    )?;
                    Arc::new(ImagePreview::new(row.get(3)?, row.get(4)?, pixels)?)
                };
                live.insert(revision, image.clone());
                Some(image)
            } else {
                None
            };
            next.insert(
                id,
                Decoration {
                    revision,
                    style,
                    image,
                },
            );
        }
        *cache = live;
        Ok(next)
    }
}

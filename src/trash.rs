//! Bounded freedesktop trash catalog. Reading never creates or removes anything.
mod deletion;
use crate::{
    journal::{Identity, bytes, path},
    operations::Cancellation,
};
use anyhow::{Context, Result, ensure};
pub use deletion::Deletion;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const MAX_ITEMS: usize = 20_000;
const NAME_BUDGET: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub directory: PathBuf,
    pub base: PathBuf,
    shared: bool,
}
impl Root {
    pub fn home(data: PathBuf) -> Self {
        Self {
            directory: data.join("Trash"),
            base: data,
            shared: false,
        }
    }
    pub fn volume(base: PathBuf, shared: bool) -> Self {
        let uid = rustix::process::getuid().as_raw();
        let directory = if shared {
            base.join(format!(".Trash/{uid}"))
        } else {
            base.join(format!(".Trash-{uid}"))
        };
        Self {
            directory,
            base,
            shared,
        }
    }
    fn guard(&self) -> Result<Vec<(PathBuf, Identity)>> {
        ensure!(
            self.directory.is_absolute() && self.base.is_absolute(),
            "Адрес корзины должен быть абсолютным"
        );
        if self.shared {
            let shared = fs::symlink_metadata(self.base.join(".Trash"))?;
            ensure!(
                shared.is_dir() && shared.mode() & 0o1000 != 0,
                "Общая корзина не защищена sticky bit или является ссылкой"
            );
        }
        let mut result = Vec::new();
        for dir in [
            self.directory.clone(),
            self.directory.join("files"),
            self.directory.join("info"),
        ] {
            let id = Identity::read(&dir)?.context("Папка корзины отсутствует")?;
            ensure!(
                id.directory()
                    && id.uid == rustix::process::getuid().as_raw()
                    && id.mode & 0o022 == 0,
                "Небезопасные права, владелец или ссылка папки корзины: {}",
                dir.display()
            );
            result.push((dir, id));
        }
        Ok(result)
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub root: Root,
    pub data: PathBuf,
    pub info: PathBuf,
    pub original: Option<PathBuf>,
    pub name: String,
    pub deleted: String,
    pub size: u64,
    pub directory: bool,
    pub symlink: bool,
    pub issue: Option<String>,
    identity: Identity,
    metadata: Option<(Identity, Vec<u8>)>,
    guards: Vec<(PathBuf, Identity)>,
}
impl Entry {
    pub(crate) fn validate_claimed(
        &self,
        parked: &Path,
        cancellation: &Cancellation,
    ) -> Result<()> {
        crate::fs::operation_checkpoint(cancellation)?;
        let guards = self.root.guard()?;
        ensure!(
            guards
                .iter()
                .zip(&self.guards)
                .all(|((p, a), (q, b))| p == q
                    && a.device == b.device
                    && a.inode == b.inode
                    && a.mode == b.mode
                    && a.uid == b.uid),
            "Папки корзины изменились"
        );
        ensure!(
            Identity::read(&self.data)?.as_ref() == Some(&self.identity),
            "Элемент корзины изменился перед восстановлением"
        );
        let (expected, raw) = self.metadata.as_ref().context("Нет метаданных корзины")?;
        let actual = Identity::read(parked)?.context("Метаданные исчезли")?;
        let mut expected = expected.clone();
        expected.changed = actual.changed;
        ensure!(
            expected == actual
                && crate::journal::trash::regular(parked)? == *raw
                && Identity::read(parked)?.as_ref() == Some(&actual),
            "Метаданные изменились перед восстановлением"
        );
        ensure!(
            Identity::read(&self.info)?.is_none(),
            "В корзине появились новые метаданные с тем же именем"
        );
        Ok(())
    }
    pub fn validate(&self, cancellation: &Cancellation) -> Result<()> {
        crate::fs::operation_checkpoint(cancellation)?;
        ensure!(
            self.issue.is_none(),
            "{}",
            self.issue.as_deref().unwrap_or_default()
        );
        let guards = self.root.guard()?;
        ensure!(
            guards.len() == self.guards.len()
                && guards
                    .iter()
                    .zip(&self.guards)
                    .all(|((p, a), (q, b))| p == q
                        && a.device == b.device
                        && a.inode == b.inode
                        && a.mode == b.mode
                        && a.uid == b.uid),
            "Папки корзины изменились; обновите список"
        );
        ensure!(
            Identity::read(&self.data)?.as_ref() == Some(&self.identity),
            "Элемент корзины изменился; обновите список"
        );
        let (id, raw) = self.metadata.as_ref().context("Нет метаданных корзины")?;
        ensure!(
            Identity::read(&self.info)?.as_ref() == Some(id)
                && crate::journal::trash::regular(&self.info)? == *raw
                && Identity::read(&self.info)?.as_ref() == Some(id),
            "Метаданные корзины изменились; обновите список"
        );
        ensure!(
            crate::journal::trash::original(raw, &self.root.base)?.as_path()
                == self.original.as_deref().context("Нет исходного пути")?,
            "Исходный путь изменился"
        );
        Ok(())
    }
    pub fn target(&self, destination: Option<&Path>) -> Result<PathBuf> {
        let original = self.original.as_deref().context("Нет исходного пути")?;
        let target = match destination {
            Some(folder) => {
                folder.join(original.file_name().context("Нельзя восстановить корень")?)
            }
            None => original.into(),
        };
        ensure!(
            target.is_absolute() && target.file_name().is_some(),
            "Неверный адрес восстановления"
        );
        ensure!(
            !target.starts_with(&self.root.directory),
            "Нельзя восстановить файл внутрь корзины"
        );
        ensure!(
            target.parent().context("Нет родительской папки")?.is_dir(),
            "Исходная папка недоступна; выберите восстановление в текущую папку"
        );
        Ok(target)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub deletions: Vec<Deletion>,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}
impl Catalog {
    fn warn(&mut self, text: String) {
        if self.warnings.len() < 32 {
            self.warnings.push(text);
        }
    }
    fn finish(mut self) -> Self {
        self.entries.sort_by_cached_key(|e| {
            (
                std::cmp::Reverse(e.deleted.clone()),
                e.name.to_lowercase(),
                e.data.clone(),
            )
        });
        self
    }
}
pub fn discover(cancellation: &Cancellation) -> Result<Vec<Root>> {
    let mut roots = Vec::new();
    if let Some(data) = dirs::data_dir() {
        roots.push(Root::home(data));
    }
    let mut data = Vec::new();
    fs::File::open("/proc/self/mountinfo")?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() <= 4 * 1024 * 1024,
        "Слишком много точек монтирования"
    );
    let mut mounts = BTreeSet::new();
    for line in data.split(|b| *b == b'\n') {
        crate::fs::operation_checkpoint(cancellation)?;
        let Some(raw) = line.split(|b| *b == b' ').nth(4) else {
            continue;
        };
        let mount = mount_path(raw)?;
        if mounts.insert(mount.clone()) {
            ensure!(mounts.len() <= 4096, "Слишком много точек монтирования");
            roots.push(Root::volume(mount.clone(), true));
            roots.push(Root::volume(mount, false));
        }
    }
    Ok(roots)
}
fn mount_path(raw: &[u8]) -> Result<PathBuf> {
    let mut output = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\\' {
            let oct = raw
                .get(i + 1..i + 4)
                .context("Неверная точка монтирования")?;
            output.push(u8::from_str_radix(std::str::from_utf8(oct)?, 8)?);
            i += 4;
        } else {
            output.push(raw[i]);
            i += 1;
        }
    }
    ensure!(!output.contains(&0), "Нулевой байт точки монтирования");
    Ok(path(&output))
}
pub fn read(roots: &[Root], cancellation: &Cancellation) -> Result<Catalog> {
    let mut result = Catalog {
        deletions: deletion::read(roots, cancellation)?,
        ..Default::default()
    };
    let mut seen = BTreeSet::new();
    let mut budget = NAME_BUDGET;
    for root in roots {
        crate::fs::operation_checkpoint(cancellation)?;
        if !seen.insert(root.directory.clone()) {
            continue;
        }
        match fs::symlink_metadata(&root.directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                result.warn(format!("{}: {e}", root.directory.display()));
                continue;
            }
            Ok(_) => {}
        }
        let guards = match root.guard() {
            Ok(g) => g,
            Err(e) => {
                result.warn(format!("{e:#}"));
                continue;
            }
        };
        let entries = match fs::read_dir(root.directory.join("files")) {
            Ok(e) => e,
            Err(e) => {
                result.warn(e.to_string());
                continue;
            }
        };
        for item in entries {
            crate::fs::operation_checkpoint(cancellation)?;
            if result.entries.len() >= MAX_ITEMS {
                result.warn("Показаны первые 20000 элементов корзины".into());
                return Ok(result.finish());
            }
            let item = match item {
                Ok(e) => e,
                Err(e) => {
                    result.warn(e.to_string());
                    continue;
                }
            };
            let data = item.path();
            let Some(identity) = Identity::read(&data)? else {
                continue;
            };
            let mut info_name = item.file_name();
            info_name.push(".trashinfo");
            let info = root.directory.join("info").join(info_name);
            let mut entry = Entry {
                root: root.clone(),
                data,
                info,
                original: None,
                name: crate::fs::clean(&item.file_name().to_string_lossy()),
                deleted: String::new(),
                size: identity.size,
                directory: identity.directory(),
                symlink: identity.link.is_some(),
                issue: None,
                identity,
                metadata: None,
                guards: guards.clone(),
            };
            let parsed = (|| -> Result<()> {
                let id = Identity::read(&entry.info)?
                    .context("Нет .trashinfo; исходный путь неизвестен")?;
                let raw = crate::journal::trash::regular(&entry.info)?;
                ensure!(
                    Identity::read(&entry.info)?.as_ref() == Some(&id),
                    "Метаданные изменились при чтении"
                );
                let original = crate::journal::trash::original(&raw, &root.base)?;
                ensure!(
                    original.file_name().is_some()
                        && original.is_absolute()
                        && !original.starts_with(&root.directory),
                    "Небезопасный исходный путь"
                );
                entry.name = crate::fs::clean(&original.file_name().unwrap().to_string_lossy());
                entry.original = Some(original);
                let date = raw
                    .split(|b| *b == b'\n')
                    .find_map(|line| line.strip_prefix(b"DeletionDate="))
                    .context("Нет даты удаления")?;
                let date = std::str::from_utf8(date)?.trim_end_matches('\r');
                chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S")
                    .context("Неверная дата удаления")?;
                entry.deleted = date.replace('T', " ");
                entry.metadata = Some((id, raw));
                Ok(())
            })();
            if let Err(e) = parsed {
                entry.issue = Some(format!("{e:#}"));
            }
            let used = bytes(&entry.data).len()
                + bytes(&entry.info).len()
                + entry.original.as_ref().map_or(0, |p| bytes(p).len())
                + entry.metadata.as_ref().map_or(0, |(_, raw)| raw.len());
            let Some(left) = budget.checked_sub(used) else {
                result.warn("Достигнут предел размера списка корзины".into());
                return Ok(result.finish());
            };
            budget = left;
            result.entries.push(entry);
        }
    }
    Ok(result.finish())
}

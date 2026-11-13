//! Locate only the exact object sent through gio, using freedesktop trash metadata.
use super::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, os::unix::ffi::OsStrExt};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Locator {
    pub data: Vec<u8>,
    pub info: Vec<u8>,
    pub metadata: snapshot::Snapshot,
}
pub struct Search {
    roots: Vec<(PathBuf, PathBuf)>,
    existing: BTreeSet<PathBuf>,
    readable: BTreeSet<PathBuf>,
}
pub(crate) fn regular(path: &Path) -> Result<Vec<u8>> {
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    ensure!(
        file.metadata()?.is_file(),
        "Метаданные корзины не являются файлом"
    );
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65_536, "Метаданные корзины слишком большие");
    Ok(bytes)
}
pub(crate) fn original(info: &[u8], base: &Path) -> Result<PathBuf> {
    let mut lines = info.split(|&b| b == b'\n');
    ensure!(
        lines
            .next()
            .is_some_and(|line| line.strip_suffix(b"\r").unwrap_or(line) == b"[Trash Info]"),
        "Неверные метаданные корзины"
    );
    let value = lines
        .find_map(|line| {
            line.strip_suffix(b"\r")
                .unwrap_or(line)
                .strip_prefix(b"Path=")
        })
        .context("Нет исходного пути корзины")?;
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < value.len() {
        if value[index] == b'%' {
            let hex = value
                .get(index + 1..index + 3)
                .context("Неверный путь корзины")?;
            decoded.push(u8::from_str_radix(std::str::from_utf8(hex)?, 16)?);
            index += 3;
        } else {
            decoded.push(value[index]);
            index += 1;
        }
    }
    ensure!(!decoded.contains(&0), "Нулевой байт в пути корзины");
    ensure!(!decoded.is_empty(), "Пустой путь корзины");
    let path = path(&decoded);
    ensure!(
        !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "Небезопасный путь корзины"
    );
    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}
impl Search {
    pub fn new(source: &Path) -> Result<Self> {
        Self::for_user(source, rustix::process::getuid().as_raw(), dirs::data_dir())
    }
    pub(crate) fn for_user(source: &Path, uid: u32, data: Option<PathBuf>) -> Result<Self> {
        let mut roots = Vec::new();
        if let Some(data) = data {
            roots.push((data.join("Trash"), data));
        }
        let mut top = source
            .parent()
            .context("Нельзя отправить корень в корзину")?
            .canonicalize()?;
        let mount = snapshot::mount(&top)?;
        while let Some(parent) = top.parent() {
            if snapshot::mount(parent)? != mount {
                break;
            }
            top = parent.into();
        }
        roots.push((top.join(format!(".Trash/{uid}")), top.clone()));
        roots.push((top.join(format!(".Trash-{uid}")), top));
        let mut result = Self {
            roots,
            existing: BTreeSet::new(),
            readable: BTreeSet::new(),
        };
        for (root, _) in &result.roots {
            match fs::read_dir(root.join("info")) {
                Ok(entries) => {
                    result.readable.insert(root.clone());
                    for entry in entries.take(100_001) {
                        ensure!(
                            result.existing.len() < 100_000,
                            "Корзина слишком велика для точного определения элемента"
                        );
                        result.existing.insert(entry?.path());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("Не удалось проверить корзину {}", root.display())
                    });
                }
            }
        }
        Ok(result)
    }
    pub fn locate(
        &self,
        source: &Path,
        expected: &snapshot::Snapshot,
        cancellation: &crate::operations::Cancellation,
    ) -> Result<(Locator, snapshot::Snapshot)> {
        let mut found = None;
        let canonical = source
            .parent()
            .context("Нет исходного родителя")?
            .canonicalize()?
            .join(source.file_name().context("Нет исходного имени")?);
        for (root, base) in &self.roots {
            let entries = match fs::read_dir(root.join("info")) {
                Ok(entries) => entries,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && !self.readable.contains(root) =>
                {
                    continue;
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Адрес корзины изменился: {}", root.display()));
                }
            };
            for (count, entry) in entries.enumerate() {
                crate::fs::operation_checkpoint(cancellation)?;
                ensure!(count < 100_000, "Корзина слишком велика");
                let entry = entry?;
                if self.existing.contains(&entry.path()) {
                    continue;
                }
                let name = entry.file_name();
                let Some(name) = name.as_bytes().strip_suffix(b".trashinfo") else {
                    continue;
                };
                let Ok(info) = regular(&entry.path()) else {
                    continue;
                };
                if original(&info, base).ok().as_ref() != Some(&canonical) {
                    continue;
                }
                let data = root.join("files").join(path(name));
                let Ok(actual) = snapshot::Snapshot::read(&data, cancellation) else {
                    continue;
                };
                if !expected.relocated(&actual, false) {
                    continue;
                }
                ensure!(
                    found.is_none(),
                    "Неоднозначный результат корзины; автоматическое восстановление заблокировано"
                );
                found = Some((
                    Locator {
                        data: bytes(&data),
                        info: bytes(&entry.path()),
                        metadata: snapshot::Snapshot::read(&entry.path(), cancellation)?,
                    },
                    actual,
                ));
            }
        }
        found.context(
            "Точный элемент корзины не найден; восстановите его системным файловым менеджером",
        )
    }
}

mod operations;
pub(crate) use operations::RetainedRemoval;
pub(crate) use operations::operate_with_trash;
pub(crate) use operations::operation_cancelled;
pub(crate) use operations::operation_checkpoint;
pub(crate) use operations::relocate_recorded;
pub use operations::{
    Operation, Report, RetainedCopy, operate, operate_with_conflicts, operate_with_control,
    operate_with_journal, rename_new, transfer_one, trash_one, validate_name,
};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Local};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub hidden: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub details: crate::columns::Details,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Size,
    Modified,
    Field(crate::columns::Column, bool),
}
impl Sort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Modified,
            Self::Modified | Self::Field(..) => Self::Name,
        }
    }
    pub fn field(self) -> (crate::columns::Column, bool) {
        use crate::columns::Column;
        match self {
            Self::Name => (Column::Name, false),
            Self::Size => (Column::Size, true),
            Self::Modified => (Column::Modified, true),
            Self::Field(c, d) => (c, d),
        }
    }
    pub fn label(self) -> String {
        let (column, descending) = self.field();
        format!("{} {}", column.label(), if descending { "↓" } else { "↑" })
    }
    pub fn sort(self, entries: &mut [Entry]) {
        let (column, descending) = self.field();
        crate::columns::sort(entries, column, descending);
    }
}

/// Escape controls from untrusted filenames and file contents before terminal rendering.
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

pub fn read_directory(path: &Path) -> Result<Vec<Entry>> {
    Ok(read_directory_until(path, || false)?.unwrap())
}

pub(crate) fn read_directory_until(
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Vec<Entry>>> {
    let mut entries = Vec::new();
    if cancelled() {
        return Ok(None);
    }
    let owners = local_owners();
    for entry in
        fs::read_dir(path).with_context(|| format!("Не удалось прочитать {}", path.display()))?
    {
        if cancelled() {
            return Ok(None);
        }
        let entry = entry?;
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // An entry may disappear while another program changes the directory.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let symlink = metadata.is_symlink();
        let directory = metadata.is_dir() || (symlink && path.is_dir());
        let raw = entry.file_name();
        let name = clean(&raw.to_string_lossy());
        let folder = crate::folder_style::FolderId::capture(&path, &metadata);
        entries.push(Entry {
            path,
            hidden: raw.as_encoded_bytes().starts_with(b"."),
            name,
            directory,
            symlink,
            size: metadata.len(),
            modified: metadata.modified().ok(),
            details: {
                use std::os::unix::fs::MetadataExt;
                crate::columns::Details {
                    folder,
                    created: metadata.created().ok(),
                    accessed: metadata.accessed().ok(),
                    mode: Some(metadata.mode()),
                    owner: Some(
                        owners
                            .get(&metadata.uid())
                            .cloned()
                            .unwrap_or_else(|| metadata.uid().to_string()),
                    ),
                }
            },
        });
    }
    Ok((!cancelled()).then_some(entries))
}

/// Metadata for a search result, read exclusively by its worker.
pub(crate) fn search_entry(
    path: PathBuf,
    source: &Path,
    owners: &std::collections::BTreeMap<u32, String>,
) -> std::io::Result<Entry> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(source)?;
    let symlink = metadata.is_symlink();
    let raw = path.file_name().unwrap_or_default();
    Ok(Entry {
        name: clean(&raw.to_string_lossy()),
        hidden: raw.as_encoded_bytes().starts_with(b"."),
        directory: metadata.is_dir() || (symlink && source.is_dir()),
        symlink,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        details: crate::columns::Details {
            folder: crate::folder_style::FolderId::capture(&path, &metadata),
            created: metadata.created().ok(),
            accessed: metadata.accessed().ok(),
            mode: Some(metadata.mode()),
            owner: Some(
                owners
                    .get(&metadata.uid())
                    .cloned()
                    .unwrap_or_else(|| metadata.uid().to_string()),
            ),
        },
        path,
    })
}

// Bounded local account labels only: no NSS/network lookup on the UI thread
// or unbounded per-entry subprocesses. Unknown/remote users retain their UID.
pub(crate) fn local_owners() -> std::collections::BTreeMap<u32, String> {
    use std::io::BufRead;
    let Ok(file) = fs::File::open("/etc/passwd") else {
        return Default::default();
    };
    let mut owners = std::collections::BTreeMap::new();
    for line in std::io::BufReader::new(file.take(256 * 1024))
        .lines()
        .map_while(Result::ok)
    {
        let mut fields = line.split(':');
        let name = fields.next().unwrap_or_default();
        let _ = fields.next();
        if let Some(uid) = fields.next().and_then(|s| s.parse().ok()) {
            owners.insert(uid, clean(name));
        }
    }
    owners
}

/// Read a tree branch through pinned no-follow directory descriptors. Reopening
/// the path after the scan detects replacements without exposing unrelated data.
pub(crate) fn read_tree_directory(
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Vec<Entry>>> {
    use rustix::fs::{Mode, OFlags};
    use std::{
        fs::File,
        os::{fd::AsRawFd, unix::fs::MetadataExt},
        path::Component,
    };
    fn pin(path: &Path) -> Result<File> {
        ensure!(path.is_absolute(), "Нужен абсолютный путь папки");
        let mut fd = File::from(rustix::fs::open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    fd = File::from(rustix::fs::openat(
                        &fd,
                        name,
                        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?)
                }
                _ => anyhow::bail!("Некорректный путь папки"),
            }
        }
        Ok(fd)
    }
    if cancelled() {
        return Ok(None);
    }
    let fd = pin(path)?;
    let before = fd.metadata()?;
    let descriptor = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
    let Some(mut entries) = read_directory_until(&descriptor, &cancelled)? else {
        return Ok(None);
    };
    if cancelled() {
        return Ok(None);
    }
    let after = pin(path)?.metadata()?;
    ensure!(
        (before.dev(), before.ino()) == (after.dev(), after.ino()),
        "Папка изменилась во время чтения. Повторите раскрытие."
    );
    for entry in &mut entries {
        entry.path = path.join(entry.path.file_name().unwrap());
        if let Some(id) = &mut entry.details.folder {
            id.rebind_scan_path(&entry.path);
        }
    }
    Ok(Some(entries))
}

pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
pub fn date(time: Option<SystemTime>) -> String {
    time.map(|t| {
        DateTime::<Local>::from(t)
            .format("%d.%m.%y %H:%M")
            .to_string()
    })
    .unwrap_or_else(|| "—".into())
}
pub fn kind(entry: &Entry) -> &'static str {
    if entry.symlink {
        "Ссылка"
    } else if entry.directory {
        "Папка"
    } else {
        match Path::new(&entry.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" | "c" | "h" | "cpp" | "py" | "js" | "ts" | "sh" => "Код",
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" => "Изображение",
            "flac" | "mp3" | "wav" | "ogg" | "opus" => "Аудио",
            "zip" | "gz" | "tar" | "xz" | "7z" | "zst" => "Архив",
            "mp4" | "mkv" | "webm" => "Видео",
            _ => "Файл",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Preview {
    pub title: String,
    pub details: Vec<String>,
    pub lines: Vec<String>,
    pub code: Option<crate::preview::syntax::CodePreview>,
    pub image: Option<std::sync::Arc<crate::preview::ImagePreview>>,
}

pub fn preview(path: &Path) -> Result<Preview> {
    preview_with_images(path, true)
}

pub fn preview_with_images(path: &Path, graphics: bool) -> Result<Preview> {
    preview_until(path, graphics, &crate::operations::Cancellation::default())
}
pub fn preview_until(
    path: &Path,
    _graphics: bool,
    _cancel: &crate::operations::Cancellation,
) -> Result<Preview> {
    let meta = fs::symlink_metadata(path)?;
    let mut result = Preview {
        title: clean(&path.file_name().unwrap_or_default().to_string_lossy()),
        details: vec![
            format!("Размер   {}", size(meta.len())),
            format!("Изменён  {}", date(meta.modified().ok())),
        ],
        lines: vec![],
        code: None,
        image: None,
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        result.details.push(format!(
            "Права    {:04o}",
            meta.permissions().mode() & 0o7777
        ));
    }
    if meta.is_symlink() {
        result.lines = vec![
            "Символическая ссылка".into(),
            clean(&format!("→ {}", fs::read_link(path)?.display())),
        ];
    } else if meta.is_dir() {
        result.details.remove(0); // Directory inode length is not the size of its contents.
        // Bound directory previews as well as text previews; don't collect a huge tree.
        result.lines.push("СОДЕРЖИМОЕ ПАПКИ".into());
        for entry in fs::read_dir(path)?.take(61) {
            let entry = entry?;
            let suffix = if entry.file_type()?.is_dir() { "/" } else { "" };
            result.lines.push(format!(
                "  {}{suffix}",
                clean(&entry.file_name().to_string_lossy())
            ));
        }
        if result.lines.len() > 61 {
            result.lines.truncate(61);
            result.lines.push("… остальные элементы".into());
        }
    } else if meta.is_file() {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            );
        }
        let mut file = options.open(path)?;
        ensure!(file.metadata()?.is_file(), "Файл изменился во время чтения");
        #[cfg(any(feature = "guitui", feature = "tui"))]
        {
            use std::io::{Seek, SeekFrom};
            let mut prefix = Vec::new();
            (&mut file).take(512).read_to_end(&mut prefix)?;
            let video = crate::preview::video::candidate(path, &prefix);
            let decoded = if video {
                Some(crate::preview::video::read(&mut file, _graphics, _cancel))
            } else {
                crate::preview::read(path, &mut file, &prefix, _graphics)
            };
            if let Some(decoded) = decoded {
                match decoded {
                    Ok(decoded) => {
                        result
                            .details
                            .extend(decoded.details.lines().map(str::to_owned));
                        result.image = decoded.image;
                        result.lines.push(if _graphics {
                            "Подготовка изображения…".into()
                        } else {
                            "Терминал не поддерживает графику".into()
                        });
                    }
                    Err(error) => {
                        result.lines = vec![
                            if video {
                                "Предпросмотр видео недоступен"
                            } else {
                                "Предпросмотр изображения недоступен"
                            }
                            .into(),
                            clean(&format!("{error:#}")),
                        ];
                    }
                }
                return Ok(result);
            }
            file.seek(SeekFrom::Start(0))?;
        }
        let mut bytes = Vec::new();
        file.take(65_537).read_to_end(&mut bytes)?;
        let truncated = bytes.len() > 65_536;
        bytes.truncate(65_536);
        // A byte budget may cut a valid final UTF-8 character in half.
        if truncated
            && let Err(error) = std::str::from_utf8(&bytes)
            && error.error_len().is_none()
        {
            bytes.truncate(error.valid_up_to());
        }
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            result.lines = vec![
                "Бинарный файл".into(),
                "Открывается внешним приложением".into(),
            ];
        } else {
            let text = std::str::from_utf8(&bytes)?;
            result.lines = text
                .lines()
                .take(300)
                .enumerate()
                .map(|(i, line)| format!("{:>3}  {}", i + 1, clean(&line.replace('\t', "    "))))
                .collect();
            result.code = crate::preview::syntax::highlight(path, &result.lines, _cancel);
            if let Some(code) = &result.code {
                result.details.push(format!("Язык     {}", code.language));
                if code.limited {
                    result.details.push("Подсветка ограничена".into());
                }
            }
            if truncated || text.lines().count() > 300 {
                result
                    .lines
                    .push("… предпросмотр ограничен: 64 KiB / 300 строк".into());
            }
            if bytes.is_empty() {
                result.lines.push("Пустой файл".into());
            }
        }
    } else {
        result
            .lines
            .push("Специальный файл · чтение отключено".into());
    }
    Ok(result)
}

pub fn open_external(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("explorer");
        c
    };
    let status = command
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Не удалось запустить внешнее приложение")?;
    ensure!(
        status.success(),
        "Внешнее приложение завершилось с {status}"
    );
    Ok(())
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn obsolete_scans_stop_between_entries_and_before_opening_the_directory() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(root.path().join(i.to_string()), "x").unwrap();
        }
        let calls = std::cell::Cell::new(0);
        let result = read_directory_until(root.path(), || {
            calls.set(calls.get() + 1);
            calls.get() == 4
        })
        .unwrap();
        assert!(result.is_none());
        assert_eq!(calls.get(), 4);
        assert!(
            read_directory_until(&root.path().join("missing"), || true)
                .unwrap()
                .is_none()
        );
        assert_eq!(read_directory(root.path()).unwrap().len(), 20);
    }
}

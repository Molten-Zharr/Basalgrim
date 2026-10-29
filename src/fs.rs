mod operations;
pub use operations::{
    Operation, Report, RetainedCopy, operate, operate_with_control, rename_new, transfer_one,
    trash_one, validate_name,
};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Local};
use std::{
    cmp::Reverse,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub hidden: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Name,
    Size,
    Modified,
}
impl Sort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Size,
            Self::Size => Self::Modified,
            Self::Modified => Self::Name,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Имя ↑",
            Self::Size => "Размер ↓",
            Self::Modified => "Дата ↓",
        }
    }
    pub fn sort(self, entries: &mut [Entry]) {
        entries.sort_by_cached_key(|e| {
            (
                !e.directory,
                match self {
                    Self::Size => Reverse(e.size),
                    _ => Reverse(0),
                },
                match self {
                    Self::Modified => Reverse(e.modified),
                    _ => Reverse(None),
                },
                e.name.to_lowercase(),
                e.path.clone(),
            )
        });
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
        entries.push(Entry {
            path,
            hidden: raw.as_encoded_bytes().starts_with(b"."),
            name,
            directory,
            symlink,
            size: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    Ok((!cancelled()).then_some(entries))
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
        match entry
            .path
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
}

pub fn preview(path: &Path) -> Result<Preview> {
    let meta = fs::symlink_metadata(path)?;
    let mut result = Preview {
        title: clean(&path.file_name().unwrap_or_default().to_string_lossy()),
        details: vec![
            format!("Размер   {}", size(meta.len())),
            format!("Изменён  {}", date(meta.modified().ok())),
        ],
        lines: vec![],
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
        let mut bytes = Vec::new();
        File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
        let truncated = bytes.len() > 65_536;
        bytes.truncate(65_536);
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

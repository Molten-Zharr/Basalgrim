use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Local};
use std::{
    cmp::Reverse,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
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
    fs::read_dir(path)
        .with_context(|| format!("Не удалось прочитать {}", path.display()))?
        .map(|entry| {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let symlink = metadata.is_symlink();
            let directory = metadata.is_dir() || (symlink && path.is_dir());
            let raw = entry.file_name();
            let name = clean(&raw.to_string_lossy());
            Ok(Entry {
                path,
                hidden: raw.as_encoded_bytes().starts_with(b"."),
                name,
                directory,
                symlink,
                size: metadata.len(),
                modified: metadata.modified().ok(),
            })
        })
        .collect()
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

#[derive(Debug, Clone)]
pub enum Operation {
    Create {
        parent: PathBuf,
        name: String,
        directory: bool,
    },
    Rename {
        source: PathBuf,
        name: String,
    },
    Transfer {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        cut: bool,
    },
    Trash {
        sources: Vec<PathBuf>,
    },
}

#[derive(Debug, Default)]
pub struct Report {
    pub completed: Vec<PathBuf>,
    pub total: usize,
    pub error: Option<String>,
}

pub fn validate_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    ensure!(
        !name.is_empty()
            && !name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            && matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none(),
        "Укажите имя без пути, /, \\ и управляющих символов"
    );
    Ok(())
}

/// Linux's RENAME_NOREPLACE prevents a concurrent writer from being overwritten.
pub fn rename_new(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            source,
            rustix::fs::CWD,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .with_context(|| {
            format!(
                "Перемещение {} → {} (без перезаписи; только в пределах файловой системы)",
                source.display(),
                destination.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        bail!("Безопасное перемещение пока поддерживается только в Linux")
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        #[cfg(not(unix))]
        bail!("Копирование ссылок пока поддерживается только в Unix");
    } else if meta.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, meta.permissions())?;
    } else if meta.is_file() {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        fs::set_permissions(destination, meta.permissions())?;
    } else {
        bail!("Специальные файлы не копируются: {}", source.display());
    }
    Ok(())
}

pub fn transfer_one(source: &Path, destination: &Path, cut: bool) -> Result<()> {
    let parent = destination
        .parent()
        .context("Нет родительской папки")?
        .canonicalize()?;
    let meta = fs::symlink_metadata(source)?;
    if meta.is_dir() {
        ensure!(
            !parent.starts_with(source.canonicalize()?),
            "Нельзя поместить папку в неё саму или её подпапку"
        );
    }
    ensure!(
        fs::symlink_metadata(destination).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        "Цель уже существует или недоступна: {}",
        destination.display()
    );
    if cut {
        return rename_new(source, destination);
    }
    // Stage each item on the target filesystem. Failure never exposes a partial destination.
    let staging = tempfile::Builder::new()
        .prefix(".basalgrim-copy-")
        .tempdir_in(parent)?;
    let staged = staging.path().join("item");
    let result = copy_tree(source, &staged).and_then(|()| rename_new(&staged, destination));
    let staging_path = staging.path().to_path_buf();
    // Preserved read-only directory modes must not prevent cleanup of our own staging tree.
    let cleanup = if result.is_err() {
        writable_directories(&staged)
    } else {
        Ok(())
    }
    .and_then(|()| staging.close().map_err(Into::into));
    match (result, cleanup) {
        // Publication succeeded; failure to remove an empty staging folder is not a failed copy.
        (Ok(()), _) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (result, Err(error)) => bail!(
            "{}; не удалось очистить временную папку {}: {error:#}",
            result
                .err()
                .map_or_else(|| "Копирование завершено".into(), |e| format!("{e:#}")),
            staging_path.display()
        ),
    }
}

fn writable_directories(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
            )?;
        }
        for entry in fs::read_dir(path)? {
            writable_directories(&entry?.path())?;
        }
    }
    Ok(())
}

pub fn operate(operation: Operation) -> Report {
    let tasks: Vec<(PathBuf, Result<()>)> = match operation {
        Operation::Create {
            parent,
            name,
            directory,
        } => {
            let path = parent.join(&name);
            let result = (|| {
                validate_name(&name)?;
                if directory {
                    fs::create_dir(&path)?;
                } else {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)?;
                }
                Ok(())
            })();
            vec![(path, result)]
        }
        Operation::Rename { source, name } => {
            let result = (|| {
                validate_name(&name)?;
                let destination = source
                    .parent()
                    .context("Нельзя переименовать корень")?
                    .join(name);
                rename_new(&source, &destination)
            })();
            vec![(source, result)]
        }
        Operation::Transfer {
            sources,
            destination,
            cut,
        } => {
            return run_batch(sources, |source| {
                let name = source.file_name().context("Нельзя копировать корень")?;
                transfer_one(source, &destination.join(name), cut)
            });
        }
        Operation::Trash { sources } => {
            return run_batch(sources, trash_one);
        }
    };
    let mut report = Report {
        total: tasks.len(),
        ..Report::default()
    };
    for (path, result) in tasks {
        match result {
            Ok(()) => report.completed.push(path),
            Err(e) => {
                report.error = Some(format!("{e:#}"));
                break;
            }
        }
    }
    report
}

fn run_batch(sources: Vec<PathBuf>, mut run: impl FnMut(&Path) -> Result<()>) -> Report {
    let mut report = Report {
        total: sources.len(),
        ..Report::default()
    };
    for source in sources {
        match run(&source) {
            Ok(()) => report.completed.push(source),
            Err(e) => {
                report.error = Some(format!("{e:#}"));
                break;
            }
        }
    }
    report
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

pub fn trash_one(path: &Path) -> Result<()> {
    let output = std::process::Command::new("gio")
        .args(["trash", "--"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .context("Для системной корзины установите gio (Debian/Ubuntu: libglib2.0-bin)")?;
    ensure!(
        output.status.success(),
        "Корзина: {}",
        clean(&String::from_utf8_lossy(&output.stderr))
    );
    Ok(())
}

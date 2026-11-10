//! Archive names are virtual paths. Only the operation worker publishes extracted data.
use crate::{fs::Entry, operations::Cancellation};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

mod create;
mod limits;
pub use create::create;

const MAX_MEMBERS: usize = 1_000_000;
const MAX_NAMES: usize = 128 * 1024 * 1024;
const PREVIEW_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
}
impl Format {
    pub fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.as_encoded_bytes().to_ascii_lowercase();
        if name.ends_with(b".tar.gz") || name.ends_with(b".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(b".tar") {
            Some(Self::Tar)
        } else if name.ends_with(b".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::Tar => ".tar",
            Self::TarGz => ".tar.gz",
        }
    }
}

pub fn unpacked_path(source: &Path) -> PathBuf {
    let raw = source.file_name().unwrap_or_default().as_encoded_bytes();
    let lower = raw.to_ascii_lowercase();
    let suffix = [b".tar.gz".as_slice(), b".tgz", b".tar", b".zip"]
        .into_iter()
        .find(|s| lower.ends_with(s))
        .unwrap_or_default();
    let name = &raw[..raw.len() - suffix.len()];
    source.with_file_name(std::ffi::OsString::from_vec(if name.is_empty() {
        "Распаковано".as_bytes().to_vec()
    } else {
        name.to_vec()
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Stamp {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Symlink(PathBuf),
    Hardlink(PathBuf),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    pub modified: Option<SystemTime>,
}
#[derive(Debug)]
pub struct Catalog {
    pub source: PathBuf,
    pub stamp: Stamp,
    pub members: BTreeMap<PathBuf, Member>,
}
#[derive(Clone, Debug)]
pub struct Extraction {
    pub source: PathBuf,
    pub stamp: Option<Stamp>,
    /// Paths relative to the archive; an empty list means the entire archive.
    pub selected: Vec<PathBuf>,
    pub base: PathBuf,
}
impl Extraction {
    pub fn all(source: PathBuf) -> Self {
        Self {
            source,
            stamp: None,
            selected: Vec::new(),
            base: PathBuf::new(),
        }
    }
    fn includes(&self, path: &Path) -> bool {
        path.starts_with(&self.base)
            && (self.selected.is_empty() || self.selected.iter().any(|p| path.starts_with(p)))
    }
}

pub(crate) fn check(cancel: &Cancellation) -> Result<()> {
    crate::fs::operation_checkpoint(cancel)
}
fn open(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Архив должен быть обычным файлом"
    );
    Ok(file)
}
/// Never sanitize a malicious name into a different, apparently safe filename.
pub fn safe_name(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().as_encoded_bytes().contains(&0),
        "Нулевой байт в имени архива"
    );
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(p) => {
                ensure!(
                    !out.as_os_str().is_empty() || !p.as_encoded_bytes().contains(&b':'),
                    "Абсолютный путь в архиве: {}",
                    path.display()
                );
                out.push(p);
            }
            Component::CurDir => {}
            _ => bail!("Небезопасный путь в архиве: {}", path.display()),
        }
    }
    ensure!(
        out.as_os_str().len() <= 64 * 1024,
        "Слишком длинное имя в архиве"
    );
    Ok(out)
}

struct Checked<'a, R> {
    inner: R,
    cancel: &'a Cancellation,
}
impl<R: Read> Read for Checked<'_, R> {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("Операция отменена"));
        }
        self.inner.read(b)
    }
}
impl<R: Seek> Seek for Checked<'_, R> {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("Операция отменена"));
        }
        self.inner.seek(p)
    }
}

fn visit(
    source: &Path,
    expected: Option<&Stamp>,
    cancel: &Cancellation,
    mut each: impl FnMut(Member, &mut dyn Read) -> Result<()>,
) -> Result<Stamp> {
    check(cancel)?;
    let mut file = open(source)?;
    let before = Stamp::of(&file.metadata()?);
    ensure!(
        expected.is_none_or(|e| e == &before),
        "Архив изменился; обновите его содержимое"
    );
    let identity = file.try_clone()?;
    let format = Format::of(source).context("Поддерживаются ZIP, TAR и TAR.GZ")?;
    if expected.is_none() {
        match format {
            Format::Zip => limits::zip(&mut file, cancel)?,
            Format::Tar => limits::tar(Checked {
                inner: &mut file,
                cancel,
            })?,
            Format::TarGz => limits::tar(Checked {
                inner: flate2::read::MultiGzDecoder::new(Checked {
                    inner: &mut file,
                    cancel,
                }),
                cancel,
            })?,
        }
        file.seek(SeekFrom::Start(0))?;
        ensure!(
            before == Stamp::of(&file.metadata()?),
            "Архив изменился при проверке каталога"
        );
    }
    let reader = Checked {
        inner: file,
        cancel,
    };
    match format {
        Format::Zip => {
            let mut zip = zip::ZipArchive::with_config(
                zip::read::Config {
                    archive_offset: zip::read::ArchiveOffset::Known(0),
                },
                reader,
            )
            .context("Не удалось прочитать ZIP")?;
            ensure!(
                zip.len() <= MAX_MEMBERS,
                "Слишком большой каталог ZIP (более {MAX_MEMBERS} записей)"
            );
            for i in 0..zip.len() {
                check(cancel)?;
                let mut entry = zip
                    .by_index(i)
                    .context("Не удалось прочитать запись ZIP; шифрование не поддерживается")?;
                let name = entry.name()?;
                ensure!(!name.contains('\\'), "Неоднозначное имя ZIP: {name}");
                let path = safe_name(Path::new(name.as_ref()))?;
                ensure!(
                    matches!(
                        entry.unix_mode().unwrap_or(0) & 0o170000,
                        0 | 0o100000 | 0o040000 | 0o120000
                    ),
                    "Неподдерживаемый специальный файл ZIP: {}",
                    path.display()
                );
                let kind = if entry.is_dir() {
                    Kind::Directory
                } else if entry.is_symlink() {
                    ensure!(entry.size() <= 64 * 1024, "Слишком длинная ссылка ZIP");
                    let mut bytes = Vec::new();
                    let expected = entry.size();
                    copy(&mut entry, &mut bytes, expected, cancel, &mut |_| Ok(()))?;
                    ensure!(!bytes.contains(&0), "Нулевой байт в ссылке ZIP");
                    Kind::Symlink(std::ffi::OsString::from_vec(bytes).into())
                } else {
                    Kind::File
                };
                let member = Member {
                    path,
                    kind,
                    size: entry.size(),
                    mode: entry.unix_mode().unwrap_or(0o644),
                    modified: None,
                };
                if !member.path.as_os_str().is_empty() {
                    each(member, &mut entry)?;
                }
            }
        }
        Format::Tar | Format::TarGz => {
            let input: Box<dyn Read + '_> = if format == Format::TarGz {
                Box::new(flate2::read::MultiGzDecoder::new(reader))
            } else {
                Box::new(reader)
            };
            let mut tar = tar::Archive::new(Checked {
                inner: input,
                cancel,
            });
            for (i, item) in tar.entries()?.enumerate() {
                check(cancel)?;
                ensure!(
                    i < MAX_MEMBERS,
                    "Слишком большой каталог TAR (более {MAX_MEMBERS} записей)"
                );
                let mut entry = item?;
                let path = safe_name(&entry.path()?)?;
                let header = entry.header();
                let ty = header.entry_type();
                let kind = if ty.is_dir() {
                    Kind::Directory
                } else if ty.is_file() || ty.is_gnu_sparse() {
                    Kind::File
                } else if ty.is_symlink() {
                    Kind::Symlink(
                        entry
                            .link_name()?
                            .context("Ссылка без адреса")?
                            .into_owned(),
                    )
                } else if ty.is_hard_link() {
                    Kind::Hardlink(safe_name(
                        &entry.link_name()?.context("Ссылка без адреса")?,
                    )?)
                } else {
                    bail!("Неподдерживаемый специальный файл TAR: {}", path.display());
                };
                let member = Member {
                    path,
                    kind,
                    size: entry.size(),
                    mode: header.mode()?,
                    modified: header
                        .mtime()
                        .ok()
                        .and_then(|s| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(s))),
                };
                if !member.path.as_os_str().is_empty() {
                    each(member, &mut entry)?;
                }
            }
        }
    }
    check(cancel)?;
    ensure!(
        before == Stamp::of(&identity.metadata()?)
            && before == Stamp::of(&fs::symlink_metadata(source)?),
        "Архив изменился во время чтения"
    );
    Ok(before)
}

impl Catalog {
    pub fn read(source: &Path, cancel: &Cancellation) -> Result<Arc<Self>> {
        let mut members: BTreeMap<PathBuf, Member> = BTreeMap::new();
        let mut names = 0usize;
        let stamp = visit(source, None, cancel, |member, _| {
            names = names.saturating_add(member.path.as_os_str().len());
            ensure!(names <= MAX_NAMES, "Каталог архива превышает 128 МиБ имён");
            for parent in member
                .path
                .ancestors()
                .skip(1)
                .filter(|p| !p.as_os_str().is_empty())
            {
                if let Some(existing) = members.get(parent) {
                    ensure!(
                        existing.kind == Kind::Directory,
                        "Родитель записи не папка: {}",
                        parent.display()
                    );
                } else {
                    names = names.saturating_add(parent.as_os_str().len());
                    members.insert(
                        parent.into(),
                        Member {
                            path: parent.into(),
                            kind: Kind::Directory,
                            size: 0,
                            mode: 0o755,
                            modified: None,
                        },
                    );
                    ensure!(
                        members.len() <= MAX_MEMBERS && names <= MAX_NAMES,
                        "Слишком большой каталог архива"
                    );
                }
            }
            if let Some(existing) = members.get(&member.path) {
                ensure!(
                    existing.kind == Kind::Directory && member.kind == Kind::Directory,
                    "Повторяющееся или конфликтующее имя: {}",
                    member.path.display()
                );
            }
            members.insert(member.path.clone(), member);
            ensure!(
                members.len() <= MAX_MEMBERS && names <= MAX_NAMES,
                "Слишком большой каталог архива"
            );
            Ok(())
        })?;
        Ok(Arc::new(Self {
            source: source.into(),
            stamp,
            members,
        }))
    }
    pub fn valid(&self) -> bool {
        fs::symlink_metadata(&self.source).is_ok_and(|m| m.is_file() && Stamp::of(&m) == self.stamp)
    }
    pub fn list(&self, cwd: &Path) -> Result<Vec<Entry>> {
        let inner = cwd.strip_prefix(&self.source)?;
        ensure!(
            inner.as_os_str().is_empty()
                || self
                    .members
                    .get(inner)
                    .is_some_and(|m| m.kind == Kind::Directory),
            "Нет такой папки в архиве"
        );
        Ok(self
            .members
            .values()
            .filter(|m| m.path.parent() == Some(inner))
            .map(|m| {
                let name = m.path.file_name().unwrap();
                Entry {
                    path: self.source.join(&m.path),
                    name: crate::fs::clean(&name.to_string_lossy()),
                    directory: m.kind == Kind::Directory,
                    symlink: matches!(m.kind, Kind::Symlink(_) | Kind::Hardlink(_)),
                    hidden: name.as_encoded_bytes().starts_with(b"."),
                    size: m.size,
                    modified: m.modified,
                }
            })
            .collect())
    }
    pub fn preview(
        &self,
        path: &Path,
        graphics: bool,
        cancel: &Cancellation,
    ) -> Result<crate::fs::Preview> {
        let relative = path.strip_prefix(&self.source)?;
        let member = self.members.get(relative).context("Нет записи в архиве")?;
        let mut preview = crate::fs::Preview {
            title: crate::fs::clean(&relative.to_string_lossy()),
            details: vec![
                format!("Архив: {}", self.source.display()),
                format!("Размер: {}", crate::fs::size(member.size)),
            ],
            ..Default::default()
        };
        if member.kind == Kind::Directory {
            preview.lines = self
                .list(path)?
                .iter()
                .take(200)
                .map(|e| format!("{}{}", e.name, if e.directory { "/" } else { "" }))
                .collect();
            return Ok(preview);
        }
        if let Kind::Symlink(target) | Kind::Hardlink(target) = &member.kind {
            preview.lines.push(format!(
                "Ссылка → {}",
                crate::fs::clean(&target.to_string_lossy())
            ));
            return Ok(preview);
        }
        if member.size > PREVIEW_LIMIT {
            preview.lines.push(
                "Файл слишком велик для предпросмотра из архива (64 МиБ). Распакуйте его.".into(),
            );
            return Ok(preview);
        }
        let temp = tempfile::tempdir()?;
        let output = self.materialize(path, temp.path(), PREVIEW_LIMIT, cancel)?;
        let mut content = crate::fs::preview_until(&output, graphics, cancel)?;
        content.title = preview.title;
        content.details = preview.details;
        Ok(content)
    }
    pub fn materialize(
        &self,
        path: &Path,
        directory: &Path,
        limit: u64,
        cancel: &Cancellation,
    ) -> Result<PathBuf> {
        let relative = path.strip_prefix(&self.source)?;
        let member = self.members.get(relative).context("Нет записи в архиве")?;
        ensure!(
            member.kind == Kind::File,
            "Ссылки и папки можно распаковать через меню"
        );
        ensure!(
            member.size <= limit,
            "Файл слишком велик для временного открытия; распакуйте его"
        );
        let output = directory.join(relative.file_name().context("Нет имени")?);
        let mut found = false;
        visit(&self.source, Some(&self.stamp), cancel, |m, input| {
            if m.path == relative {
                ensure!(m == *member, "Запись архива изменилась");
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&output)?;
                copy(input, &mut file, member.size, cancel, &mut |_| Ok(()))?;
                found = true;
            }
            Ok(())
        })?;
        ensure!(found, "Запись архива исчезла");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o400))?;
        Ok(output)
    }
}

pub type Directory = (PathBuf, Vec<Entry>, Option<Arc<Catalog>>);
pub fn directory(
    path: &Path,
    cached: Option<Arc<Catalog>>,
    cancel: &Cancellation,
) -> Result<Option<Directory>> {
    check(cancel)?;
    if let Some(catalog) = cached.filter(|c| path.starts_with(&c.source) && c.valid()) {
        let entries = catalog.list(path)?;
        return Ok(Some((path.into(), entries, Some(catalog))));
    }
    if path.is_dir() {
        return Ok(None);
    }
    for source in path.ancestors() {
        check(cancel)?;
        if Format::of(source).is_some() && fs::symlink_metadata(source).is_ok_and(|m| m.is_file()) {
            let inner = path.strip_prefix(source)?;
            let resolved = source.canonicalize()?;
            let cwd = resolved.join(inner);
            let catalog = Catalog::read(&resolved, cancel)?;
            let entries = catalog.list(&cwd)?;
            return Ok(Some((cwd, entries, Some(catalog))));
        }
    }
    Ok(None)
}

fn copy(
    input: &mut dyn Read,
    output: &mut dyn Write,
    expected: u64,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(u64) -> Result<()>,
) -> Result<()> {
    let mut buf = [0; 64 * 1024];
    let mut total = 0u64;
    loop {
        check(cancel)?;
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .context("Размер записи переполнен")?;
        ensure!(
            total <= expected,
            "Запись архива превышает заявленный размер"
        );
        output.write_all(&buf[..n])?;
        notify(n as u64)?;
    }
    ensure!(
        total == expected,
        "Запись архива обрезана: {total} / {expected}"
    );
    Ok(())
}

/// Destination must be a fresh private staging directory, never a user directory.
pub fn extract(
    request: &Extraction,
    destination: &Path,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Path, u64) -> Result<()>,
) -> Result<()> {
    let catalog = Catalog::read(&request.source, cancel)?;
    ensure!(
        request.stamp.as_ref().is_none_or(|s| s == &catalog.stamp),
        "Архив изменился; обновите содержимое перед распаковкой"
    );
    ensure!(
        safe_name(&request.base)? == request.base
            && (request.base.as_os_str().is_empty()
                || catalog
                    .members
                    .get(&request.base)
                    .is_some_and(|m| m.kind == Kind::Directory)),
        "Неверная папка архива"
    );
    for path in &request.selected {
        ensure!(
            catalog.members.contains_key(path) && path.starts_with(&request.base),
            "Выбранная запись отсутствует в архиве"
        );
    }
    fs::create_dir(destination)?;
    let mut dirs = Vec::new();
    for member in catalog
        .members
        .values()
        .filter(|m| request.includes(&m.path))
    {
        check(cancel)?;
        let target = destination.join(member.path.strip_prefix(&request.base)?);
        if member.kind == Kind::Directory {
            fs::create_dir_all(&target)?;
            dirs.push((target, member.mode));
        } else if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
    }
    visit(
        &request.source,
        Some(&catalog.stamp),
        cancel,
        |member, input| {
            if !request.includes(&member.path) {
                return Ok(());
            }
            ensure!(
                catalog.members.get(&member.path) == Some(&member),
                "Запись архива изменилась"
            );
            let target = destination.join(member.path.strip_prefix(&request.base)?);
            if member.kind == Kind::File {
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&target)?;
                copy(input, &mut file, member.size, cancel, &mut |n| {
                    notify(&member.path, n)
                })?;
                file.set_permissions(fs::Permissions::from_mode(0o600 | (member.mode & 0o177)))?;
                if let Some(time) = member.modified {
                    file.set_times(fs::FileTimes::new().set_modified(time))?;
                }
                file.sync_all()?;
            }
            notify(&member.path, 0)
        },
    )?;
    // Links are installed last: no archive-provided symlink can ever be a write parent.
    for member in catalog
        .members
        .values()
        .filter(|m| request.includes(&m.path))
    {
        check(cancel)?;
        let target = destination.join(member.path.strip_prefix(&request.base)?);
        match &member.kind {
            Kind::Symlink(link) => std::os::unix::fs::symlink(link, &target)?,
            Kind::Hardlink(link) => {
                let mut link = link;
                let mut seen = std::collections::BTreeSet::new();
                loop {
                    check(cancel)?;
                    ensure!(seen.insert(link), "Циклическая жёсткая ссылка");
                    match catalog.members.get(link).map(|m| &m.kind) {
                        Some(Kind::File) => break,
                        Some(Kind::Hardlink(next)) => link = next,
                        _ => bail!("Цель жёсткой ссылки не обычный файл архива"),
                    }
                }
                ensure!(
                    request.includes(link),
                    "Цель жёсткой ссылки не выбрана: {}",
                    link.display()
                );
                let source = destination.join(link.strip_prefix(&request.base)?);
                ensure!(
                    fs::symlink_metadata(&source)?.is_file(),
                    "Цель жёсткой ссылки не обычный файл"
                );
                fs::hard_link(source, target)?;
            }
            _ => {}
        }
    }
    for (path, mode) in dirs.into_iter().rev() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700 | (mode & 0o77)))?;
    }
    File::open(destination)?.sync_all()?;
    check(cancel)
}

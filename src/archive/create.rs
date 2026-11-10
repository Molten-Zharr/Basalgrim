use super::*;
use std::{collections::BTreeSet, os::fd::AsRawFd};

enum Writer {
    Zip(Box<zip::ZipWriter<File>>),
    Tar(tar::Builder<File>),
    Gzip(tar::Builder<flate2::write::GzEncoder<File>>),
}
struct Tracked<'a> {
    file: &'a mut File,
    cancel: &'a Cancellation,
    path: &'a Path,
    notify: &'a mut dyn FnMut(&Path, u64) -> Result<()>,
}
impl Read for Tracked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        check(self.cancel).map_err(std::io::Error::other)?;
        let n = self.file.read(buf)?;
        if n > 0 {
            (self.notify)(self.path, n as u64).map_err(std::io::Error::other)?;
        }
        Ok(n)
    }
}
impl Writer {
    fn entry(
        &mut self,
        source: &Path,
        name: &Path,
        meta: &fs::Metadata,
        cancel: &Cancellation,
        notify: &mut dyn FnMut(&Path, u64) -> Result<()>,
    ) -> Result<()> {
        check(cancel)?;
        let name = safe_name(name)?;
        let link = if meta.is_symlink() {
            Some(fs::read_link(source)?)
        } else {
            None
        };
        ensure!(
            meta.is_dir() || meta.is_file() || link.is_some(),
            "Нельзя добавить специальный файл: {}",
            source.display()
        );
        let mut file = if meta.is_file() {
            Some(open(source)?)
        } else {
            None
        };
        if let Some(f) = &file {
            ensure!(
                Stamp::of(&f.metadata()?) == Stamp::of(meta),
                "Файл изменился: {}",
                source.display()
            );
        }
        match self {
            Self::Zip(writer) => {
                let name = name.to_str().context(
                    "ZIP требует UTF-8 в именах; используйте TAR для исходных Unix-имён",
                )?;
                ensure!(
                    !name.contains('\\'),
                    "Для имён с обратной косой чертой используйте TAR"
                );
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .unix_permissions(meta.mode() & 0o777)
                    .large_file(meta.len() >= u32::MAX as u64);
                if meta.is_dir() {
                    writer.add_directory(name, options)?;
                } else if let Some(link) = link {
                    writer.add_symlink(
                        name,
                        link.to_str()
                            .context("Ссылка не в UTF-8; используйте TAR")?,
                        options,
                    )?;
                } else {
                    writer.start_file(name, options)?;
                    copy(
                        file.as_mut().unwrap(),
                        writer,
                        meta.len(),
                        cancel,
                        &mut |n| notify(Path::new(name), n),
                    )?;
                }
            }
            Self::Tar(writer) => append(
                writer,
                &name,
                meta,
                link.as_deref(),
                file.as_mut(),
                cancel,
                notify,
            )?,
            Self::Gzip(writer) => append(
                writer,
                &name,
                meta,
                link.as_deref(),
                file.as_mut(),
                cancel,
                notify,
            )?,
        }
        if let Some(f) = file {
            ensure!(
                Stamp::of(&f.metadata()?) == Stamp::of(meta),
                "Файл изменился во время архивации: {}",
                source.display()
            );
        }
        notify(&name, 0)
    }
    fn finish(self) -> Result<()> {
        let file = match self {
            Self::Zip(writer) => writer.finish()?,
            Self::Tar(writer) => writer.into_inner()?,
            Self::Gzip(writer) => writer.into_inner()?.finish()?,
        };
        file.sync_all()?;
        Ok(())
    }
}
fn append<W: Write>(
    writer: &mut tar::Builder<W>,
    name: &Path,
    meta: &fs::Metadata,
    link: Option<&Path>,
    file: Option<&mut File>,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Path, u64) -> Result<()>,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_metadata(meta);
    header.set_mode(meta.mode() & 0o777);
    header.set_uid(0);
    header.set_gid(0);
    if let Some(link) = link {
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        writer.append_link(&mut header, name, link)?;
    } else if let Some(file) = file {
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(meta.len());
        writer.append_data(
            &mut header,
            name,
            Tracked {
                file,
                cancel,
                path: name,
                notify,
            },
        )?;
    } else {
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        writer.append_data(&mut header, name, std::io::empty())?;
    }
    Ok(())
}
struct WalkState {
    count: usize,
    mount: u64,
}
fn mount_id(path: &Path) -> Result<u64> {
    use rustix::fs::{AtFlags, CWD, StatxFlags};
    let stat = rustix::fs::statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::MNT_ID)?;
    ensure!(
        stat.stx_mask & StatxFlags::MNT_ID.bits() != 0,
        "Не удалось проверить границы файловой системы"
    );
    Ok(stat.stx_mnt_id)
}
fn walk(
    writer: &mut Writer,
    source: &Path,
    name: &Path,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Path, u64) -> Result<()>,
    depth: usize,
    state: &mut WalkState,
) -> Result<()> {
    check(cancel)?;
    ensure!(depth < 256, "Глубина папок превышает 256 уровней");
    state.count += 1;
    ensure!(
        state.count <= MAX_MEMBERS,
        "Слишком много записей для одного архива"
    );
    ensure!(
        mount_id(source)? == state.mount,
        "Вложенная точка монтирования: {}",
        name.display()
    );
    let meta = fs::symlink_metadata(source)?;
    writer.entry(source, name, &meta, cancel, notify)?;
    if meta.is_dir() {
        let fd = rustix::fs::open(
            source,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?;
        let file = File::from(fd);
        ensure!(
            Stamp::of(&file.metadata()?) == Stamp::of(&meta),
            "Папка изменилась во время архивации"
        );
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        for item in fs::read_dir(&pinned)? {
            check(cancel)?;
            let item = item?;
            walk(
                writer,
                &item.path(),
                &name.join(item.file_name()),
                cancel,
                notify,
                depth + 1,
                state,
            )?;
        }
        ensure!(
            Stamp::of(&file.metadata()?) == Stamp::of(&meta),
            "Состав папки изменился во время архивации"
        );
    }
    Ok(())
}
pub fn create(
    sources: &[PathBuf],
    destination: &Path,
    format: Format,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Path, u64) -> Result<()>,
) -> Result<()> {
    ensure!(!sources.is_empty(), "Нет выбранных файлов");
    let mut names = BTreeSet::new();
    let parent = destination
        .parent()
        .context("Нет папки назначения")?
        .canonicalize()?;
    for source in sources {
        check(cancel)?;
        ensure!(
            names.insert(source.file_name().context("Нельзя архивировать корень")?),
            "Выбраны файлы с одинаковыми именами"
        );
        if fs::symlink_metadata(source)?.is_dir() {
            ensure!(
                !parent.starts_with(source.canonicalize()?),
                "Нельзя создавать архив внутри включённой в него папки"
            );
        }
    }
    let file = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    let mut writer = match format {
        Format::Zip => Writer::Zip(Box::new(zip::ZipWriter::new(file))),
        Format::Tar => Writer::Tar(tar::Builder::new(file)),
        Format::TarGz => Writer::Gzip(tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::default(),
        ))),
    };
    let mut state = WalkState { count: 0, mount: 0 };
    for source in sources {
        state.mount = mount_id(source)?;
        walk(
            &mut writer,
            source,
            Path::new(source.file_name().unwrap()),
            cancel,
            notify,
            0,
            &mut state,
        )?;
    }
    check(cancel)?;
    writer.finish()
}

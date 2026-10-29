//! Copy/verify/publish/remove for EXDEV moves. Source deletion never uses remove_dir_all.
use super::*;
use rustix::fs::{AtFlags, CWD, Mode, OFlags, StatxFlags};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    os::unix::fs::MetadataExt,
};

#[derive(Debug)]
pub(super) struct PartialMove(pub RetainedCopy);
impl fmt::Display for PartialMove {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Копия создана в {}; перенос {} не завершён",
            self.0.destination.display(),
            self.0.source.display()
        )?;
        if let Some(path) = &self.0.recovery {
            write!(f, "; остаток исходника: {}", path.display())?;
        }
        Ok(())
    }
}

struct Snapshot {
    meta: fs::Metadata,
    link: Option<PathBuf>,
    children: BTreeMap<OsString, Snapshot>,
    copied: Option<fs::Metadata>,
}

fn mount_id(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::MNT_ID)
        .context("Не удалось проверить границы файловой системы")?;
    ensure!(
        stat.stx_mask & StatxFlags::MNT_ID.bits() != 0,
        "Для проверки границ файловой системы требуется Linux 5.8 или новее"
    );
    Ok(stat.stx_mnt_id)
}

fn same(a: &fs::Metadata, b: &fs::Metadata, renamed: bool) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.len() == b.len()
        && a.uid() == b.uid()
        && a.gid() == b.gid()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && (renamed || (a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()))
}

fn names(path: &Path) -> Result<BTreeSet<OsString>> {
    fs::read_dir(path)?
        .map(|entry| Ok(entry?.file_name()))
        .collect()
}

fn open_dir(path: &Path) -> Result<File> {
    Ok(File::from(rustix::fs::openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

impl Snapshot {
    fn capture(path: &Path, mount: u64, control: &mut Control<'_>) -> Result<Self> {
        control.current(path)?;
        ensure!(
            mount_id(path)? == mount,
            "Вложенная точка монтирования не переносится: {}",
            path.display()
        );
        let meta = fs::symlink_metadata(path)?;
        let mut children = BTreeMap::new();
        let link = if meta.is_symlink() {
            Some(fs::read_link(path)?)
        } else if meta.is_dir() {
            for name in names(path)? {
                children.insert(
                    name.clone(),
                    Self::capture(&path.join(name), mount, control)?,
                );
            }
            None
        } else {
            ensure!(
                meta.is_file(),
                "Специальный файл не переносится между дисками: {}",
                path.display()
            );
            None
        };
        let result = Self {
            meta,
            children,
            link,
            copied: None,
        };
        result.check_source(path, false)?;
        Ok(result)
    }

    fn check_source(&self, path: &Path, renamed: bool) -> Result<()> {
        ensure!(
            same(&self.meta, &fs::symlink_metadata(path)?, renamed),
            "Источник изменился: {}",
            path.display()
        );
        if self.meta.is_dir() {
            ensure!(
                names(path)? == self.children.keys().cloned().collect(),
                "Содержимое папки изменилось: {}",
                path.display()
            );
        } else if let Some(link) = &self.link {
            ensure!(
                &fs::read_link(path)? == link,
                "Ссылка изменилась: {}",
                path.display()
            );
        }
        Ok(())
    }

    fn copy(&mut self, source: &Path, destination: &Path, control: &mut Control<'_>) -> Result<()> {
        control.current(source)?;
        self.check_source(source, false)?;
        if let Some(link) = &self.link {
            std::os::unix::fs::symlink(link, destination)?;
        } else if self.meta.is_dir() {
            fs::create_dir(destination)?;
            for (name, child) in &mut self.children {
                child.copy(&source.join(name), &destination.join(name), control)?;
            }
            fs::set_permissions(destination, self.meta.permissions())?;
            let dir = open_dir(destination)?;
            dir.set_times(fs::FileTimes::new().set_modified(self.meta.modified()?))?;
            dir.sync_all()?;
        } else {
            copy_file(
                source,
                destination,
                &self.meta,
                control,
                Some(self.meta.len()),
            )?;
            let file = open_regular(destination)?;
            file.set_times(fs::FileTimes::new().set_modified(self.meta.modified()?))?;
            file.sync_all()?;
        }
        self.check_source(source, false)?;
        self.copied = Some(fs::symlink_metadata(destination)?);
        control.entry_done(source);
        Ok(())
    }

    fn verify(
        &self,
        source: &Path,
        destination: &Path,
        renamed: bool,
        control: &mut Control<'_>,
    ) -> Result<()> {
        control.current(source)?;
        self.check_source(source, renamed)?;
        let copied = self.copied.as_ref().unwrap();
        ensure!(
            same(copied, &fs::symlink_metadata(destination)?, renamed),
            "Копия изменена или заменена: {}",
            destination.display()
        );
        if let Some(link) = &self.link {
            ensure!(
                &fs::read_link(destination)? == link,
                "Копия ссылки изменилась: {}",
                destination.display()
            );
        } else if self.meta.is_dir() {
            ensure!(
                names(destination)? == self.children.keys().cloned().collect(),
                "Содержимое копии изменилось: {}",
                destination.display()
            );
            for (name, child) in &self.children {
                child.verify(&source.join(name), &destination.join(name), false, control)?;
            }
        } else {
            let mut input = open_regular(source)?;
            let mut output = open_regular(destination)?;
            ensure!(
                same(&self.meta, &input.metadata()?, renamed)
                    && same(copied, &output.metadata()?, renamed),
                "Файл заменён при проверке: {}",
                source.display()
            );
            let mut a = vec![0; 256 * 1024];
            let mut b = vec![0; a.len()];
            let mut left = self.meta.len();
            while left > 0 {
                control.checkpoint()?;
                let count = left.min(a.len() as u64) as usize;
                input.read_exact(&mut a[..count])?;
                output.read_exact(&mut b[..count])?;
                ensure!(
                    a[..count] == b[..count],
                    "Содержимое копии отличается: {}",
                    destination.display()
                );
                left -= count as u64;
                control.emit();
            }
            ensure!(
                input.read(&mut [0])? == 0 && output.read(&mut [0])? == 0,
                "Размер файла изменился при проверке"
            );
            ensure!(
                same(&self.meta, &input.metadata()?, renamed)
                    && same(copied, &output.metadata()?, renamed),
                "Файл изменился при проверке: {}",
                source.display()
            );
        }
        self.check_source(source, renamed)?;
        Ok(())
    }

    fn remove(
        &self,
        parent: &File,
        name: &std::ffi::OsStr,
        source: &Path,
        destination: &Path,
        control: &mut Control<'_>,
    ) -> Result<()> {
        let stat = rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
        ensure!(
            stat.st_dev == self.meta.dev() && stat.st_ino == self.meta.ino(),
            "Источник заменён перед удалением: {}",
            source.display()
        );
        let copied = self.copied.as_ref().unwrap();
        ensure!(
            same(copied, &fs::symlink_metadata(destination)?, true),
            "Копия изменилась перед удалением исходника: {}",
            destination.display()
        );
        if self.meta.is_dir() {
            let dir = File::from(rustix::fs::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            let opened = dir.metadata()?;
            ensure!(
                opened.dev() == self.meta.dev() && opened.ino() == self.meta.ino(),
                "Папка заменена перед удалением"
            );
            for (child_name, child) in &self.children {
                child.remove(
                    &dir,
                    child_name,
                    &source.join(child_name),
                    &destination.join(child_name),
                    control,
                )?;
            }
            // A new, unverified child makes rmdir fail; it is never deleted.
            rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR)?;
        } else {
            // ctime can change when deleting another hard link from this tree.
            ensure!(
                same(&self.meta, &fs::symlink_metadata(source)?, true),
                "Источник изменился перед удалением: {}",
                source.display()
            );
            rustix::fs::unlinkat(parent, name, AtFlags::empty())?;
        }
        control.emit(); // Source cleanup is committed and cannot be cancelled mid-item.
        Ok(())
    }
}

pub(super) fn transfer(
    source: &Path,
    destination: &Path,
    parent: &Path,
    control: &mut Control<'_>,
) -> Result<()> {
    let mut snapshot = Snapshot::capture(source, mount_id(source)?, control)?;
    let staging = tempfile::Builder::new()
        .prefix(".basalgrim-copy-")
        .tempdir_in(parent)?;
    let staged = staging.path().join("item");
    let mut published_dir = None;
    let result = (|| {
        snapshot.copy(source, &staged, control)?;
        control.progress.phase = Phase::Verifying;
        control.emit();
        snapshot.verify(source, &staged, false, control)?;
        open_dir(staging.path())?.sync_all()?;
        if snapshot.meta.is_dir() {
            use std::os::unix::fs::PermissionsExt;
            // Moving a directory between parents may require owner write on
            // its '..' entry. The staging parent remains private (0700).
            let dir = open_dir(&staged)?;
            dir.set_permissions(fs::Permissions::from_mode(snapshot.meta.mode() | 0o200))?;
            published_dir = Some(dir);
        }
        control.checkpoint()?;
        rename_new(&staged, destination)
    })();
    if let Err(error) = result {
        control.progress.phase = Phase::CleaningUp;
        control.emit();
        let staging_path = staging.path().to_path_buf();
        let cleanup =
            writable_directories(&staged).and_then(|()| staging.close().map_err(Into::into));
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(CleanupFailure(format!(
                "Не удалось очистить {}: {cleanup:#}",
                staging_path.display()
            )))),
        };
    }
    // Publication succeeded; the complete destination is never rolled back.
    let _ = staging.close();
    let mut retained = RetainedCopy {
        source: source.into(),
        destination: destination.into(),
        recovery: None,
    };
    let result = (|| -> Result<()> {
        if let Some(dir) = &published_dir {
            // Use the held inode, even if someone replaces the public path.
            dir.set_permissions(snapshot.meta.permissions())?;
            dir.sync_all()?;
        }
        open_dir(parent)?.sync_all()?;
        control.progress.phase = Phase::RemovingSource;
        control.emit();
        control.checkpoint()?;
        // Claim the source under a private name before destructive cleanup.
        // Never let a TempDir destructor recursively remove claimed user data.
        let source_parent = source
            .parent()
            .context("Нет родительской папки исходника")?;
        let claim_dir = tempfile::Builder::new()
            .prefix(".basalgrim-move-")
            .tempdir_in(source_parent)?
            .keep();
        let name = source.file_name().context("Нельзя перемещать корень")?;
        let claimed = claim_dir.join(name);
        if let Err(error) = rename_new(source, &claimed) {
            let _ = fs::remove_dir(&claim_dir);
            return Err(error);
        }
        let cleanup: Result<()> = (|| {
            snapshot.verify(&claimed, destination, true, control)?;
            control.checkpoint()?; // Last cancellable boundary before deleting source entries.
            let dir = open_dir(&claim_dir)?;
            snapshot.remove(&dir, name, &claimed, destination, control)?;
            // The source is fully removed now; a late directory fsync failure
            // must not turn the completed move into an unfinished clipboard item.
            let _ = dir.sync_all();
            Ok(())
        })();
        if let Err(error) = cleanup {
            if let Err(restore) = rename_new(&claimed, source) {
                retained.recovery = Some(claimed);
                return Err(error.context(CleanupFailure(format!(
                    "Не удалось восстановить исходный путь без перезаписи: {restore:#}"
                ))));
            }
            let _ = fs::remove_dir(&claim_dir);
            return Err(error);
        }
        let _ = fs::remove_dir(claim_dir);
        Ok(())
    })();
    match result {
        Ok(()) => {
            control.progress.phase = Phase::Running;
            Ok(())
        }
        Err(error) => Err(error.context(PartialMove(retained))),
    }
}

use super::*;
use crate::conflicts::{Choice, Conflict, Decision, DestinationRecovery, ItemInfo};
use std::{
    ffi::OsString,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
};

#[derive(Debug)]
pub(super) struct TargetExists(pub PathBuf);
impl fmt::Display for TargetExists {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Цель уже существует: {}", self.0.display())
    }
}
impl std::error::Error for TargetExists {}
#[derive(Debug)]
struct Changed;
impl fmt::Display for Changed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Цель изменилась; требуется новый выбор")
    }
}
impl std::error::Error for Changed {}

fn same(a: &fs::Metadata, b: &fs::Metadata, renamed: bool) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && (renamed || (a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()))
}

pub(super) struct Replacement {
    destination: PathBuf,
    expected: fs::Metadata,
    backup: Option<PathBuf>,
    published: bool,
}
impl Replacement {
    pub fn publish(
        &mut self,
        incoming: &Path,
        destination: &Path,
        mut journal: Option<&mut crate::journal::Recorder<'_>>,
        cancellation: &Cancellation,
    ) -> Result<()> {
        ensure!(
            destination == self.destination,
            "Назначение замены изменилось"
        );
        match fs::symlink_metadata(destination) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                crate::journal::tracked_rename(
                    journal,
                    "publish",
                    incoming,
                    destination,
                    false,
                    Some(cancellation),
                )?;
                self.published = true;
                return Ok(());
            }
            Ok(meta) if same(&meta, &self.expected, false) => {}
            Ok(_) => return Err(Changed.into()),
            Err(e) => return Err(e.into()),
        }
        let parent = destination.parent().context("Нет родителя цели")?;
        let dir = tempfile::Builder::new()
            .prefix(".basalgrim-replaced-")
            .tempdir_in(parent)?
            .keep();
        let backup = dir.join(destination.file_name().context("Нельзя заменить корень")?);
        if let Err(error) = crate::journal::tracked_rename(
            journal.as_deref_mut(),
            "replace_claim",
            destination,
            &backup,
            false,
            Some(cancellation),
        ) {
            let _ = fs::remove_dir(dir);
            return Err(error);
        }
        self.backup = Some(backup.clone());
        self.install(incoming, destination, &dir, journal, cancellation)
    }

    fn install(
        &mut self,
        incoming: &Path,
        destination: &Path,
        dir: &Path,
        mut journal: Option<&mut crate::journal::Recorder<'_>>,
        cancellation: &Cancellation,
    ) -> Result<()> {
        let backup = self.backup.as_ref().unwrap().clone();
        let result = (|| {
            ensure!(
                same(&self.expected, &fs::symlink_metadata(&backup)?, true),
                Changed
            );
            crate::journal::tracked_rename(
                journal.as_deref_mut(),
                "publish",
                incoming,
                destination,
                false,
                Some(cancellation),
            )
        })();
        match result {
            Ok(()) => {
                self.published = true;
                Ok(())
            }
            Err(error) => {
                if let Err(restore) = crate::journal::tracked_rename(
                    journal,
                    "restore_replaced",
                    &backup,
                    destination,
                    true,
                    None,
                ) {
                    return Err(error.context(CleanupFailure(format!("Прежняя цель сохранена в {}; восстановление без перезаписи не удалось: {restore:#}", backup.display()))));
                }
                self.backup = None;
                self.expected = fs::symlink_metadata(destination)?;
                let _ = fs::remove_dir(dir);
                Err(error)
            }
        }
    }
}

pub(super) fn has_recovery(control: &Control<'_, '_>) -> bool {
    control
        .replacement
        .as_ref()
        .is_some_and(|r| r.backup.is_some() && !r.published)
}

// Only the explicitly replaced object is discarded. Directory descriptors and
// mount IDs prevent following a link or deleting a nested mounted filesystem.
fn discard(parent: &File, name: &std::ffi::OsStr, mount: u64) -> Result<()> {
    use rustix::fs::{self as sys, AtFlags, Mode, OFlags, StatxFlags};
    let stat = sys::statx(parent, name, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::MNT_ID)?;
    ensure!(
        stat.stx_mask & StatxFlags::MNT_ID.bits() != 0 && stat.stx_mnt_id == mount,
        "Остаток заменённой цели содержит другую точку монтирования"
    );
    let before = sys::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if sys::FileType::from_raw_mode(before.st_mode) == sys::FileType::Directory {
        let dir = File::from(sys::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let meta = dir.metadata()?;
        ensure!(
            meta.dev() == before.st_dev && meta.ino() == before.st_ino,
            "Прежняя папка заменена при очистке"
        );
        for entry in sys::Dir::read_from(&dir)? {
            let entry = entry?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            discard(&dir, std::ffi::OsStr::from_bytes(raw), mount)?;
        }
        sys::unlinkat(parent, name, AtFlags::REMOVEDIR)?;
    } else {
        sys::unlinkat(parent, name, AtFlags::empty())?;
    }
    Ok(())
}

pub(super) fn finish(control: &mut Control<'_, '_>) -> Result<()> {
    let Some(replacement) = control.replacement.take() else {
        return Ok(());
    };
    let Some(backup) = replacement.backup else {
        return Ok(());
    };
    let dir = backup.parent().unwrap();
    let cleanup = if replacement.published {
        control.progress.phase = Phase::CleaningUp;
        control.emit();
        (|| -> Result<()> {
            ensure!(
                same(&replacement.expected, &fs::symlink_metadata(&backup)?, true),
                "Прежняя цель изменилась после подтверждения"
            );
            let parent = File::from(rustix::fs::openat(
                rustix::fs::CWD,
                dir,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?);
            let stat = rustix::fs::statx(
                &parent,
                ".",
                rustix::fs::AtFlags::empty(),
                rustix::fs::StatxFlags::MNT_ID,
            )?;
            let step = control.before_step("discard_replaced", Some(&backup), None)?;
            let result = discard(&parent, backup.file_name().unwrap(), stat.stx_mnt_id);
            control.after_step(step, "discard_replaced", None, &result);
            result?;
            Ok(())
        })()
    } else {
        bail_recovery(&backup)
    };
    match cleanup {
        Ok(()) => {
            let _ = fs::remove_dir(dir);
            control.progress.phase = Phase::Running;
            Ok(())
        }
        Err(error) => {
            control.recoveries.push(DestinationRecovery {
                destination: replacement.destination,
                backup: backup.clone(),
            });
            Err(error.context(CleanupFailure(format!(
                "Остаток прежней цели сохранён в {}",
                backup.display()
            ))))
        }
    }
}
fn bail_recovery(path: &Path) -> Result<()> {
    bail!("Не удалось вернуть прежнюю цель из {}", path.display())
}

pub(super) fn conclude(outcome: Result<bool>, control: &mut Control<'_, '_>) -> Result<()> {
    let cleanup = finish(control);
    match (outcome, cleanup) {
        (Err(error), Err(cleanup)) => Err(error.context(CleanupFailure(format!("{cleanup:#}")))),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), result) => result,
    }
}

fn info(meta: &fs::Metadata) -> ItemInfo {
    ItemInfo {
        kind: if meta.is_symlink() {
            "Ссылка"
        } else if meta.is_dir() {
            "Папка"
        } else if meta.is_file() {
            "Файл"
        } else {
            "Специальный файл"
        },
        bytes: meta.is_file().then_some(meta.len()),
        modified: meta.modified().ok(),
    }
}
fn location(path: &Path) -> Result<PathBuf> {
    Ok(path
        .parent()
        .context("Нет родительской папки")?
        .canonicalize()?
        .join(path.file_name().context("Нет имени")?))
}
fn numbered(path: &Path, directory: bool, number: usize) -> Result<PathBuf> {
    let name = path.file_name().context("Нет имени цели")?;
    let raw = name.as_bytes();
    let split = if directory {
        None
    } else {
        raw.iter().rposition(|&b| b == b'.').filter(|&n| n > 0)
    };
    let (mut stem, mut extension) = split.map_or((raw, &[][..]), |n| (&raw[..n], &raw[n..]));
    let suffix = format!(" ({number})");
    if extension.len() + suffix.len() >= 255 {
        stem = raw;
        extension = &[];
    }
    let mut length = stem.len().min(255 - suffix.len() - extension.len());
    if let Ok(text) = std::str::from_utf8(stem) {
        while !text.is_char_boundary(length) {
            length -= 1;
        }
    }
    let mut bytes = stem[..length].to_vec();
    bytes.extend(suffix.as_bytes());
    bytes.extend(extension);
    Ok(path.with_file_name(OsString::from_vec(bytes)))
}

pub(super) struct Request<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub cut: bool,
    pub rename: bool,
    pub remaining: usize,
}

pub(super) fn run(
    request: Request<'_>,
    policy: &mut Option<Choice>,
    resolve: &mut Option<&mut dyn FnMut(&Conflict) -> Decision>,
    control: &mut Control<'_, '_>,
) -> Result<bool> {
    let Request {
        source,
        destination,
        cut,
        rename,
        remaining,
    } = request;
    if resolve.is_none() && policy.is_none() {
        if rename {
            control.publish(source, destination)?;
        } else {
            transfer(source, destination, cut, control)?;
        }
        return Ok(true);
    }
    validate_paths(source, destination)?;
    let same_location = location(source)? == location(destination)?;
    if same_location && cut {
        return Ok(false);
    }
    let mut target = destination.to_path_buf();
    let mut number = 0;
    let mut keep_both = false;
    let mut force_prompt = false;
    for _ in 0..10_000 {
        control.checkpoint()?;
        let existing = match fs::symlink_metadata(&target) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(meta) = existing {
            if keep_both {
                number += 1;
                target = numbered(destination, fs::symlink_metadata(source)?.is_dir(), number)?;
                continue;
            }
            let incoming = fs::symlink_metadata(source)?;
            let conflict = Conflict {
                source: source.into(),
                destination: target.clone(),
                incoming: info(&incoming),
                existing: info(&meta),
                remaining,
                can_replace: !(incoming.dev() == meta.dev() && incoming.ino() == meta.ino()),
                can_keep_both: true,
            };
            let choice = if !force_prompt && policy.is_some_and(|c| conflict.allows(c)) {
                policy.unwrap()
            } else {
                control.progress.phase = Phase::AwaitingDecision;
                control.emit();
                let resolver = resolve
                    .as_mut()
                    .context("Совпадение требует явного выбора")?;
                let decision = resolver(&conflict);
                control.checkpoint()?;
                ensure!(
                    conflict.allows(decision.choice),
                    "Недоступный вариант разрешения совпадения"
                );
                if decision.remaining && decision.choice != Choice::Cancel {
                    *policy = Some(decision.choice);
                }
                decision.choice
            };
            control.progress.phase = Phase::Running;
            control.emit();
            match choice {
                Choice::Cancel => {
                    control.cancellation.cancel();
                    return Err(Cancelled.into());
                }
                Choice::Skip => return Ok(false),
                Choice::KeepBoth => {
                    keep_both = true;
                    number += 1;
                    target = numbered(destination, incoming.is_dir(), number)?;
                    continue;
                }
                Choice::Replace => {
                    control.replacement = Some(Replacement {
                        destination: target.clone(),
                        expected: meta,
                        backup: None,
                        published: false,
                    })
                }
            }
        }
        let result = if rename {
            control.publish(source, &target)
        } else {
            transfer(source, &target, cut, control)
        };
        match result {
            Ok(()) => return Ok(true),
            Err(error) => {
                let retry = error.is::<Changed>()
                    || error.is::<TargetExists>()
                    || error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::EXIST);
                if !retry
                    || has_recovery(control)
                    || error.downcast_ref::<CleanupFailure>().is_some()
                {
                    return Err(error);
                }
                finish(control)?;
                force_prompt = true;
            }
        }
    }
    bail!("Не удалось выбрать свободное имя после 10000 попыток")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_publication_restores_the_old_target_without_removing_it() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::write(&target, "original").unwrap();
        let mut replacement = Replacement {
            destination: target.clone(),
            expected: fs::symlink_metadata(&target).unwrap(),
            backup: None,
            published: false,
        };
        assert!(
            replacement
                .publish(
                    &root.path().join("missing"),
                    &target,
                    None,
                    &Cancellation::default()
                )
                .is_err()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert!(replacement.backup.is_none());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_race_between_claim_and_publication_preserves_both_the_new_occupant_and_backup() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let incoming = root.path().join("incoming");
        fs::write(&target, "approved original").unwrap();
        fs::write(&incoming, "incoming").unwrap();
        let expected = fs::symlink_metadata(&target).unwrap();
        let dir = tempfile::tempdir_in(root.path()).unwrap().keep();
        let backup = dir.join("target");
        rename_new(&target, &backup).unwrap();
        fs::write(&target, "new occupant").unwrap();
        let mut replacement = Replacement {
            destination: target.clone(),
            expected,
            backup: Some(backup.clone()),
            published: false,
        };
        let error = replacement
            .install(&incoming, &target, &dir, None, &Cancellation::default())
            .unwrap_err();
        assert!(error.downcast_ref::<CleanupFailure>().is_some());
        assert_eq!(fs::read_to_string(&target).unwrap(), "new occupant");
        assert_eq!(fs::read_to_string(&incoming).unwrap(), "incoming");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "approved original");
        let cancel = Cancellation::default();
        let mut notify = |_: &Progress| {};
        let mut control = Control {
            cancellation: &cancel,
            progress: Progress::default(),
            notify: &mut notify,
            replacement: Some(replacement),
            recoveries: Vec::new(),
            journal: None,
        };
        assert!(finish(&mut control).is_err());
        assert_eq!(control.recoveries[0].backup, backup);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "approved original");
    }

    #[test]
    fn discarding_a_backup_rejects_mount_boundaries_before_unlinking_anything() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("keep"), "precious").unwrap();
        let dir = File::open(root.path()).unwrap();
        assert!(discard(&dir, std::ffi::OsStr::new("keep"), u64::MAX).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("keep")).unwrap(),
            "precious"
        );
    }
}

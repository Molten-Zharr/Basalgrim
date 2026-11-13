//! Route ordinary operations before their first mutation, never retry a partial result.
use super::*;
use crate::{
    conflicts::{Conflict, Decision},
    fs::Report,
    operations::{Cancellation, Progress},
};
use rustix::fs::{Access, AtFlags, CWD, accessat};
use std::os::unix::fs::MetadataExt;

fn denied(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}
fn needs(path: &Path, mode: Access) -> bool {
    // Effective credentials and the kernel's ACL check, not guesses from mode bits.
    matches!(
        accessat(CWD, path, mode, AtFlags::EACCESS),
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM)
    )
}
fn parent_needs_write(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if needs(parent, Access::WRITE_OK | Access::EXEC_OK) {
        return true;
    }
    // Unlink/rename of another owner's entry in a sticky directory.
    if let (Ok(dir), Ok(file)) = (fs::metadata(parent), fs::symlink_metadata(path)) {
        let uid = rustix::process::geteuid().as_raw();
        return dir.mode() & 0o1000 != 0 && dir.uid() != uid && file.uid() != uid;
    }
    false
}
fn readable(path: &Path, removing: bool, cancel: &Cancellation, depth: usize) -> Result<bool> {
    crate::fs::operation_checkpoint(cancel)?;
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return Ok(denied(&e)),
    };
    if meta.is_symlink() {
        return Ok(false);
    }
    if !meta.is_dir() {
        return Ok(meta.is_file() && needs(path, Access::READ_OK));
    }
    if needs(path, Access::READ_OK | Access::EXEC_OK)
        || (removing && needs(path, Access::WRITE_OK | Access::EXEC_OK))
    {
        return Ok(true);
    }
    // Keep preflight bounded by the helper's supported depth. Ordinary operations
    // retain their existing behavior beyond it; a later failure is never replayed.
    if depth >= 128 {
        return Ok(false);
    }
    let fd = match rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(e) => {
            return Ok(matches!(
                e,
                rustix::io::Errno::ACCESS | rustix::io::Errno::PERM
            ));
        }
    };
    use std::os::fd::AsRawFd;
    let entries = match fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd())) {
        Ok(entries) => entries,
        Err(e) => return Ok(denied(&e)),
    };
    for entry in entries {
        crate::fs::operation_checkpoint(cancel)?;
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if denied(&e) {
                    return Ok(true);
                } else {
                    continue;
                }
            }
        };
        let child = path.join(entry.file_name());
        if (removing && parent_needs_write(&child))
            || readable(&child, removing, cancel, depth + 1)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn request(operation: &Operation) -> Option<Request> {
    let (kind, sources, directory, text, destination_directory) = match operation {
        Operation::Create {
            parent,
            name,
            directory,
        } => (
            if *directory { Kind::Folder } else { Kind::File },
            vec![],
            parent.clone(),
            name.clone(),
            None,
        ),
        Operation::Rename { source, name } => (
            Kind::Rename,
            vec![bytes(source)],
            source.parent()?.into(),
            name.clone(),
            None,
        ),
        Operation::Transfer {
            sources,
            destination,
            cut,
        } => (
            if *cut { Kind::Move } else { Kind::Copy },
            sources.iter().map(|p| bytes(p)).collect(),
            destination.clone(),
            destination.to_string_lossy().into_owned(),
            Some(bytes(destination)),
        ),
        Operation::Duplicate {
            sources,
            destination,
        } => (
            Kind::Duplicate,
            sources.iter().map(|p| bytes(p)).collect(),
            destination.clone(),
            destination.to_string_lossy().into_owned(),
            Some(bytes(destination)),
        ),
        Operation::Trash { sources } => (
            Kind::Trash,
            sources.iter().map(|p| bytes(p)).collect(),
            sources.first()?.parent()?.into(),
            String::new(),
            None,
        ),
        Operation::Permissions(change) => (
            Kind::Permissions,
            vec![bytes(&change.path)],
            change.path.parent()?.into(),
            format!("{:03o}", change.mode),
            None,
        ),
        _ => return None,
    };
    Some(Request {
        kind,
        sources,
        directory: bytes(&directory),
        text,
        destination_directory,
        trash: None,
    })
}

/// Read-only access check before any filesystem change or authentication request.
/// Unsupported operations keep their existing executor and semantics.
pub fn prepare(operation: &Operation, cancel: &Cancellation) -> Result<Option<Plan>> {
    crate::fs::operation_checkpoint(cancel)?;
    if rustix::process::geteuid().is_root() {
        return Ok(None);
    }
    let Some(mut request) = request(operation) else {
        return Ok(None);
    };
    let elevate = match operation {
        Operation::Create { parent, .. } => needs(parent, Access::WRITE_OK | Access::EXEC_OK),
        Operation::Rename { source, .. } => parent_needs_write(source),
        Operation::Trash { sources } => sources.iter().any(|source| parent_needs_write(source)),
        Operation::Permissions(change) => match fs::symlink_metadata(&change.path) {
            Ok(m) => m.uid() != rustix::process::geteuid().as_raw(),
            Err(e) => denied(&e),
        },
        Operation::Transfer {
            sources,
            destination,
            ..
        }
        | Operation::Duplicate {
            sources,
            destination,
        } => {
            let moving = matches!(operation, Operation::Transfer { cut: true, .. });
            let destination_device = fs::metadata(destination).ok().map(|m| m.dev());
            let mut elevate = needs(destination, Access::WRITE_OK | Access::EXEC_OK);
            for source in sources {
                crate::fs::operation_checkpoint(cancel)?;
                if elevate {
                    break;
                }
                let same_device = fs::symlink_metadata(source)
                    .ok()
                    .is_some_and(|m| Some(m.dev()) == destination_device);
                elevate = (moving && parent_needs_write(source))
                    || ((!moving || !same_device) && readable(source, moving, cancel, 0)?);
            }
            elevate
        }
        _ => false,
    };
    if !elevate {
        return Ok(None);
    }
    if request.kind == Kind::Trash {
        request.trash = Some(super::UserTrash::capture()?);
    }
    let plan = Plan::capture(request)?;
    if let Operation::Permissions(change) = operation {
        let parent = plan
            .parents
            .iter()
            .find(|(p, _)| path(p) == change.path.parent().unwrap())
            .and_then(|(_, id)| id.as_ref());
        ensure!(
            plan.sources.first() == Some(&Some(change.before.clone()))
                && parent.is_some_and(|id| (id.device, id.inode) == change.parent),
            "Объект или родительская папка изменились после открытия свойств"
        );
    }
    crate::fs::operation_checkpoint(cancel)?;
    Ok(Some(plan))
}

type Launcher<'a> = dyn FnMut(
        Plan,
        &Cancellation,
        &mut dyn FnMut(&Progress),
        &mut dyn FnMut(&Conflict) -> Decision,
    ) -> Report
    + 'a;

pub fn execute(
    operation: Operation,
    database: &mut crate::journal::Database,
    token: u64,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&Conflict) -> Decision,
) -> Report {
    execute_with_launcher(
        operation,
        database,
        token,
        cancel,
        notify,
        resolve,
        &mut super::run,
    )
}

/// Test injection is explicit and never selected by environment or application settings.
#[doc(hidden)]
pub fn execute_with_launcher(
    operation: Operation,
    database: &mut crate::journal::Database,
    token: u64,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&Conflict) -> Decision,
    launch: &mut Launcher<'_>,
) -> Report {
    let total = operation.total();
    let mut recorder = match database.begin(&operation, token) {
        Ok(r) => r,
        Err(e) => {
            return Report {
                total,
                error: Some(format!(
                    "Не удалось начать журнал; файлы не изменены: {e:#}"
                )),
                ..Default::default()
            };
        }
    };
    notify(&Progress {
        items_total: total,
        ..Default::default()
    });
    let plan = match prepare(&operation, cancel) {
        Ok(plan) => plan,
        Err(e) => {
            let cancelled = crate::fs::operation_cancelled(&e);
            let mut report = Report {
                total,
                cancelled,
                error: (!cancelled).then(|| format!("{e:#}")),
                ..Default::default()
            };
            recorder.finish(&mut report);
            return report;
        }
    };
    let Some(plan) = plan else {
        return crate::fs::operate_with_journal(operation, cancel, notify, resolve, &mut recorder);
    };
    if let Err(e) = recorder.delegated_intent() {
        let mut report = Report {
            total,
            error: Some(format!("Журнал недоступен; файлы не изменены: {e:#}")),
            ..Default::default()
        };
        recorder.finish(&mut report);
        return report;
    }
    let mut report = if cancel.is_cancelled() {
        Report {
            total,
            cancelled: true,
            journal: Some(Default::default()),
            ..Default::default()
        }
    } else {
        notify(&Progress {
            phase: crate::operations::Phase::AwaitingAuthorization,
            items_total: total,
            ..Default::default()
        });
        launch(plan, cancel, notify, resolve)
    };
    recorder.finish_delegated(&mut report);
    report
}

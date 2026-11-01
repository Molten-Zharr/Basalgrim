//! Permanent deletion only of captured trash objects. Never follow links or mount boundaries.
use super::*;
use crate::journal::Identity;
use rustix::fs::{AtFlags, Mode, OFlags, StatxFlags};
use std::{
    ffi::{OsStr, OsString},
    os::fd::AsRawFd,
};

fn open_dir(parent: &File, name: &OsStr) -> Result<File> {
    Ok(File::from(rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}
fn proc_path(parent: &File, name: &OsStr) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name)
}
fn mount(parent: &File, name: &OsStr) -> Result<u64> {
    let stat = rustix::fs::statx(parent, name, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::MNT_ID)?;
    ensure!(
        stat.stx_mask & StatxFlags::MNT_ID.bits() != 0,
        "Не удалось проверить границы файловой системы"
    );
    Ok(stat.stx_mnt_id)
}
fn matches(a: &Identity, b: &Identity) -> bool {
    let mut a = a.clone();
    a.changed = b.changed;
    a == *b
}
struct Node {
    id: Identity,
    children: Vec<(OsString, Node)>,
}
impl Node {
    fn capture(
        parent: &File,
        name: &OsStr,
        expected_mount: u64,
        nodes: &mut usize,
        depth: usize,
        control: &mut Control<'_, '_>,
    ) -> Result<Self> {
        control.checkpoint()?;
        ensure!(
            depth <= 256,
            "Дерево удаления слишком глубокое; удаление не начато"
        );
        *nodes += 1;
        ensure!(
            *nodes <= 100_000,
            "Дерево удаления превышает 100000 объектов; удаление элемента не начато"
        );
        ensure!(
            mount(parent, name)? == expected_mount,
            "Вложенная точка монтирования не удаляется"
        );
        let id = Identity::read(&proc_path(parent, name))?
            .context("Элемент исчез при подготовке удаления")?;
        let mut children = Vec::new();
        if id.directory() {
            let dir = open_dir(parent, name)?;
            ensure!(
                matches(
                    &id,
                    &Identity::read(&proc_path(&dir, OsStr::new(".")))?.context("Папка исчезла")?
                ),
                "Папка заменена"
            );
            for child in fs::read_dir(proc_path(&dir, OsStr::new(".")))? {
                let name = child?.file_name();
                let node = Self::capture(&dir, &name, expected_mount, nodes, depth + 1, control)?;
                children.push((name, node));
            }
        }
        Ok(Self { id, children })
    }
    fn remove(
        &self,
        parent: &File,
        name: &OsStr,
        expected_mount: u64,
        control: &mut Control<'_, '_>,
    ) -> Result<()> {
        ensure!(
            mount(parent, name)? == expected_mount,
            "Точка монтирования изменилась перед удалением"
        );
        let current =
            Identity::read(&proc_path(parent, name))?.context("Элемент исчез перед удалением")?;
        ensure!(
            matches(&self.id, &current),
            "Элемент изменился перед удалением"
        );
        if self.id.directory() {
            let dir = open_dir(parent, name)?;
            let opened =
                Identity::read(&proc_path(&dir, OsStr::new(".")))?.context("Папка исчезла")?;
            ensure!(matches(&self.id, &opened), "Папка заменена перед удалением");
            for (name, child) in &self.children {
                child.remove(&dir, name, expected_mount, control)?;
            }
            // New children are never guessed/deleted: rmdir fails and the remainder is restored.
            rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR)?;
        } else {
            rustix::fs::unlinkat(parent, name, AtFlags::empty())?;
        }
        control.progress.entries_done = control.progress.entries_done.saturating_add(1);
        control.emit(); // Cancellation is deferred once irreversible removal has started.
        Ok(())
    }
}

pub(super) fn run(
    target: &crate::trash::Deletion,
    control: &mut Control<'_, '_>,
) -> (bool, Result<()>) {
    let mut removed = false;
    let result = run_inner(target, control, &mut removed);
    (removed, result)
}
fn run_inner(
    target: &crate::trash::Deletion,
    control: &mut Control<'_, '_>,
    removed: &mut bool,
) -> Result<()> {
    target.validate(control.cancellation)?;
    let holding = tempfile::Builder::new()
        .prefix(".basalgrim-delete-")
        .tempdir_in(&target.root.directory)?
        .keep();
    let directory = File::from(rustix::fs::openat(
        rustix::fs::CWD,
        &holding,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let expected_mount = mount(&directory, OsStr::new("."))?;
    let mut claimed = Vec::<(PathBuf, PathBuf, Identity)>::new();
    let outcome = (|| -> Result<()> {
        for (i, (source, expected)) in target.objects.iter().enumerate() {
            let parked = holding.join(i.to_string());
            let step = control.before_step("delete_claim", Some(source), Some(&parked))?;
            control.checkpoint()?;
            // Recheck every object after a possibly blocking journal commit. Earlier claims
            // are allowed to be absent at their original names and are checked separately.
            for (path, id) in target.objects.iter().skip(i) {
                ensure!(
                    Identity::read(path)?.as_ref() == Some(id),
                    "Элемент корзины изменился перед удалением"
                );
            }
            target.validate_roots()?;
            let result = rename_new(source, &parked);
            if result.is_ok() {
                claimed.push((source.clone(), parked.clone(), expected.clone()));
            }
            control.after_step(step, "delete_claim", Some(&parked), &result);
            result?;
            if let Some(journal) = &control.journal {
                journal.check()?;
            }
        }
        let mut nodes = 0;
        let mut prepared = Vec::new();
        for (_, parked, expected) in &claimed {
            let name = parked.file_name().unwrap();
            let node = Node::capture(&directory, name, expected_mount, &mut nodes, 0, control)?;
            ensure!(matches(expected, &node.id), "Элемент подменён при захвате");
            prepared.push(node);
        }
        let step = control.before_step("permanent_delete", Some(&holding), None)?;
        control.checkpoint()?;
        target.validate_roots()?;
        for (_, parked, expected) in &claimed {
            ensure!(
                Identity::read(parked)?.is_some_and(|id| matches(expected, &id)),
                "Захваченный элемент изменился"
            );
        }
        if let Some(journal) = &mut control.journal {
            journal.destructive_started();
        }
        let result = (|| {
            for ((_, parked, _), node) in claimed.iter().zip(&prepared) {
                node.remove(
                    &directory,
                    parked.file_name().unwrap(),
                    expected_mount,
                    control,
                )?;
            }
            *removed = true;
            directory.sync_all()?;
            Ok(())
        })();
        control.after_step(step, "permanent_delete", None, &result);
        result
    })();
    if let Err(error) = outcome {
        let mut remaining = Vec::new();
        for (source, parked, _) in claimed.iter().rev() {
            if Identity::read(parked)?.is_some()
                && let Err(recovery) = control.rename("delete_restore", parked, source, true)
            {
                remaining.push(format!("{}: {recovery:#}", parked.display()));
            }
        }
        let _ = fs::remove_dir(&holding); // Empty directories only; claimed data is never auto-removed.
        if !remaining.is_empty() {
            return Err(error.context(CleanupFailure(format!(
                "Остатки сохранены: {}",
                remaining.join("; ")
            ))));
        }
        return Err(error.context(if *removed {
            "Данные удалены; не удалось завершить запись результата"
        } else {
            "Удаление прервано; сохранившиеся данные возвращены в корзину"
        }));
    }
    let _ = fs::remove_dir(&holding);
    Ok(())
}

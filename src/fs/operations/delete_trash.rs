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

/// A retained object pinned through a no-follow directory descriptor. Preparation
/// is cancellable; after removal starts the current object is finished first.
pub(crate) struct RetainedRemoval {
    directory: File,
    container_parent: File,
    container_name: OsString,
    name: OsString,
    node: Node,
    mount: u64,
}
impl RetainedRemoval {
    pub(crate) fn prepare(
        path: &Path,
        expected: &crate::journal::snapshot::Snapshot,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        use std::path::Component;
        let mut directory = File::from(rustix::fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        ensure!(path.is_absolute(), "Требуется абсолютный путь хранилища");
        for component in path.parent().context("Нет папки хранилища")?.components()
        {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => directory = open_dir(&directory, name)?,
                _ => bail!("Недопустимый путь хранилища"),
            }
        }
        use std::os::unix::fs::MetadataExt;
        let metadata = directory.metadata()?;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o022 == 0,
            "Хранилище должно быть личной папкой владельца"
        );
        let name = path
            .file_name()
            .context("Нет имени объекта")?
            .to_os_string();
        let container_parent = open_dir(&directory, OsStr::new(".."))?;
        let container_name = path
            .parent()
            .and_then(Path::file_name)
            .context("Нет имени папки хранения")?
            .to_os_string();
        let mount = mount(&directory, OsStr::new("."))?;
        let mut control = Control {
            cancellation,
            progress: Progress::default(),
            notify: &mut |_| {},
            replacement: None,
            recoveries: Vec::new(),
            journal: None,
            restore_guard: None,
            rename_destination: None,
        };
        let node = Node::capture(&directory, &name, mount, &mut 0, 0, &mut control)?;
        let actual =
            crate::journal::snapshot::Snapshot::read(&proc_path(&directory, &name), cancellation)?;
        ensure!(
            expected.unchanged(&actual),
            "Сохранённые данные изменились: {}",
            path.display()
        );
        Ok(Self {
            directory,
            container_parent,
            container_name,
            name,
            node,
            mount,
        })
    }
    pub(crate) fn remove(
        self,
        progress: &mut Progress,
        notify: &mut dyn FnMut(&Progress),
    ) -> (bool, Result<()>) {
        let mut control = Control {
            cancellation: &Cancellation::default(),
            progress: progress.clone(),
            notify,
            replacement: None,
            recoveries: Vec::new(),
            journal: None,
            restore_guard: None,
            rename_destination: None,
        };
        let result = self.node.remove_guarded(
            &self.directory,
            &self.name,
            self.mount,
            &mut control,
            Some(&mut std::collections::BTreeMap::new()),
        );
        *progress = control.progress;
        let removed = result.is_ok();
        let result = result.and_then(|_| self.directory.sync_all().map_err(Into::into));
        if removed {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(metadata), Ok(Some(current))) = (
                self.directory.metadata(),
                Identity::read(&proc_path(&self.container_parent, &self.container_name)),
            ) && current.device == metadata.dev()
                && current.inode == metadata.ino()
            {
                // Only our now-empty container is eligible. New arrivals and
                // renamed/replaced directories are never recursively removed.
                let _ = rustix::fs::unlinkat(
                    &self.container_parent,
                    &self.container_name,
                    AtFlags::REMOVEDIR,
                );
            }
        }
        (removed, result)
    }
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
        self.remove_guarded(parent, name, expected_mount, control, None)
    }
    fn remove_guarded(
        &self,
        parent: &File,
        name: &OsStr,
        expected_mount: u64,
        control: &mut Control<'_, '_>,
        mut own_changes: Option<&mut std::collections::BTreeMap<(u64, u64), [i64; 2]>>,
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
        let expected_change = own_changes
            .as_ref()
            .and_then(|changes| changes.get(&(self.id.device, self.id.inode)))
            .copied()
            .unwrap_or(self.id.changed);
        ensure!(
            own_changes.is_none() || current.changed == expected_change,
            "Данные изменились после проверки содержимого"
        );
        if self.id.directory() {
            let dir = open_dir(parent, name)?;
            let opened =
                Identity::read(&proc_path(&dir, OsStr::new(".")))?.context("Папка исчезла")?;
            ensure!(matches(&self.id, &opened), "Папка заменена перед удалением");
            ensure!(
                own_changes.is_none() || opened.changed == expected_change,
                "Папка изменилась после проверки"
            );
            for (name, child) in &self.children {
                child.remove_guarded(
                    &dir,
                    name,
                    expected_mount,
                    control,
                    own_changes.as_deref_mut(),
                )?;
            }
            // New children are never guessed/deleted: rmdir fails and the remainder is restored.
            rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR)?;
        } else {
            let pinned = if own_changes.is_some() {
                Some(File::from(rustix::fs::openat(
                    parent,
                    name,
                    OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?))
            } else {
                None
            };
            if let Some(file) = &pinned {
                use std::os::unix::fs::MetadataExt;
                let meta = file.metadata()?;
                ensure!(
                    meta.dev() == current.device
                        && meta.ino() == current.inode
                        && [meta.ctime(), meta.ctime_nsec()] == expected_change,
                    "Файл заменён перед удалением"
                );
            }
            rustix::fs::unlinkat(parent, name, AtFlags::empty())?;
            if let (Some(changes), Some(file)) = (&mut own_changes, pinned) {
                use std::os::unix::fs::MetadataExt;
                let meta = file.metadata()?;
                // Unlinking our own earlier hard link legitimately changes ctime.
                // Record that exact change before notifying progress observers.
                changes.insert(
                    (current.device, current.inode),
                    [meta.ctime(), meta.ctime_nsec()],
                );
            }
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

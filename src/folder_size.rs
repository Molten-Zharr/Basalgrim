//! Cancellable no-follow directory measurement with a disk-backed work queue.
use crate::operations::Cancellation;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::OsString,
    os::{
        fd::OwnedFd,
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

const CACHE_AGE: Duration = Duration::from_secs(30);
const CACHE_ENTRIES: usize = 64;
const RECOUNT_QUIET: Duration = Duration::from_secs(1);
const FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Clone, Debug, Default)]
pub struct Measurement {
    pub bytes: u64,
    pub allocated: u64,
    pub files: u64,
    pub folders: u64,
    pub links: u64,
    pub other: u64,
    pub duplicates: u64,
    pub skipped: u64,
    pub issues: Vec<String>,
    pub failure: Option<String>,
    pub done: bool,
    pub stopped: bool,
    pub stale: bool,
    pub cached: bool,
    pub measured: Option<SystemTime>,
    stamp: Option<Stamp>,
}
impl Measurement {
    pub fn complete(&self) -> bool {
        self.done && !self.stopped && !self.stale && self.failure.is_none() && self.skipped == 0
    }
    pub fn status(&self) -> &'static str {
        if self.stopped {
            "Остановлено · частично"
        } else if self.failure.is_some() {
            "Ошибка · частично"
        } else if !self.done {
            "Подсчёт…"
        } else if self.stale || self.skipped > 0 {
            "С пропусками / изменениями"
        } else if self.cached {
            "Кэш · предыдущий подсчёт"
        } else {
            "Подсчёт завершён"
        }
    }
    fn issue(&mut self, path: &Path, error: impl std::fmt::Display) {
        self.skipped += 1;
        if self.issues.len() < 3 {
            self.issues
                .push(crate::fs::clean(&format!("{}: {error}", path.display())));
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    mtime: i64,
    mnano: u64,
    ctime: i64,
    cnano: u64,
}
impl From<&Stat> for Stamp {
    fn from(s: &Stat) -> Self {
        Self {
            dev: s.st_dev,
            ino: s.st_ino,
            mtime: s.st_mtime,
            mnano: s.st_mtime_nsec,
            ctime: s.st_ctime,
            cnano: s.st_ctime_nsec,
        }
    }
}
struct Walking {
    dir: Dir,
    path: PathBuf,
    stamp: Stamp,
}
struct Scan {
    db: Connection,
    root: OwnedFd,
    current: Option<Walking>,
    result: Measurement,
    cancel: Cancellation,
}
type QueuedDirectory = (i64, Vec<u8>, Vec<u8>, Vec<u8>);

// Open one component at a time: no directory symlinks, no recursion stack or
// depth/PATH_MAX cap, and only a bounded number of live descriptors.
fn open_relative(root: &OwnedFd, path: &Path, cancel: &Cancellation) -> Result<OwnedFd> {
    let mut fd = fs::openat(root, ".", FLAGS, Mode::empty())?;
    for part in path.components() {
        ensure!(!cancel.is_cancelled(), "Подсчёт остановлен");
        let Component::Normal(part) = part else {
            anyhow::bail!("Некорректный путь обхода");
        };
        fd = fs::openat(&fd, part, FLAGS, Mode::empty())?;
    }
    Ok(fd)
}
impl Scan {
    fn new(root: OwnedFd, cancel: Cancellation) -> Result<Self> {
        let stamp = fs::fstat(&root)?;
        // Reconstructible, unnamed temporary SQLite storage; no persistent cache
        // files or unbounded in-memory queues/sets, even for millions of entries.
        let db = Connection::open("")?;
        db.execute_batch(
            "PRAGMA synchronous=OFF; PRAGMA cache_size=-1024; PRAGMA temp_store=FILE;
            CREATE TABLE queue(id INTEGER PRIMARY KEY, path BLOB, dev BLOB, ino BLOB);
            CREATE TABLE seen(dev BLOB, ino BLOB, PRIMARY KEY(dev,ino)) WITHOUT ROWID;",
        )?;
        let mut scan = Self {
            db,
            root,
            current: None,
            result: Measurement::default(),
            cancel,
        };
        scan.result.stamp = Some(Stamp::from(&stamp));
        scan.account(&PathBuf::new(), &stamp, true)?;
        Ok(scan)
    }
    fn account(&mut self, path: &Path, stat: &Stat, root: bool) -> Result<()> {
        let kind = FileType::from_raw_mode(stat.st_mode);
        if !root {
            match kind {
                FileType::RegularFile => self.result.files += 1,
                FileType::Directory => self.result.folders += 1,
                FileType::Symlink => self.result.links += 1,
                _ => self.result.other += 1,
            }
        }
        let dev = stat.st_dev.to_le_bytes();
        let ino = stat.st_ino.to_le_bytes();
        if self
            .db
            .prepare_cached("INSERT OR IGNORE INTO seen VALUES(?1,?2)")?
            .execute(params![dev.as_slice(), ino.as_slice()])?
            == 0
        {
            self.result.duplicates += 1;
            return Ok(());
        }
        if kind == FileType::RegularFile {
            self.result.bytes = self
                .result
                .bytes
                .checked_add(u64::try_from(stat.st_size)?)
                .ok_or_else(|| anyhow::anyhow!("Размер превышает u64"))?;
        }
        self.result.allocated = self
            .result
            .allocated
            .checked_add(
                u64::try_from(stat.st_blocks)?
                    .checked_mul(512)
                    .ok_or_else(|| anyhow::anyhow!("Размер превышает u64"))?,
            )
            .ok_or_else(|| anyhow::anyhow!("Размер превышает u64"))?;
        if kind == FileType::Directory {
            self.db
                .prepare_cached("INSERT INTO queue(path,dev,ino) VALUES(?1,?2,?3)")?
                .execute(params![
                    path.as_os_str().as_bytes(),
                    dev.as_slice(),
                    ino.as_slice()
                ])?;
        }
        Ok(())
    }
    fn step(&mut self) -> Result<()> {
        let previous = self.result.clone();
        self.db.execute_batch("BEGIN")?;
        let outcome = self.batch();
        if let Err(error) = outcome.and_then(|()| Ok(self.db.execute_batch("COMMIT")?)) {
            let _ = self.db.execute_batch("ROLLBACK");
            self.result = previous;
            return Err(error);
        }
        Ok(())
    }
    fn batch(&mut self) -> Result<()> {
        let started = Instant::now();
        for _ in 0..512 {
            if self.cancel.is_cancelled() {
                self.result.stopped = true;
                self.result.done = true;
                break;
            }
            if started.elapsed() >= Duration::from_millis(20) {
                break;
            }
            if self.current.is_none() {
                let next: Option<QueuedDirectory> = self
                    .db
                    .query_row(
                        "SELECT id,path,dev,ino FROM queue ORDER BY id LIMIT 1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let Some((id, bytes, dev, ino)) = next else {
                    self.result.done = true;
                    break;
                };
                self.db.execute("DELETE FROM queue WHERE id=?1", [id])?;
                let path = PathBuf::from(OsString::from_vec(bytes));
                let opened = (|| {
                    let fd = open_relative(&self.root, &path, &self.cancel)?;
                    let stamp = fs::fstat(&fd)?;
                    ensure!(
                        stamp.st_dev.to_le_bytes().as_slice() == dev
                            && stamp.st_ino.to_le_bytes().as_slice() == ino,
                        "Папка изменилась во время обхода"
                    );
                    Ok(Walking {
                        dir: Dir::new(fd)?,
                        path: path.clone(),
                        stamp: Stamp::from(&stamp),
                    })
                })();
                match opened {
                    Ok(dir) => self.current = Some(dir),
                    Err(error) => {
                        self.result.issue(&path, error);
                        continue;
                    }
                }
            }
            let current = self.current.as_mut().unwrap();
            let Some(entry) = current.dir.next() else {
                let changed = fs::fstat(current.dir.fd()?)
                    .map(|s| Stamp::from(&s) != current.stamp)
                    .unwrap_or(true);
                if changed {
                    self.result
                        .issue(&current.path, "Содержимое менялось во время обхода");
                }
                self.current = None;
                continue;
            };
            let entry = match entry {
                Ok(e) => e,
                Err(error) => {
                    self.result.issue(&current.path, error);
                    self.current = None;
                    continue;
                }
            };
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let path = current
                .path
                .join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            match fs::statat(current.dir.fd()?, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => self.account(&path, &stat, false)?,
                Err(error) => self.result.issue(&path, error),
            }
        }
        Ok(())
    }
}

pub fn measure(
    path: &Path,
    cancel: &Cancellation,
    mut progress: impl FnMut(&Measurement),
) -> Measurement {
    if cancel.is_cancelled() {
        return Measurement {
            done: true,
            stopped: true,
            ..Default::default()
        };
    }
    let outcome = (|| {
        let root = fs::open(path, FLAGS, Mode::empty())?;
        Scan::new(root, cancel.clone())
    })();
    let mut scan = match outcome {
        Ok(scan) => scan,
        Err(e) => {
            return Measurement {
                done: true,
                failure: Some(format!("{e:#}")),
                ..Default::default()
            };
        }
    };
    let mut last = Instant::now();
    while !scan.result.done {
        if let Err(e) = scan.step() {
            scan.result.failure = Some(format!("{e:#}"));
            scan.result.done = true;
        }
        if !scan.result.done && last.elapsed() >= Duration::from_millis(100) {
            progress(&scan.result);
            last = Instant::now();
        }
    }
    scan.result.stopped |= cancel.is_cancelled();
    scan.result.measured = Some(SystemTime::now());
    // A replacement root must not seed a cache entry for the old inode.
    if fs::statat(rustix::fs::CWD, path, AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .map(|s| Stamp::from(&s))
        != scan.result.stamp
    {
        scan.result.stale = true;
    }
    scan.result
}

#[derive(Clone)]
struct Cached {
    path: PathBuf,
    result: Measurement,
    at: Instant,
}
struct Request {
    id: u64,
    path: PathBuf,
    cancel: Cancellation,
    cached: Option<Cached>,
}
type Inbox = Arc<Mutex<Option<Request>>>;
type Output = Arc<Mutex<Option<(u64, Measurement)>>>;

#[derive(Clone, Copy, Default)]
enum RowSize {
    #[default]
    Pending,
    Bytes(u64),
    Unavailable,
    Changed {
        after: Instant,
        previous: Option<u64>,
    },
}
impl RowSize {
    fn bytes(self) -> Option<u64> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Changed { previous, .. } => previous,
            _ => None,
        }
    }
    fn ready(self, now: Instant) -> bool {
        matches!(self, Self::Pending) || matches!(self, Self::Changed { after, .. } if now >= after)
    }
}

pub struct FolderSizes {
    inbox: Inbox,
    output: Output,
    wake: mpsc::SyncSender<()>,
    cancel: Cancellation,
    pub id: u64,
    pub path: Option<PathBuf>,
    pub result: Measurement,
    cache: VecDeque<Cached>,
    invalidated: bool,
    // Only the currently displayed rows retain their labels. Re-entering the
    // viewport goes through the same validated, expiring cache as a manual scan.
    visible: BTreeMap<PathBuf, RowSize>,
    automatic: Option<PathBuf>,
}
impl Default for FolderSizes {
    fn default() -> Self {
        let inbox: Inbox = Default::default();
        let output: Output = Default::default();
        let (wake, rx) = mpsc::sync_channel(1);
        let input = inbox.clone();
        let out = output.clone();
        thread::spawn(move || {
            while rx.recv().is_ok() {
                let Some(request) = input.lock().unwrap().take() else {
                    continue;
                };
                if request.cancel.is_cancelled() {
                    continue;
                }
                let cached = request.cached.filter(|c| {
                    c.at.elapsed() < CACHE_AGE
                        && fs::statat(fs::CWD, &request.path, AtFlags::SYMLINK_NOFOLLOW)
                            .ok()
                            .map(|s| Stamp::from(&s))
                            == c.result.stamp
                });
                let result = if let Some(c) = cached {
                    Measurement {
                        cached: true,
                        ..c.result
                    }
                } else {
                    measure(&request.path, &request.cancel, |r| {
                        *out.lock().unwrap() = Some((request.id, r.clone()));
                    })
                };
                *out.lock().unwrap() = Some((request.id, result));
            }
        });
        Self {
            inbox,
            output,
            wake,
            cancel: Default::default(),
            id: 0,
            path: None,
            result: Measurement {
                done: true,
                ..Default::default()
            },
            cache: VecDeque::new(),
            invalidated: false,
            visible: BTreeMap::new(),
            automatic: None,
        }
    }
}
impl Drop for FolderSizes {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl FolderSizes {
    pub fn start(&mut self, path: PathBuf, force: bool) {
        self.automatic = None;
        self.path = Some(path.clone());
        self.result = Measurement::default();
        self.invalidated = false;
        self.submit(path, force);
    }
    fn submit(&mut self, path: PathBuf, force: bool) {
        self.cancel.cancel();
        self.cancel = Default::default();
        self.id += 1;
        let cached = (!force)
            .then(|| {
                self.cache
                    .iter()
                    .find(|c| c.path == path && c.at.elapsed() < CACHE_AGE)
                    .cloned()
            })
            .flatten();
        self.cache
            .retain(|c| (!force || c.path != path) && c.at.elapsed() < CACHE_AGE);
        *self.inbox.lock().unwrap() = Some(Request {
            id: self.id,
            path,
            cancel: self.cancel.clone(),
            cached,
        });
        let _ = self.wake.try_send(());
    }
    pub fn stop(&mut self) {
        if !self.result.done {
            self.cancel.cancel();
            self.result.done = true;
            self.result.stopped = true;
        }
    }
    pub fn poll(&mut self) {
        if let Some((id, mut result)) = self.output.lock().unwrap().take()
            && id == self.id
        {
            let automatic = self.automatic.is_some();
            if automatic && !result.done {
                return;
            }
            if self.cancel.is_cancelled() {
                result.done = true;
                result.stopped = true;
            }
            result.stale |= !automatic && self.invalidated;
            let Some(path) = self.automatic.as_ref().or(self.path.as_ref()) else {
                return;
            };
            if result.done && !result.complete() {
                self.cache.retain(|c| c.path != *path);
            }
            if result.complete() && result.measured.is_some() && !result.cached {
                self.cache.retain(|c| c.path != *path);
                self.cache.push_back(Cached {
                    path: path.clone(),
                    result: result.clone(),
                    at: Instant::now(),
                });
                while self.cache.len() > CACHE_ENTRIES {
                    self.cache.pop_front();
                }
            }
            if result.done
                && let Some(row) = self.visible.get_mut(path)
            {
                *row = if result.complete() {
                    RowSize::Bytes(result.bytes)
                } else {
                    RowSize::Unavailable
                };
            }
            if automatic {
                self.automatic = None;
            } else {
                self.result = result;
            }
        }
    }
    fn cancel_automatic(&mut self) {
        if self.automatic.take().is_some() {
            self.cancel.cancel();
            self.id += 1;
        }
    }
    pub(crate) fn update_visible(&mut self, paths: BTreeSet<PathBuf>) {
        self.visible.retain(|path, _| paths.contains(path));
        if self.automatic.as_ref().is_some_and(|p| !paths.contains(p)) {
            self.cancel_automatic();
        }
        for path in paths {
            self.visible.entry(path).or_default();
        }
        let now = Instant::now();
        if self.result.done
            && self.automatic.is_none()
            && let Some((path, _)) = self.visible.iter().find(|(_, s)| s.ready(now))
        {
            let path = path.clone();
            self.automatic = Some(path.clone());
            self.submit(path, false);
        }
    }
    pub(crate) fn visible_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.visible.keys()
    }
    pub(crate) fn row_label(&self, path: &Path) -> String {
        let row = self.visible.get(path).copied().unwrap_or_default();
        if let Some(bytes) = row.bytes() {
            format!("≈{}", crate::fs::size(bytes))
        } else if matches!(row, RowSize::Unavailable) {
            "?".into()
        } else {
            "…".into()
        }
    }
    pub fn cached(&self, path: &Path) -> Option<&Measurement> {
        self.cache
            .iter()
            .find(|c| c.path == path && c.at.elapsed() < CACHE_AGE)
            .map(|c| &c.result)
    }
    pub fn invalidate(&mut self, path: &Path) {
        self.invalidate_matching(|p| p.starts_with(path) || path.starts_with(p), None);
    }
    pub(crate) fn invalidate_ancestors(&mut self, changed: &Path) {
        // Keep the last visible estimate during a download or burst of writes.
        // Other pending folders can be measured while this one settles.
        self.invalidate_matching(
            |p| changed.starts_with(p),
            Some(Instant::now() + RECOUNT_QUIET),
        );
    }
    pub(crate) fn listing_changed(
        &mut self,
        dir: &Path,
        before: &[crate::fs::Entry],
        after: &[crate::fs::Entry],
    ) {
        // A changed file in this directory affects its total, but not the
        // contents of unrelated child folders. Compare only tracked branches.
        let mut branches = BTreeMap::new();
        for path in self
            .visible
            .keys()
            .chain(self.cache.iter().map(|c| &c.path))
            .chain(&self.path)
        {
            if let Ok(relative) = path.strip_prefix(dir)
                && let Some(child) = relative.iter().next()
            {
                branches.insert(dir.join(child), [None; 2]);
            }
        }
        self.invalidate_ancestors(dir);
        if branches.is_empty() {
            return;
        }
        for (index, entries) in [before, after].into_iter().enumerate() {
            for e in entries {
                if let Some(stamps) = branches.get_mut(&e.path) {
                    stamps[index] = Some((
                        e.directory,
                        e.symlink,
                        e.modified,
                        e.size,
                        e.details.created,
                        e.details.mode,
                    ));
                }
            }
        }
        for (child, [old, new]) in branches {
            if old != new {
                self.invalidate(&child);
            }
        }
    }
    fn invalidate_matching(&mut self, affected: impl Fn(&Path) -> bool, after: Option<Instant>) {
        self.cache.retain(|c| !affected(&c.path));
        if self.automatic.as_deref().is_some_and(&affected) {
            self.cancel_automatic();
        }
        for (path, row) in &mut self.visible {
            if affected(path) {
                *row = after.map_or(RowSize::Pending, |after| RowSize::Changed {
                    after,
                    previous: row.bytes(),
                });
            }
        }
        if self.path.as_deref().is_some_and(affected) {
            self.invalidated = true;
            self.result.stale = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as disk;

    #[test]
    fn a_busy_folder_keeps_its_estimate_and_yields_to_other_visible_folders() {
        let root = tempfile::tempdir().unwrap();
        let busy = root.path().join("a");
        let next = root.path().join("b");
        let mut sizes = FolderSizes::default();
        sizes.visible.insert(busy.clone(), RowSize::Bytes(50));
        sizes.invalidate_ancestors(&busy);
        assert_eq!(sizes.row_label(&busy), "≈50 B");
        let row = sizes.visible[&busy];
        let RowSize::Changed { after, .. } = row else {
            panic!("recount was not deferred")
        };
        assert!(!row.ready(after - Duration::from_millis(1)));
        assert!(row.ready(after));
        sizes.update_visible(BTreeSet::from([busy, next.clone()]));
        assert_eq!(sizes.automatic.as_deref(), Some(next.as_path()));
    }

    #[test]
    fn obsolete_auto_results_cannot_replace_manual_or_visible_sizes() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        disk::create_dir(&folder).unwrap();
        disk::write(folder.join("sample"), b"sample").unwrap();
        let missing = root.path().join("missing");
        let mut sizes = FolderSizes::default();
        sizes.update_visible(BTreeSet::from([folder.clone()]));
        let first = sizes.id;
        sizes.start(missing.clone(), false);
        // Rendering another viewport must leave the explicit request in control.
        let second = sizes.id;
        sizes.update_visible(BTreeSet::from([missing.clone()]));
        assert_eq!(sizes.id, second);
        let end = Instant::now() + Duration::from_secs(5);
        while !sizes.result.done {
            sizes.poll();
            assert!(Instant::now() < end);
            thread::yield_now();
        }
        assert!(sizes.result.failure.is_some());
        assert_eq!(sizes.row_label(&missing), "?");
        sizes.update_visible(BTreeSet::from([missing.clone()]));
        assert_eq!(sizes.id, second, "failed rows must not spin");
        assert!(sizes.cached(&folder).is_none());
        sizes.update_visible(BTreeSet::from([folder.clone()]));
        let cancelled = sizes.id;
        sizes.update_visible(BTreeSet::new());
        for id in [first, cancelled] {
            *sizes.output.lock().unwrap() = Some((
                id,
                Measurement {
                    bytes: 999,
                    done: true,
                    measured: Some(SystemTime::now()),
                    ..Default::default()
                },
            ));
            sizes.poll();
            assert!(sizes.cached(&folder).is_none());
            assert!(sizes.result.failure.is_some());
        }
    }

    #[test]
    fn cancelled_steps_and_storage_failures_keep_only_committed_totals() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..1200 {
            disk::write(root.path().join(i.to_string()), [0; 10]).unwrap();
        }
        let cancel = Cancellation::default();
        let mut scan = Scan::new(
            fs::open(root.path(), FLAGS, Mode::empty()).unwrap(),
            cancel.clone(),
        )
        .unwrap();
        while scan.result.files == 0 {
            scan.step().unwrap();
            assert!(!scan.result.done);
        }
        assert!(scan.result.files > 0 && scan.result.files < 1200);
        let bytes = scan.result.bytes;
        cancel.cancel();
        scan.step().unwrap();
        assert!(scan.result.stopped);
        assert_eq!(scan.result.bytes, bytes);
        let mut scan = Scan::new(
            fs::open(root.path(), FLAGS, Mode::empty()).unwrap(),
            Cancellation::default(),
        )
        .unwrap();
        scan.db.execute_batch("CREATE TRIGGER fail BEFORE INSERT ON seen WHEN (SELECT COUNT(*) FROM seen)>3 BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;").unwrap();
        // The 20 ms work budget may expire before reaching the trigger.
        // Only the failing transaction rolls back; earlier batches stay committed.
        loop {
            let prior = (scan.result.bytes, scan.result.files);
            if let Err(error) = scan.step() {
                assert!(error.to_string().contains("simulated failure"));
                assert_eq!((scan.result.bytes, scan.result.files), prior);
                break;
            }
            assert!(!scan.result.done);
        }
    }

    #[test]
    fn queued_directory_replaced_by_symlink_never_escapes_the_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        disk::write(outside.path().join("secret"), [0; 12345]).unwrap();
        for i in 0..600 {
            disk::create_dir(root.path().join(format!("d{i}"))).unwrap();
        }
        let mut scan = Scan::new(
            fs::open(root.path(), FLAGS, Mode::empty()).unwrap(),
            Cancellation::default(),
        )
        .unwrap();
        // A timed step may yield before enqueueing the first child on a busy
        // machine. Wait for the race's actual precondition, not one time slice.
        let bytes: Vec<u8> = loop {
            scan.step().unwrap();
            if let Some(path) = scan
                .db
                .query_row("SELECT path FROM queue ORDER BY id LIMIT 1", [], |r| {
                    r.get(0)
                })
                .optional()
                .unwrap()
            {
                break path;
            }
            assert!(
                !scan.result.done,
                "scan finished before a directory was queued"
            );
        };
        let path = root.path().join(OsString::from_vec(bytes));
        disk::remove_dir(&path).unwrap();
        std::os::unix::fs::symlink(outside.path(), &path).unwrap();
        while !scan.result.done {
            scan.step().unwrap();
        }
        assert_eq!(scan.result.bytes, 0);
        assert!(scan.result.skipped > 0);
        assert!(!scan.result.complete());
    }

    #[test]
    fn cache_capacity_expiry_and_parent_invalidation_are_bounded() {
        let mut sizes = FolderSizes::default();
        for i in 0..CACHE_ENTRIES + 5 {
            sizes.id += 1;
            sizes.path = Some(PathBuf::from(format!("/cache/{i}")));
            *sizes.output.lock().unwrap() = Some((
                sizes.id,
                Measurement {
                    done: true,
                    measured: Some(SystemTime::now()),
                    ..Default::default()
                },
            ));
            sizes.poll();
        }
        assert_eq!(sizes.cache.len(), CACHE_ENTRIES);
        assert!(sizes.cached(Path::new("/cache/0")).is_none());
        let last = sizes.cache.back_mut().unwrap();
        let path = last.path.clone();
        last.at = Instant::now() - CACHE_AGE - Duration::from_secs(1);
        assert!(sizes.cached(&path).is_none());
        sizes.invalidate(Path::new("/cache"));
        assert!(sizes.cache.is_empty());
    }

    #[test]
    fn paths_longer_than_path_max_are_walked_by_directory_descriptors() {
        let root = tempfile::tempdir().unwrap();
        let mut fd = fs::open(root.path(), FLAGS, Mode::empty()).unwrap();
        for _ in 0..96 {
            let name = "deep".repeat(16);
            fs::mkdirat(&fd, &name, Mode::RWXU).unwrap();
            fd = fs::openat(&fd, &name, FLAGS, Mode::empty()).unwrap();
        }
        let file = fs::openat(
            &fd,
            "last",
            OFlags::CREATE | OFlags::WRONLY,
            Mode::RUSR | Mode::WUSR,
        )
        .unwrap();
        rustix::io::write(&file, b"deep").unwrap();
        let result = measure(root.path(), &Cancellation::default(), |_| {});
        assert!(result.complete(), "{result:?}");
        assert_eq!(result.bytes, 4);
        assert_eq!(result.folders, 96);
    }
}

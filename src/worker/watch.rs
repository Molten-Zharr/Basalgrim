//! Coalesced directory notifications. No filesystem calls run on the UI thread.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

const QUIET: Duration = Duration::from_millis(150);
const MAX_DELAY: Duration = Duration::from_millis(600);
const RETRY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Pending(BTreeMap<PathBuf, (Instant, Instant)>);

impl Pending {
    fn mark(&mut self, path: PathBuf, now: Instant) {
        self.0
            .entry(path)
            .and_modify(|(_, last)| *last = now)
            .or_insert((now, now));
    }

    fn ready(&mut self, now: Instant) -> BTreeSet<PathBuf> {
        let ready = self
            .0
            .iter()
            .filter(|(_, (first, last))| {
                now.duration_since(*last) >= QUIET || now.duration_since(*first) >= MAX_DELAY
            })
            .map(|(path, _)| path.clone())
            .collect::<BTreeSet<_>>();
        self.0.retain(|path, _| !ready.contains(path));
        ready
    }
}

#[derive(Default)]
pub struct Changes {
    pub paths: BTreeSet<PathBuf>,
    pub error: Option<String>,
}

pub struct Watcher {
    paths: BTreeSet<PathBuf>,
    desired: Arc<Mutex<Option<BTreeSet<PathBuf>>>>,
    wake: SyncSender<()>,
    changes: Arc<Mutex<Changes>>,
}

impl Watcher {
    pub fn new() -> Self {
        let desired = Arc::new(Mutex::new(None::<BTreeSet<PathBuf>>));
        let requests = Arc::clone(&desired);
        let changes = Arc::new(Mutex::new(Changes::default()));
        let output = Arc::clone(&changes);
        let (wake, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut native = Native::default();
            let mut paths = BTreeSet::new();
            let mut pending = Pending::default();
            let mut retry = Instant::now();
            let mut last_error = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(40)) {
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let now = Instant::now();
                if let Some(latest) = requests.lock().unwrap().take() {
                    paths = latest;
                    pending.0.retain(|path, _| paths.contains(path));
                    retry = now;
                }
                let mut dirty = BTreeSet::new();
                let mut error = None;
                if now >= retry {
                    // Also retries vanished/replaced directories and exhausted watch limits.
                    native.sync(&paths, &mut dirty, &mut error);
                    retry = now + RETRY;
                }
                native.poll(&mut dirty, &mut error);
                for path in dirty.intersection(&paths) {
                    pending.mark(path.clone(), now);
                }
                let ready = pending.ready(now);
                let mut output = output.lock().unwrap();
                output.paths.retain(|path| paths.contains(path));
                for path in ready {
                    output.paths.insert(path);
                }
                if error.is_some() && error != last_error {
                    output.error = error.clone();
                }
                if error.is_some() {
                    last_error = error;
                }
            }
        });
        Self {
            paths: BTreeSet::new(),
            desired,
            wake,
            changes,
        }
    }

    pub fn set_paths(&mut self, paths: BTreeSet<PathBuf>) {
        if self.paths != paths {
            self.paths = paths.clone();
            *self.desired.lock().unwrap() = Some(paths);
            let _ = self.wake.try_send(());
        }
    }

    pub fn take_changes(&self) -> Changes {
        std::mem::take(&mut *self.changes.lock().unwrap())
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Native {
    fd: Option<rustix::fd::OwnedFd>,
    watches: BTreeMap<PathBuf, i32>,
}

#[cfg(target_os = "linux")]
impl Native {
    fn sync(
        &mut self,
        paths: &BTreeSet<PathBuf>,
        dirty: &mut BTreeSet<PathBuf>,
        error: &mut Option<String>,
    ) {
        use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
        if self.fd.is_none() {
            match inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK) {
                Ok(fd) => self.fd = Some(fd),
                Err(e) => {
                    *error = Some(e.to_string());
                    dirty.extend(paths.iter().cloned());
                    return;
                }
            }
        }
        let fd = self.fd.as_ref().unwrap();
        let obsolete: Vec<_> = self
            .watches
            .keys()
            .filter(|p| !paths.contains(*p))
            .cloned()
            .collect();
        for path in obsolete {
            let wd = self.watches.remove(&path).unwrap();
            // Different path spellings can refer to the same watched inode.
            if !self.watches.values().any(|&other| other == wd) {
                let _ = inotify::remove_watch(fd, wd);
            }
        }
        let flags = WatchFlags::CREATE
            | WatchFlags::DELETE
            | WatchFlags::MOVED_FROM
            | WatchFlags::MOVED_TO
            | WatchFlags::MODIFY
            | WatchFlags::CLOSE_WRITE
            | WatchFlags::ATTRIB
            | WatchFlags::DELETE_SELF
            | WatchFlags::MOVE_SELF
            | WatchFlags::ONLYDIR;
        for path in paths {
            if self.watches.contains_key(path) {
                continue;
            }
            match inotify::add_watch(fd, path, flags) {
                Ok(wd) => {
                    self.watches.insert(path.clone(), wd);
                }
                Err(e) => {
                    if e != rustix::io::Errno::NOENT && e != rustix::io::Errno::NOTDIR {
                        *error = Some(e.to_string());
                    }
                }
            }
            // Covers the gap between the first scan and watch registration,
            // and supplies a polling fallback for paths that cannot be watched.
            dirty.insert(path.clone());
        }
    }

    fn poll(&mut self, dirty: &mut BTreeSet<PathBuf>, error: &mut Option<String>) {
        use rustix::fs::inotify::{self, ReadFlags};
        let Some(fd) = &self.fd else { return };
        let mut buffer = [std::mem::MaybeUninit::uninit(); 16_384];
        let mut reader = inotify::Reader::new(fd, &mut buffer);
        // A busy writer must not starve subscription updates or shutdown.
        for index in 0..4096 {
            let event = match reader.next() {
                Ok(event) => event,
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => {
                    *error = Some(e.to_string());
                    dirty.extend(self.watches.keys().cloned());
                    self.watches.clear();
                    self.fd = None;
                    break;
                }
            };
            if index == 4095 {
                // Dropping a bounded Reader can discard the rest of its buffer.
                dirty.extend(self.watches.keys().cloned());
            }
            if event.events().contains(ReadFlags::QUEUE_OVERFLOW) {
                dirty.extend(self.watches.keys().cloned());
                continue;
            }
            let affected: Vec<_> = self
                .watches
                .iter()
                .filter(|(_, wd)| **wd == event.wd())
                .map(|(path, _)| path.clone())
                .collect();
            dirty.extend(affected.iter().cloned());
            if event.events().intersects(
                ReadFlags::IGNORED
                    | ReadFlags::DELETE_SELF
                    | ReadFlags::MOVE_SELF
                    | ReadFlags::UNMOUNT,
            ) {
                for path in affected {
                    self.watches.remove(&path);
                }
                if !event.events().contains(ReadFlags::IGNORED) {
                    let _ = inotify::remove_watch(fd, event.wd());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bursts_coalesce_without_starving_a_continuously_changing_directory() {
        let mut pending = Pending::default();
        let start = Instant::now();
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        pending.mark(a.clone(), start);
        pending.mark(a.clone(), start + Duration::from_millis(100));
        pending.mark(b.clone(), start);
        assert_eq!(pending.ready(start + QUIET), BTreeSet::from([b]));
        assert!(pending.ready(start + Duration::from_millis(200)).is_empty());
        assert_eq!(
            pending.ready(start + Duration::from_millis(250)),
            BTreeSet::from([a.clone()])
        );
        for millis in (300..=900).step_by(50) {
            pending.mark(a.clone(), start + Duration::from_millis(millis));
        }
        assert_eq!(
            pending.ready(start + Duration::from_millis(900)),
            BTreeSet::from([a])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_do_not_trigger_refresh_and_removed_subscriptions_stay_removed() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "data").unwrap();
        let mut watcher = Watcher::new();
        watcher.set_paths(BTreeSet::from([root.path().to_path_buf()]));
        let end = Instant::now() + Duration::from_secs(5);
        while watcher.take_changes().paths.is_empty() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
        crate::fs::read_directory(root.path()).unwrap();
        std::fs::read(&file).unwrap();
        thread::sleep(QUIET * 2);
        assert!(
            watcher.take_changes().paths.is_empty(),
            "Our own scans must not loop"
        );
        watcher.set_paths(BTreeSet::new());
        std::fs::write(file, "changed").unwrap();
        thread::sleep(QUIET * 2);
        assert!(watcher.take_changes().paths.is_empty());
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Default)]
struct Native;

#[cfg(not(target_os = "linux"))]
impl Native {
    fn sync(
        &mut self,
        paths: &BTreeSet<PathBuf>,
        dirty: &mut BTreeSet<PathBuf>,
        _: &mut Option<String>,
    ) {
        dirty.extend(paths.iter().cloned());
    }
    fn poll(&mut self, _: &mut BTreeSet<PathBuf>, _: &mut Option<String>) {}
}

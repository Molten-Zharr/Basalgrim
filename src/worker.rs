use crate::fs::{self, Operation, Preview, Report};
use crate::operations::{Cancellation, Progress};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

mod watch;

pub type DirectoryResult = Result<crate::archive::Directory, String>;
type Archive = Option<Arc<crate::archive::Catalog>>;

pub enum Event {
    RenamePreview {
        token: u64,
        result: Result<crate::batch_rename::Preview, String>,
    },
    HistoryStorage {
        token: u64,
        result: Result<crate::journal::storage::Catalog, String>,
    },
    Trash {
        token: u64,
        result: Result<crate::trash::Catalog, String>,
    },
    Journal {
        token: u64,
        result: Result<Vec<crate::journal::Stored>, String>,
    },
    Conflict {
        token: u64,
        request: u64,
        conflict: crate::conflicts::Conflict,
        reply: mpsc::SyncSender<crate::conflicts::Decision>,
    },
    Directory {
        token: u64,
        result: DirectoryResult,
    },
    Preview {
        token: u64,
        result: Result<Preview, String>,
    },
    Operation {
        token: u64,
        report: Box<Report>,
    },
    Open {
        path: Option<PathBuf>,
        result: Result<(), String>,
    },
}
enum Task {
    HistoryStorage {
        token: u64,
        cancellation: Cancellation,
    },
    Journal {
        token: u64,
        path: Option<PathBuf>,
        configure: bool,
    },
    Operation {
        token: u64,
        operation: Operation,
        cancellation: Cancellation,
    },
}
pub struct Worker {
    rename_previews: Sender<(u64, Arc<Vec<PathBuf>>, String, Cancellation)>,
    trash: Sender<(u64, Option<Vec<crate::trash::Root>>, Cancellation)>,
    scans: Sender<(u64, PathBuf, Archive, Cancellation)>,
    previews: Sender<(u64, PathBuf, bool, Archive, Cancellation)>,
    tasks: Option<Sender<Task>>,
    task_thread: Option<thread::JoinHandle<()>>,
    opens: Sender<(PathBuf, Archive)>,
    open_cancel: Cancellation,
    shutdown: Arc<AtomicBool>,
    pub events: Receiver<Event>,
    progress: Arc<Mutex<Option<(u64, Progress)>>>,
    watcher: watch::Watcher,
}
impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
impl Worker {
    pub fn new() -> Self {
        let (send, events) = mpsc::channel();
        let (rename_previews, rx) =
            mpsc::channel::<(u64, Arc<Vec<PathBuf>>, String, Cancellation)>();
        let result = send.clone();
        thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                for newer in rx.try_iter() {
                    request = newer;
                }
                let (token, sources, pattern, cancel) = request;
                if cancel.is_cancelled() {
                    continue;
                }
                let preview = crate::batch_rename::preview(&sources, &pattern, &cancel)
                    .map_err(|e| format!("{e:#}"));
                if !cancel.is_cancelled()
                    && result
                        .send(Event::RenamePreview {
                            token,
                            result: preview,
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
        let (trash, rx) = mpsc::channel::<(u64, Option<Vec<crate::trash::Root>>, Cancellation)>();
        let result = send.clone();
        thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                for newer in rx.try_iter() {
                    request = newer;
                }
                let (token, roots, cancel) = request;
                if cancel.is_cancelled() {
                    continue;
                }
                let catalog = (|| -> anyhow::Result<_> {
                    let roots = match roots {
                        Some(r) => r,
                        None => crate::trash::discover(&cancel)?,
                    };
                    crate::trash::read(&roots, &cancel)
                })()
                .map_err(|e| format!("{e:#}"));
                if !cancel.is_cancelled()
                    && result
                        .send(Event::Trash {
                            token,
                            result: catalog,
                        })
                        .is_err()
                {
                    break;
                }
            }
        });
        let (scans, rx) = mpsc::channel::<(u64, PathBuf, Archive, Cancellation)>();
        let result = send.clone();
        thread::spawn(move || {
            for (token, path, archive, cancellation) in rx {
                if cancellation.is_cancelled() {
                    continue;
                }
                let read = (|| -> anyhow::Result<_> {
                    if let Some(snapshot) =
                        crate::archive::directory(&path, archive, &cancellation)?
                    {
                        return Ok(Some(snapshot));
                    }
                    let resolved = path.canonicalize()?;
                    Ok(
                        fs::read_directory_until(&resolved, || cancellation.is_cancelled())?
                            .map(|entries| (resolved, entries, None)),
                    )
                })();
                if cancellation.is_cancelled() {
                    continue;
                }
                let read = match read {
                    Ok(Some(snapshot)) => Ok(snapshot),
                    Ok(None) => continue,
                    Err(error) => Err(format!("{error:#}")),
                };
                let event = Event::Directory {
                    token,
                    result: read,
                };
                if result.send(event).is_err() {
                    break;
                }
            }
        });
        let (previews, rx) = mpsc::channel::<(u64, PathBuf, bool, Archive, Cancellation)>();
        let result = send.clone();
        thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                // Fast cursor motion only needs the latest preview.
                for newer in rx.try_iter() {
                    request = newer;
                }
                let (token, path, graphics, archive, cancellation) = request;
                if cancellation.is_cancelled() {
                    continue;
                }
                let preview = match archive {
                    Some(archive) => archive.preview(&path, graphics, &cancellation),
                    None => fs::preview_until(&path, graphics, &cancellation),
                }
                .map_err(|e| format!("{e:#}"));
                if cancellation.is_cancelled() {
                    continue;
                }
                if result
                    .send(Event::Preview {
                        token,
                        result: preview,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let open_cancel = Cancellation::default();
        let opening = open_cancel.clone();
        let (opens, rx) = mpsc::channel::<(PathBuf, Archive)>();
        let result = send.clone();
        thread::spawn(move || {
            let mut retained = Vec::new();
            let mut bytes = 0u64;
            for (path, archive) in rx {
                if opening.is_cancelled() {
                    break;
                }
                let recent = archive.is_none().then(|| path.clone());
                let opened = (|| -> anyhow::Result<()> {
                    let path = if let Some(archive) = archive {
                        anyhow::ensure!(
                            retained.len() < 32,
                            "Открыто 32 временных файла; используйте распаковку"
                        );
                        let dir = tempfile::tempdir()?;
                        let path = archive.materialize(
                            &path,
                            dir.path(),
                            (512u64 * 1024 * 1024).saturating_sub(bytes),
                            &opening,
                        )?;
                        bytes += std::fs::metadata(&path)?.len();
                        retained.push(dir);
                        path
                    } else {
                        path
                    };
                    crate::archive::check(&opening)?;
                    fs::open_external(&path)
                })();
                if result
                    .send(Event::Open {
                        path: recent,
                        result: opened.map_err(|e| format!("{e:#}")),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let (tasks, rx) = mpsc::channel();
        let progress = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&progress);
        let shutdown = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&shutdown);
        let task_thread = thread::spawn(move || {
            let mut journal: Option<Result<crate::journal::Database, String>> = None;
            let mut journal_path: Option<PathBuf> = None;
            for task in rx {
                let event = match task {
                    Task::HistoryStorage {
                        token,
                        cancellation,
                    } => {
                        if cancellation.is_cancelled() {
                            continue;
                        }
                        let result = match &mut journal {
                            Some(Ok(database)) => database
                                .storage_catalog(&cancellation)
                                .map_err(|e| format!("{e:#}")),
                            _ => Err("Журнал недоступен".into()),
                        };
                        Event::HistoryStorage { token, result }
                    }
                    Task::Journal {
                        token,
                        path,
                        configure,
                    } => {
                        if configure {
                            journal_path = path;
                            journal = None;
                        }
                        if journal.is_none() || journal.as_ref().is_some_and(Result::is_err) {
                            journal = Some(
                                journal_path
                                    .as_deref()
                                    .ok_or_else(|| "Путь журнала не задан".into())
                                    .and_then(|p| {
                                        crate::journal::Database::open(p)
                                            .map_err(|e| format!("{e:#}"))
                                    }),
                            );
                        }
                        let result = match journal.as_mut().unwrap() {
                            Ok(db) => db.history().map_err(|e| format!("{e:#}")),
                            Err(error) => Err(error.clone()),
                        };
                        if let Err(error) = &result {
                            journal = Some(Err(error.clone()));
                        }
                        Event::Journal { token, result }
                    }
                    Task::Operation {
                        token,
                        operation,
                        cancellation,
                    } => {
                        if stopping.load(Ordering::Acquire) {
                            cancellation.cancel();
                        }
                        let mut last = Instant::now()
                            .checked_sub(Duration::from_secs(1))
                            .unwrap_or_else(Instant::now);
                        let mut phase = None;
                        let mut request = 0;
                        let mut notify = |progress: &Progress| {
                            if stopping.load(Ordering::Acquire) {
                                cancellation.cancel();
                            }
                            if phase != Some(progress.phase)
                                || last.elapsed() >= Duration::from_millis(50)
                            {
                                if let Ok(mut slot) = latest.lock() {
                                    *slot = Some((token, progress.clone()));
                                }
                                phase = Some(progress.phase);
                                last = Instant::now();
                            }
                        };
                        let mut resolve = |conflict: &crate::conflicts::Conflict| {
                            request += 1;
                            let (reply, rx) = mpsc::sync_channel(1);
                            if send
                                .send(Event::Conflict {
                                    token,
                                    request,
                                    conflict: conflict.clone(),
                                    reply,
                                })
                                .is_err()
                            {
                                return crate::conflicts::Decision::CANCEL;
                            }
                            loop {
                                if cancellation.is_cancelled() || stopping.load(Ordering::Acquire) {
                                    return crate::conflicts::Decision::CANCEL;
                                }
                                match rx.recv_timeout(Duration::from_millis(40)) {
                                    Ok(decision) => return decision,
                                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                                        return crate::conflicts::Decision::CANCEL;
                                    }
                                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                                }
                            }
                        };
                        let report = if cancellation.is_cancelled() {
                            Report {
                                total: operation.total(),
                                cancelled: true,
                                ..Default::default()
                            }
                        } else {
                            match &mut journal {
                                None => fs::operate_with_conflicts(
                                    operation,
                                    &cancellation,
                                    &mut notify,
                                    &mut resolve,
                                ),
                                Some(Err(error)) => Report {
                                    total: operation.total(),
                                    error: Some(format!(
                                        "Журнал недоступен; файлы не изменены: {error}"
                                    )),
                                    ..Default::default()
                                },
                                Some(Ok(database)) => match operation {
                                    Operation::Privileged(request) => match database
                                        .begin(&Operation::Privileged(request.clone()), token)
                                    {
                                        Ok(mut recorder) => {
                                            let mut report = match recorder.delegated_intent() {
                                                Ok(()) => crate::privileged::run(
                                                    request,
                                                    &cancellation,
                                                    &mut notify,
                                                    &mut resolve,
                                                ),
                                                Err(e) => Report {
                                                    total: request.total(),
                                                    error: Some(format!(
                                                        "Журнал недоступен: {e:#}"
                                                    )),
                                                    ..Default::default()
                                                },
                                            };
                                            recorder.finish_delegated(&mut report);
                                            report
                                        }
                                        Err(e) => Report {
                                            total: request.total(),
                                            error: Some(format!("Журнал недоступен: {e:#}")),
                                            ..Default::default()
                                        },
                                    },
                                    Operation::CleanupHistory { selection } => database
                                        .cleanup_history(
                                            selection,
                                            token,
                                            &cancellation,
                                            &mut notify,
                                        ),
                                    Operation::Replay { original, redo, .. } => database.replay(
                                        original,
                                        redo,
                                        token,
                                        &cancellation,
                                        &mut notify,
                                    ),
                                    operation => crate::privileged::execute(
                                        operation,
                                        database,
                                        token,
                                        &cancellation,
                                        &mut notify,
                                        &mut resolve,
                                    ),
                                },
                            }
                        };
                        Event::Operation {
                            token,
                            report: Box::new(report),
                        }
                    }
                };
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            rename_previews,
            trash,
            scans,
            previews,
            tasks: Some(tasks),
            task_thread: Some(task_thread),
            opens,
            open_cancel,
            shutdown,
            events,
            progress,
            watcher: watch::Watcher::new(),
        }
    }
    pub fn scan(&self, token: u64, path: PathBuf, cancellation: Cancellation) {
        self.scan_archive(token, path, None, cancellation);
    }
    pub fn scan_archive(
        &self,
        token: u64,
        path: PathBuf,
        archive: Archive,
        cancellation: Cancellation,
    ) {
        let _ = self.scans.send((token, path, archive, cancellation));
    }
    pub fn rename_preview(
        &self,
        token: u64,
        sources: Arc<Vec<PathBuf>>,
        pattern: String,
        cancel: Cancellation,
    ) {
        let _ = self.rename_previews.send((token, sources, pattern, cancel));
    }
    pub fn read_trash(
        &self,
        token: u64,
        roots: Option<Vec<crate::trash::Root>>,
        cancellation: Cancellation,
    ) -> bool {
        self.trash.send((token, roots, cancellation)).is_ok()
    }
    pub fn journal(&self, token: u64, path: Option<PathBuf>, configure: bool) -> bool {
        self.tasks
            .as_ref()
            .unwrap()
            .send(Task::Journal {
                token,
                path,
                configure,
            })
            .is_ok()
    }
    pub fn history_storage(&self, token: u64, cancellation: Cancellation) -> bool {
        self.tasks
            .as_ref()
            .unwrap()
            .send(Task::HistoryStorage {
                token,
                cancellation,
            })
            .is_ok()
    }
    pub(crate) fn watch(&mut self, paths: BTreeSet<PathBuf>) {
        self.watcher.set_paths(paths);
    }
    pub(crate) fn directory_changes(&self) -> watch::Changes {
        self.watcher.take_changes()
    }
    pub fn preview(&self, token: u64, path: PathBuf) {
        self.preview_image(token, path, true, Cancellation::default());
    }
    pub fn preview_image(
        &self,
        token: u64,
        path: PathBuf,
        graphics: bool,
        cancellation: Cancellation,
    ) {
        self.preview_archive(token, path, graphics, None, cancellation);
    }
    pub fn preview_archive(
        &self,
        token: u64,
        path: PathBuf,
        graphics: bool,
        archive: Archive,
        cancellation: Cancellation,
    ) {
        let _ = self
            .previews
            .send((token, path, graphics, archive, cancellation));
    }
    pub fn operate(&self, token: u64, operation: Operation, cancellation: Cancellation) -> bool {
        self.tasks
            .as_ref()
            .unwrap()
            .send(Task::Operation {
                token,
                operation,
                cancellation,
            })
            .is_ok()
    }
    pub fn take_progress(&self) -> Option<(u64, Progress)> {
        self.progress.lock().ok()?.take()
    }
    pub fn open(&self, path: PathBuf) {
        let _ = self.opens.send((path, None));
    }
    pub fn open_archive(&self, path: PathBuf, archive: Arc<crate::archive::Catalog>) {
        let _ = self.opens.send((path, Some(archive)));
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.open_cancel.cancel();
        self.shutdown.store(true, Ordering::Release);
        self.tasks.take();
        if let Some(thread) = self.task_thread.take() {
            let _ = thread.join();
        }
    }
}

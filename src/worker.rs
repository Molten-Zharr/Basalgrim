use crate::fs::{self, Entry, Operation, Preview, Report};
use crate::operations::{Cancellation, Progress};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

pub enum Event {
    Directory {
        token: u64,
        result: Result<(PathBuf, Vec<Entry>), String>,
    },
    Preview {
        token: u64,
        result: Result<Preview, String>,
    },
    Operation {
        token: u64,
        report: Report,
    },
    Open(Result<(), String>),
}
enum Task {
    Operation {
        token: u64,
        operation: Operation,
        cancellation: Cancellation,
    },
    Open(PathBuf),
}
pub struct Worker {
    scans: Sender<(u64, PathBuf)>,
    previews: Sender<(u64, PathBuf)>,
    tasks: Sender<Task>,
    pub events: Receiver<Event>,
    progress: Arc<Mutex<Option<(u64, Progress)>>>,
}
impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
impl Worker {
    pub fn new() -> Self {
        let (send, events) = mpsc::channel();
        let (scans, rx) = mpsc::channel::<(u64, PathBuf)>();
        let result = send.clone();
        thread::spawn(move || {
            for (token, path) in rx {
                let event = Event::Directory {
                    token,
                    result: (|| {
                        let resolved = path.canonicalize()?;
                        let entries = fs::read_directory(&resolved)?;
                        Ok((resolved, entries))
                    })()
                    .map_err(|e: anyhow::Error| format!("{e:#}")),
                };
                if result.send(event).is_err() {
                    break;
                }
            }
        });
        let (previews, rx) = mpsc::channel::<(u64, PathBuf)>();
        let result = send.clone();
        thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                // Fast cursor motion only needs the latest preview.
                for newer in rx.try_iter() {
                    request = newer;
                }
                let (token, path) = request;
                if result
                    .send(Event::Preview {
                        token,
                        result: fs::preview(&path).map_err(|e| format!("{e:#}")),
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
        thread::spawn(move || {
            for task in rx {
                let event = match task {
                    Task::Operation {
                        token,
                        operation,
                        cancellation,
                    } => {
                        let mut last = Instant::now()
                            .checked_sub(Duration::from_secs(1))
                            .unwrap_or_else(Instant::now);
                        let mut phase = None;
                        let report =
                            fs::operate_with_control(operation, &cancellation, &mut |progress| {
                                if phase != Some(progress.phase)
                                    || last.elapsed() >= Duration::from_millis(50)
                                {
                                    if let Ok(mut slot) = latest.lock() {
                                        *slot = Some((token, progress.clone()));
                                    }
                                    phase = Some(progress.phase);
                                    last = Instant::now();
                                }
                            });
                        Event::Operation { token, report }
                    }
                    Task::Open(path) => {
                        Event::Open(fs::open_external(&path).map_err(|e| format!("{e:#}")))
                    }
                };
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            scans,
            previews,
            tasks,
            events,
            progress,
        }
    }
    pub fn scan(&self, token: u64, path: PathBuf) {
        let _ = self.scans.send((token, path));
    }
    pub fn preview(&self, token: u64, path: PathBuf) {
        let _ = self.previews.send((token, path));
    }
    pub fn operate(&self, token: u64, operation: Operation, cancellation: Cancellation) -> bool {
        self.tasks
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
        let _ = self.tasks.send(Task::Open(path));
    }
}

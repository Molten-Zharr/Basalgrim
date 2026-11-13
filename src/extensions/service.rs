use super::*;
use std::{
    sync::{
        Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

pub enum Job {
    Reload,
    Install(PathBuf),
    Toggle(Plugin),
    Remove(Plugin),
    Run {
        plugin: Plugin,
        command: String,
        context: Context,
    },
}
pub enum Event {
    Catalog(Catalog),
    Done {
        id: u64,
        result: Result<String, String>,
    },
    HookError(String),
}
struct Notice {
    hook: &'static str,
    context: Context,
    outcome: Option<String>,
}
pub struct Service {
    jobs: Option<SyncSender<(u64, Job, Cancellation)>>,
    pub events: Receiver<Event>,
    notices: Arc<Mutex<Vec<Notice>>>,
    shutdown: Cancellation,
    current: Arc<Mutex<Cancellation>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Service {
    pub fn new(store: Store) -> Self {
        let (jobs, receive) = mpsc::sync_channel::<(u64, Job, Cancellation)>(4);
        let (events, receiver) = mpsc::sync_channel(8);
        let notices: Arc<Mutex<Vec<Notice>>> = Default::default();
        let pending = notices.clone();
        let shutdown = Cancellation::default();
        let stop = shutdown.clone();
        let current: Arc<Mutex<Cancellation>> = Default::default();
        let active = current.clone();
        let task = thread::spawn(move || {
            let mut catalog: Catalog = Arc::new(vec![]);
            while !stop.is_cancelled() {
                let job = match receive.recv_timeout(Duration::from_millis(30)) {
                    Ok(job) => Some(job),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                };
                if let Some((id, job, cancel)) = job {
                    *active.lock().unwrap() = cancel.clone();
                    if stop.is_cancelled() {
                        cancel.cancel();
                    }
                    let reload = !matches!(job, Job::Run { .. });
                    let result: Result<String> = (|| {
                        crate::fs::operation_checkpoint(&cancel)?;
                        match job {
                            Job::Reload => Ok("Список расширений обновлён".into()),
                            Job::Install(source) => store.install(&source, &cancel),
                            Job::Toggle(plugin) => store.toggle(&plugin, &cancel),
                            Job::Remove(plugin) => store.remove(&plugin, &cancel),
                            Job::Run {
                                plugin,
                                command,
                                context,
                            } => {
                                ensure!(
                                    plugin
                                        .manifest
                                        .commands
                                        .iter()
                                        .any(|c| c.id == command && context.matches(c)),
                                    "Команда недоступна для этого выбора"
                                );
                                let r = process::run(
                                    &plugin,
                                    &Request {
                                        api: 1,
                                        hook: "command",
                                        command: Some(&command),
                                        context: &context,
                                        outcome: None,
                                    },
                                    &cancel,
                                    Duration::from_secs(30),
                                )?;
                                Ok(if r.message.is_empty() {
                                    "Команда расширения завершена".into()
                                } else {
                                    crate::fs::clean(&r.message)
                                })
                            }
                        }
                    })();
                    let mut result = result.map_err(|e| format!("{e:#}"));
                    if reload {
                        for p in catalog.iter() {
                            p.lease.cancel();
                        }
                        match store.load(&stop) {
                            Ok(plugins) => catalog = Arc::new(plugins),
                            Err(e) => {
                                catalog = Arc::new(vec![]);
                                result = Err(format!("{e:#}"));
                            }
                        }
                        if !send(&events, Event::Catalog(catalog.clone()), &stop) {
                            break;
                        }
                    }
                    if !send(&events, Event::Done { id, result }, &stop) {
                        break;
                    }
                } else {
                    let notice = {
                        let mut n = pending.lock().unwrap();
                        if n.is_empty() {
                            None
                        } else {
                            Some(n.remove(0))
                        }
                    };
                    if let Some(notice) = notice {
                        for plugin in catalog.iter().filter(|p| {
                            p.enabled && p.manifest.events.iter().any(|e| e == notice.hook)
                        }) {
                            *active.lock().unwrap() = stop.clone();
                            let request = Request {
                                api: 1,
                                hook: notice.hook,
                                command: None,
                                context: &notice.context,
                                outcome: notice.outcome.as_deref(),
                            };
                            if let Err(e) =
                                process::run(plugin, &request, &stop, Duration::from_secs(2))
                            {
                                let _ = events.try_send(Event::HookError(format!(
                                    "{}: {e:#}",
                                    plugin.manifest.name
                                )));
                            }
                        }
                    }
                }
            }
            for p in catalog.iter() {
                p.lease.cancel();
            }
        });
        Self {
            jobs: Some(jobs),
            events: receiver,
            notices,
            shutdown,
            current,
            thread: Some(task),
        }
    }
    pub fn send(&self, id: u64, job: Job, cancel: Cancellation) -> bool {
        self.jobs
            .as_ref()
            .is_some_and(|tx| tx.try_send((id, job, cancel)).is_ok())
    }
    pub fn notify(&self, hook: &'static str, context: Context, outcome: Option<String>) {
        let mut notices = self.notices.lock().unwrap();
        // Events describe the latest state, not a lossless operation log.
        if let Some(old) = notices.iter_mut().find(|n| n.hook == hook) {
            *old = Notice {
                hook,
                context,
                outcome,
            };
        } else if notices.len() < 2 {
            notices.push(Notice {
                hook,
                context,
                outcome,
            });
        }
    }
}
fn send(tx: &SyncSender<Event>, mut event: Event, stop: &Cancellation) -> bool {
    loop {
        if stop.is_cancelled() {
            return false;
        }
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(e)) => {
                event = e;
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.current.lock().unwrap().cancel();
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

//! GVfs connections; UI-independent requests and guarded asynchronous interaction.
use crate::operations::Cancellation;
use anyhow::{Context, Result, ensure};
use gio::glib;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use zeroize::Zeroizing;
mod gvfs;
pub use gvfs::Gvfs;

pub const SCHEMES: &[&str] = &["sftp", "smb", "dav", "davs", "ftp", "ftps"];
pub fn address(text: &str) -> Result<String> {
    ensure!(
        text.len() <= 4096 && !text.chars().any(char::is_control),
        "Адрес слишком длинный или содержит управляющие символы"
    );
    let text = text.trim();
    let parsed = glib::Uri::parse(
        text,
        glib::UriFlags::HAS_PASSWORD
            | glib::UriFlags::HAS_AUTH_PARAMS
            | glib::UriFlags::ENCODED_PATH,
    )
    .map_err(|_| anyhow::anyhow!("Неверный адрес. Пример: sftp://server/папка"))?;
    ensure!(
        SCHEMES.contains(&parsed.scheme().as_str()),
        "Используйте sftp://, smb://, dav://, davs:// или ftp://"
    );
    ensure!(
        parsed.host().is_some_and(|h| !h.is_empty()),
        "Укажите имя сервера после ://"
    );
    ensure!(
        parsed.password().is_none() && parsed.auth_params().is_none(),
        "Пароль вводится отдельно, после подключения к серверу"
    );
    ensure!(
        !parsed
            .host()
            .is_some_and(|s| s.chars().any(char::is_control))
            && !parsed
                .user()
                .is_some_and(|s| s.chars().any(char::is_control))
            && !parsed
                .path()
                .as_bytes()
                .windows(3)
                .any(|s| s.eq_ignore_ascii_case(b"%00")),
        "Адрес содержит недопустимые управляющие символы"
    );
    ensure!(
        parsed.query().is_none() && parsed.fragment().is_none(),
        "Адрес папки не должен содержать ? или #; используйте %3F и %23 в именах"
    );
    Ok(parsed.to_str().into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub label: String,
    pub root: PathBuf,
    pub path: PathBuf,
    /// Private monitor generation, never a remote/server-supplied identifier.
    pub identity: String,
}
impl Location {
    pub fn address_for(&self, path: &Path) -> Option<String> {
        use std::os::unix::ffi::OsStrExt;
        let relative = path.strip_prefix(&self.root).ok()?;
        let mut uri = self.uri.trim_end_matches('/').to_owned();
        for component in relative.components() {
            uri.push('/');
            for &b in component.as_os_str().as_bytes() {
                if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                    uri.push(b as char);
                } else {
                    use std::fmt::Write;
                    let _ = write!(uri, "%{b:02X}");
                }
            }
        }
        Some(uri)
    }
}
#[derive(Clone)]
pub enum Prompt {
    Password {
        message: String,
        user: String,
        domain: String,
        need_user: bool,
        need_password: bool,
        need_domain: bool,
        anonymous: bool,
    },
    Question {
        message: String,
        choices: Vec<String>,
    },
}
pub enum Answer {
    Credentials {
        user: String,
        password: Zeroizing<String>,
        domain: String,
        anonymous: bool,
    },
    Choice(usize),
}
pub enum Event {
    Prompt {
        operation: u64,
        prompt: u64,
        value: Prompt,
    },
    Withdraw {
        operation: u64,
        prompt: u64,
    },
    Done {
        operation: u64,
        result: Result<Option<Location>, String>,
    },
}
pub struct Interaction {
    pub operation: u64,
    pub cancel: Cancellation,
    pub(super) events: mpsc::Sender<Event>,
    pub(super) answers: mpsc::Receiver<(u64, Answer)>,
}
impl Interaction {
    pub fn ask(&self, prompt: u64, value: Prompt) -> Result<()> {
        ensure!(!self.cancel.is_cancelled(), "Подключение отменено");
        self.events.send(Event::Prompt {
            operation: self.operation,
            prompt,
            value,
        })?;
        Ok(())
    }
    pub fn answer(&self) -> Option<(u64, Answer)> {
        self.answers.try_recv().ok()
    }
}
pub trait Backend {
    fn list(&mut self) -> Result<Vec<Location>>;
    fn connect(&mut self, uri: &str, interaction: &Interaction) -> Result<Location>;
    fn disconnect(&mut self, location: &Location, interaction: &Interaction) -> Result<()>;
}
pub enum Request {
    Connect(String),
    Disconnect(Location),
}
struct Job {
    id: u64,
    request: Request,
    cancel: Cancellation,
    answers: mpsc::Receiver<(u64, Answer)>,
}
type Latest = Arc<Mutex<Option<Result<Vec<Location>, String>>>>;
struct Runtime {
    sender: Option<mpsc::SyncSender<Job>>,
    events: mpsc::Receiver<Event>,
    latest: Latest,
    thread: Option<thread::JoinHandle<()>>,
    stop: Cancellation,
    active: Arc<Mutex<Option<Cancellation>>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(cancel) = self.active.lock().unwrap().as_ref() {
            cancel.cancel();
        }
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Default)]
pub struct Network {
    pub locations: Vec<Location>,
    pub revision: u64,
    pub error: Option<String>,
    pub ready: bool,
    pub working: Option<u64>,
    runtime: Option<Runtime>,
    cancel: Cancellation,
    answers: Option<mpsc::SyncSender<(u64, Answer)>>,
}
impl Network {
    pub fn attach(&mut self, factory: impl FnOnce() -> Box<dyn Backend> + Send + 'static) {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let (events, receive) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let shared = latest.clone();
        let stop = Cancellation::default();
        let stopped = stop.clone();
        let active: Arc<Mutex<Option<Cancellation>>> = Arc::default();
        let running = active.clone();
        let thread = thread::spawn(move || {
            let mut backend = factory();
            let context = glib::MainContext::new();
            let _ = context.with_thread_default(|| {
                while !stopped.is_cancelled() {
                    let result = backend.list().map_err(|e| format!("{e:#}"));
                    *shared.lock().unwrap() = Some(result);
                    match receiver.recv_timeout(Duration::from_secs(3)) {
                        Ok(job) => {
                            *running.lock().unwrap() = Some(job.cancel.clone());
                            if stopped.is_cancelled() {
                                job.cancel.cancel();
                            }
                            let io = Interaction {
                                operation: job.id,
                                cancel: job.cancel,
                                events: events.clone(),
                                answers: job.answers,
                            };
                            let result = if io.cancel.is_cancelled() {
                                Err(anyhow::anyhow!("Подключение отменено"))
                            } else {
                                match job.request {
                                    Request::Connect(uri) => backend.connect(&uri, &io).map(Some),
                                    Request::Disconnect(location) => {
                                        backend.disconnect(&location, &io).map(|_| None)
                                    }
                                }
                            }
                            .map_err(|e| format!("{e:#}"));
                            *running.lock().unwrap() = None;
                            let snapshot = backend.list().map_err(|e| format!("{e:#}"));
                            *shared.lock().unwrap() = Some(snapshot);
                            if events
                                .send(Event::Done {
                                    operation: job.id,
                                    result,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            });
        });
        *self = Self {
            runtime: Some(Runtime {
                sender: Some(sender),
                events: receive,
                latest,
                thread: Some(thread),
                stop,
                active,
            }),
            ..Default::default()
        };
    }
    pub fn start(&mut self, id: u64, request: Request) -> Result<()> {
        ensure!(self.working.is_none(), "Дождитесь завершения подключения");
        let runtime = self.runtime.as_ref().context("Служба сети не подключена")?;
        let (sender, answers) = mpsc::sync_channel(1);
        self.cancel = Cancellation::default();
        runtime
            .sender
            .as_ref()
            .unwrap()
            .try_send(Job {
                id,
                request,
                cancel: self.cancel.clone(),
                answers,
            })
            .map_err(|_| anyhow::anyhow!("Служба сети занята"))?;
        self.answers = Some(sender);
        self.working = Some(id);
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.cancel.cancel();
        self.answers = None;
    }
    pub fn reply(&self, operation: u64, prompt: u64, answer: Answer) {
        if self.working == Some(operation)
            && let Some(tx) = &self.answers
        {
            let _ = tx.try_send((prompt, answer));
        }
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(runtime) = &self.runtime else {
            return vec![];
        };
        let events: Vec<_> = runtime.events.try_iter().collect();
        if let Some(snapshot) = runtime.latest.lock().unwrap().take() {
            match snapshot {
                Ok(locations) => {
                    if locations != self.locations {
                        self.revision += 1;
                    }
                    self.locations = locations;
                    self.ready = true;
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error);
                    self.ready = false;
                }
            }
        }
        for event in &events {
            if let Event::Done { operation, .. } = event
                && self.working == Some(*operation)
            {
                self.working = None;
                self.answers = None;
            }
        }
        events
    }
    pub fn location(&self, path: &Path) -> Option<&Location> {
        self.locations
            .iter()
            .filter(|l| path.starts_with(&l.root))
            .max_by_key(|l| l.root.components().count())
    }
}

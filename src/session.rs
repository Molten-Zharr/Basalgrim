//! Session data and its independent, coalescing background writer.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_TABS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Location {
    Text(String),
    Unix { unix_bytes: Vec<u8> },
}
impl Location {
    fn new(path: &Path) -> Self {
        path.to_str().map_or_else(
            || Self::Unix {
                unix_bytes: path.as_os_str().as_bytes().to_vec(),
            },
            |s| Self::Text(s.into()),
        )
    }
    fn path(&self) -> PathBuf {
        match self {
            Self::Text(s) => s.into(),
            Self::Unix { unix_bytes } => std::ffi::OsString::from_vec(unix_bytes.clone()).into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SavedPane {
    tabs: Vec<Location>,
    active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    version: u8,
    panes: [SavedPane; 2],
    active: usize,
}
impl State {
    pub fn path() -> Option<PathBuf> {
        dirs::state_dir().map(|p| p.join("molten-zharr/basalgrim/session.json"))
    }
    pub fn capture(app: &crate::app::App) -> Self {
        Self {
            version: 1,
            panes: std::array::from_fn(|i| SavedPane {
                tabs: app.panes[i]
                    .tabs
                    .iter()
                    .map(|t| Location::new(&t.cwd))
                    .collect(),
                active: app.panes[i].active,
            }),
            active: app.active,
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            "Неизвестная версия сессии: {}",
            self.version
        );
        ensure!(self.active < 2, "Неверная активная панель");
        for pane in &self.panes {
            ensure!(
                !pane.tabs.is_empty() && pane.tabs.len() <= MAX_TABS,
                "В сессии должно быть от 1 до {MAX_TABS} вкладок на панель"
            );
            ensure!(pane.active < pane.tabs.len(), "Неверная активная вкладка");
            for location in &pane.tabs {
                let path = location.path();
                let bytes = path.as_os_str().as_bytes();
                ensure!(
                    path.is_absolute() && bytes.len() <= 4096 && !bytes.contains(&0),
                    "Некорректный путь в сессии"
                );
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let file = match rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.is_file(),
            "Сессия должна быть обычным файлом"
        );
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "Файл сессии слишком большой"
        );
        let state: Self = serde_json::from_slice(&bytes).context("Некорректный JSON сессии")?;
        state.validate()?;
        Ok(Some(state))
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_if(path, || true).map(|_| ())
    }
    fn save_if(&self, path: &Path, allowed: impl Fn() -> bool) -> Result<bool> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self)?;
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            "Файл сессии слишком большой"
        );
        if !allowed() {
            return Ok(false);
        }
        let parent = path.parent().context("Нет папки сессии")?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        if !allowed() {
            return Ok(false);
        }
        file.persist(path).map_err(|e| e.error)?;
        File::open(parent)?.sync_all()?;
        Ok(true)
    }
    pub(crate) fn restore(self, app: &mut crate::app::App) {
        app.panes = self.panes.map(|p| crate::model::Pane {
            tabs: p
                .tabs
                .into_iter()
                .map(|path| crate::model::Tab::new(path.path()))
                .collect(),
            active: p.active,
        });
        app.active = if app.settings.dual { self.active } else { 0 };
        for pane in 0..2 {
            for tab in 0..app.panes[pane].tabs.len() {
                app.reload(pane, tab);
            }
        }
    }
}

struct Request {
    id: u64,
    state: State,
}
pub(crate) struct Runtime {
    sender: Option<SyncSender<Request>>,
    results: Receiver<(u64, Result<(), String>)>,
    thread: Option<JoinHandle<()>>,
    enabled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    pending: Option<Request>,
    last: Option<State>,
    id: u64,
    retry: bool,
}
impl Runtime {
    pub fn new(path: PathBuf, enabled: bool) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        let (done, results) = mpsc::channel();
        let enabled = Arc::new(AtomicBool::new(enabled));
        let flag = Arc::clone(&enabled);
        let generation = Arc::new(AtomicU64::new(0));
        let current = Arc::clone(&generation);
        let thread = thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                let limit = Instant::now() + Duration::from_millis(1500);
                let mut quiet = Instant::now() + Duration::from_millis(300);
                let mut disconnected = false;
                loop {
                    let remaining = quiet.min(limit).saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    match rx.recv_timeout(remaining) {
                        Ok(newer) => {
                            request = newer;
                            quiet = Instant::now() + Duration::from_millis(300);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
                if flag.load(Ordering::Relaxed) {
                    let result = request.state.save_if(&path, || {
                        flag.load(Ordering::Relaxed) && current.load(Ordering::SeqCst) == request.id
                    });
                    match result {
                        Ok(false) => {}
                        result => {
                            let _ = done.send((
                                request.id,
                                result.map(|_| ()).map_err(|e| format!("{e:#}")),
                            ));
                        }
                    }
                }
                if disconnected {
                    break;
                }
            }
        });
        Self {
            sender: Some(tx),
            results,
            thread: Some(thread),
            enabled,
            generation,
            pending: None,
            last: None,
            id: 0,
            retry: false,
        }
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled.swap(enabled, Ordering::Relaxed) != enabled {
            self.last = None;
            self.generation.store(0, Ordering::SeqCst);
        }
        if !enabled {
            self.pending = None;
        }
    }
    pub fn retry(&mut self) {
        self.retry = true;
    }
    pub fn update(&mut self, state: State, enabled: bool) -> Option<Result<(), String>> {
        self.set_enabled(enabled);
        if enabled && (self.retry || self.last.as_ref() != Some(&state)) {
            self.id += 1;
            self.generation.store(self.id, Ordering::SeqCst);
            self.retry = false;
            self.last = Some(state.clone());
            self.pending = Some(Request { id: self.id, state });
        }
        if let Some(request) = self.pending.take() {
            match self.sender.as_ref().unwrap().try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => self.pending = Some(request),
                Err(TrySendError::Disconnected(_)) => {
                    return Some(Err("Исполнитель сохранения недоступен".into()));
                }
            }
        }
        self.results
            .try_iter()
            .filter(|(id, _)| enabled && *id == self.id)
            .last()
            .map(|(_, result)| result)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(request) = self.pending.take() {
            let _ = self.sender.as_ref().unwrap().send(request);
        }
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for (id, result) in self.results.try_iter() {
            if self.enabled.load(Ordering::Relaxed)
                && id == self.id
                && let Err(error) = result
            {
                eprintln!("Не удалось сохранить сессию: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_revoked_save_preserves_the_previous_session_after_staging() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.json");
        let mut state = State {
            version: 1,
            active: 0,
            panes: std::array::from_fn(|_| SavedPane {
                tabs: vec![Location::Text("/".into())],
                active: 0,
            }),
        };
        state.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        state.panes[0].tabs[0] = Location::Text("/tmp".into());
        let checks = std::cell::Cell::new(0);
        assert!(
            !state
                .save_if(&path, || {
                    let value = checks.get();
                    checks.set(value + 1);
                    value == 0
                })
                .unwrap()
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

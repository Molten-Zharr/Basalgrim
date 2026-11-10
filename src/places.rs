//! Personal places and desktop imports. All I/O belongs to the attached worker.
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
mod desktop;
mod store;
pub use store::Store;

pub const MAX_ITEMS: usize = 10_000;
pub const RECENT_LIMIT: usize = 500;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Bookmarks,
    Favorites,
    Recent,
}
impl Section {
    pub const ALL: [Self; 3] = [Self::Bookmarks, Self::Favorites, Self::Recent];
    pub fn label(self) -> &'static str {
        match self {
            Self::Bookmarks => "Закладки",
            Self::Favorites => "Избранное",
            Self::Recent => "Недавние",
        }
    }
    pub(crate) fn key(self) -> i64 {
        match self {
            Self::Bookmarks => 0,
            Self::Favorites => 1,
            Self::Recent => 2,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub label: String,
    pub desktop: bool,
    pub visited: i64,
}
impl Item {
    pub fn new(path: PathBuf) -> Self {
        let label = crate::fs::clean(
            &path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy(),
        );
        Self {
            path,
            label,
            desktop: false,
            visited: 0,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub bookmarks: Vec<Item>,
    pub favorites: Vec<Item>,
    pub recent: Vec<Item>,
    pub warnings: Vec<String>,
}
impl Snapshot {
    pub fn items(&self, section: Section) -> &[Item] {
        match section {
            Section::Bookmarks => &self.bookmarks,
            Section::Favorites => &self.favorites,
            Section::Recent => &self.recent,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Config {
    pub database: PathBuf,
    pub bookmarks: PathBuf,
    pub legacy_bookmarks: PathBuf,
    pub recent: PathBuf,
}
impl Config {
    pub fn desktop() -> Option<Self> {
        Some(Self {
            database: dirs::data_dir()?.join("molten-zharr/basalgrim/places.sqlite3"),
            bookmarks: dirs::config_dir()?.join("gtk-3.0/bookmarks"),
            legacy_bookmarks: dirs::home_dir()?.join(".gtk-bookmarks"),
            recent: dirs::data_dir()?.join("recently-used.xbel"),
        })
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    Refresh,
    Bookmark(PathBuf),
    Favorite {
        paths: Vec<PathBuf>,
        add: bool,
    },
    Remove {
        section: Section,
        path: PathBuf,
    },
    Rename {
        section: Section,
        path: PathBuf,
        label: String,
    },
    Recent(PathBuf),
    Resolve {
        id: u64,
        path: PathBuf,
    },
}
pub enum Event {
    Updated(Result<Snapshot, String>),
    Resolved(u64, Result<bool, String>),
}
struct Runtime {
    sender: Option<SyncSender<Command>>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub struct Library {
    pub snapshot: Snapshot,
    pub revision: u64,
    pub pending: usize,
    pub ready: bool,
    pub error: Option<String>,
    runtime: Option<Runtime>,
    refresh_at: Instant,
}
impl Default for Library {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            revision: 0,
            pending: 0,
            ready: false,
            error: None,
            runtime: None,
            refresh_at: Instant::now(),
        }
    }
}
impl Library {
    pub fn attach(&mut self, config: Config) {
        let (tx, rx) = mpsc::sync_channel(32);
        let (done, events) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut store = Store::open(&config.database).map_err(|e| format!("{e:#}"));
            for command in rx {
                let event = if let Command::Resolve { id, path } = command {
                    Event::Resolved(
                        id,
                        std::fs::metadata(&path)
                            .map(|m| m.is_dir())
                            .map_err(|e| format!("{}: {e}", path.display())),
                    )
                } else {
                    // Retry explicitly after a storage error without replacing the original file.
                    if store.is_err() && matches!(command, Command::Refresh) {
                        store = Store::open(&config.database).map_err(|e| format!("{e:#}"));
                    }
                    let result = match &mut store {
                        Ok(store) => store
                            .apply(command)
                            .and_then(|()| store.snapshot(&config))
                            .map_err(|e| format!("{e:#}")),
                        Err(e) => Err(e.clone()),
                    };
                    Event::Updated(result)
                };
                if done.send(event).is_err() {
                    break;
                }
            }
        });
        *self = Self {
            runtime: Some(Runtime {
                sender: Some(tx),
                events,
                thread: Some(thread),
            }),
            ..Self::default()
        };
        self.send(Command::Refresh);
    }
    pub fn send(&mut self, command: Command) -> bool {
        let Some(runtime) = &self.runtime else {
            self.error = Some("Хранилище мест не подключено".into());
            return false;
        };
        if runtime.sender.as_ref().unwrap().try_send(command).is_err() {
            self.error = Some("Места заняты; повторите действие".into());
            return false;
        }
        self.pending += 1;
        self.refresh_at = Instant::now() + Duration::from_secs(3);
        true
    }
    pub fn poll(&mut self) -> Vec<(u64, Result<bool, String>)> {
        let mut resolved = Vec::new();
        if let Some(runtime) = &self.runtime {
            while let Ok(event) = runtime.events.try_recv() {
                self.pending = self.pending.saturating_sub(1);
                match event {
                    Event::Updated(Ok(snapshot)) => {
                        if snapshot != self.snapshot {
                            self.revision += 1;
                            self.snapshot = snapshot;
                        }
                        self.ready = true;
                        self.error = None;
                    }
                    Event::Updated(Err(error)) => {
                        self.error = Some(error);
                    }
                    Event::Resolved(id, result) => resolved.push((id, result)),
                }
            }
        }
        if self.runtime.is_some() && self.pending == 0 && Instant::now() >= self.refresh_at {
            self.send(Command::Refresh);
        }
        resolved
    }
    pub fn writable(&self) -> bool {
        self.ready && self.pending == 0 && self.error.is_none()
    }
}

pub(crate) fn valid_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path.as_os_str().as_encoded_bytes().contains(&0)
            && path.as_os_str().len() <= 64 * 1024,
        "Некорректный путь места"
    );
    Ok(())
}
pub(crate) fn valid_label(label: &str) -> Result<()> {
    ensure!(
        !label.trim().is_empty() && label.len() <= 1024 && !label.chars().any(char::is_control),
        "Название должно содержать от 1 до 1024 байт без управляющих символов"
    );
    Ok(())
}

mod actions;
mod clipboard;
mod commands;
mod conflicts;
pub use conflicts::ConflictDialog;
mod directories;
pub use commands::{CommandItem, CommandMenu, MenuContext};
mod events;
mod journal;
mod storage;
pub use storage::StorageView;
mod operations;
mod path;
pub use path::{PathMenu, PathOwner};
mod selection;
mod session;
mod settings;
mod trash;
pub use settings::{
    SettingControl, SettingRow, SettingsEdit, SettingsFocus, SettingsSection, SettingsTab,
    SettingsView,
};
pub use trash::TrashView;

use crate::{
    fs::{self, Operation, Preview},
    input::Action,
    model::{Pane, Tab},
    settings::Settings,
    worker::{Event, Worker},
};
use ratatui::layout::{Position, Rect};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub enum Target {
    PathGo {
        owner: PathOwner,
        path: PathBuf,
    },
    PathCopy(PathOwner),
    PathEdit(PathOwner),
    PathParents {
        owner: PathOwner,
        anchor: Position,
    },
    PathParent(usize),
    PathScroll(isize),
    PathMenuCopy,
    PathMenuEdit,
    PathTextCopy,
    PathTextSelect {
        area: Rect,
        start: usize,
    },
    JournalRefresh,
    StorageRefresh,
    StoragePolicy,
    StorageClear,
    ConflictChoice(crate::conflicts::Choice),
    ConflictRemaining,
    Command(usize),
    CommandScroll(isize),
    MenuBackground,
    DismissMenu,
    EntryMark {
        pane: usize,
        index: usize,
    },
    Operation(usize),
    Action(Action),
    Place(PathBuf),
    Entry {
        pane: usize,
        index: usize,
    },
    Tab {
        pane: usize,
        index: usize,
    },
    Confirm,
    Cancel,
    SettingsSection(SettingsSection),
    SettingsTab(SettingsTab),
    Setting {
        index: usize,
        activate: bool,
        direction: isize,
    },
    BindingModifier(usize),
    BindingKey(crate::input::Key),
    RetrySave,
}

#[derive(Clone)]
pub enum EditKind {
    TrashSearch { original: String },
    Search { original: String },
    Path(PathBuf),
    Folder,
    File,
    Rename(PathBuf),
    Setting(SettingsEdit),
}
impl EditKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::TrashSearch { .. } => "Поиск в корзине",
            Self::Search { .. } => "Поиск в текущей папке",
            Self::Path(_) => "Перейти к папке",
            Self::Folder => "Новая папка",
            Self::File => "Новый файл",
            Self::Rename(_) => "Переименовать",
            Self::Setting(edit) => edit.title(),
        }
    }
}
pub enum Dialog {
    HistoryStorage,
    CleanupHistory(crate::journal::storage::Selection),
    DeleteTrash {
        targets: Vec<crate::trash::Deletion>,
        empty: bool,
    },
    RestoreTrash {
        entries: Vec<crate::trash::Entry>,
        destination: Option<PathBuf>,
    },
    Paths(PathMenu),
    Conflict(ConflictDialog),
    Commands(CommandMenu),
    Operations,
    Edit {
        kind: EditKind,
        text: String,
        error: Option<String>,
    },
    Trash(Vec<PathBuf>),
    Help,
    Settings,
    CaptureBinding {
        index: usize,
        error: Option<String>,
    },
}

pub use crate::clipboard::files::FileList as Clipboard;
pub struct Place {
    pub label: &'static str,
    pub path: PathBuf,
}

pub struct App {
    pub trash: Option<TrashView>,
    pub trash_home_only: bool,
    pub icon_backend: crate::icons::IconBackend,
    pub icon_slots: Vec<crate::icons::IconSlot>,
    pub preview_slot: Option<crate::preview::PreviewSlot>,
    pub panes: [Pane; 2],
    pub active: usize,
    pub settings: Settings,
    pub settings_dirty: bool,
    pub settings_view: SettingsView,
    pub settings_save_error: Option<String>,
    pub settings_save_available: bool,
    pub session_save_error: Option<String>,
    pub session_load_error: Option<String>,
    pub journal_path: Option<PathBuf>,
    pub journal_enabled: bool,
    pub journal_loading: bool,
    pub journal_error: Option<String>,
    pub history_storage: StorageView,
    pub dialog: Option<Dialog>,
    pub clipboard: Option<Clipboard>,
    file_exchange: clipboard::Exchange,
    text_copy: Option<crate::clipboard::TextCopy>,
    text_copy_token: u64,
    pub path_text_selected: bool,
    pub path_cursor: usize,
    pub places: Vec<Place>,
    pub preview: Option<Preview>,
    pub preview_error: Option<String>,
    pub preview_loading: bool,
    pub preview_offset: usize,
    pub preview_page: usize,
    pub help_offset: usize,
    pub help_max: usize,
    pub notice: String,
    pub notice_error: bool,
    pub busy: bool,
    pub quit: bool,
    pub hits: Vec<(Rect, Target)>,
    pub pointer: Option<Position>,
    pub preview_area: Rect,
    pub pane_areas: [Rect; 2],
    pub file_areas: [Rect; 2],
    marquee: Option<selection::Marquee>,
    pub operation_count: usize,
    pub operations: std::collections::VecDeque<crate::operations::OperationRecord>,
    pub operations_selected: usize,
    pub operations_offset: usize,
    pub operations_capacity: usize,
    pub operations_detail_offset: usize,
    pub operations_detail_max: usize,
    pub operations_details_area: Rect,
    running_operation: Option<operations::RunningOperation>,
    worker: Worker,
    serial: u64,
    preview_token: u64,
    preview_path: Option<PathBuf>,
    preview_graphics: bool,
    preview_cancel: crate::operations::Cancellation,
    directory_results: BTreeMap<u64, crate::worker::DirectoryResult>,
    pending_directory_changes: BTreeSet<PathBuf>,
    conflict_previous: Option<Dialog>,
    conflict_sequence: u64,
    session: Option<crate::session::Runtime>,
    journal_request: u64,
}

impl App {
    pub fn new(directory: PathBuf, settings: Settings) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| directory.clone());
        let mut places = vec![Place {
            label: "⌂  Домой",
            path: home.clone(),
        }];
        for (label, path) in [
            ("↓  Загрузки", dirs::download_dir()),
            ("▤  Документы", dirs::document_dir()),
            ("♫  Музыка", dirs::audio_dir()),
            ("▧  Изображения", dirs::picture_dir()),
            ("◇  Проекты", Some(home.join("projects"))),
        ] {
            if let Some(path) = path.filter(|p| p.is_dir() && *p != home) {
                places.push(Place { label, path });
            }
        }
        places.push(Place {
            label: "/  Файловая система",
            path: PathBuf::from("/"),
        });
        let mut app = Self {
            trash: None,
            trash_home_only: false,
            icon_backend: crate::icons::IconBackend::Symbols,
            icon_slots: Vec::new(),
            preview_slot: None,
            panes: [
                Pane {
                    tabs: vec![Tab::new(directory.clone())],
                    active: 0,
                },
                Pane {
                    tabs: vec![Tab::new(directory)],
                    active: 0,
                },
            ],
            active: 0,
            settings,
            settings_dirty: false,
            settings_view: SettingsView::default(),
            settings_save_error: None,
            settings_save_available: false,
            session_save_error: None,
            session_load_error: None,
            journal_path: None,
            journal_enabled: false,
            journal_loading: false,
            journal_error: None,
            history_storage: StorageView::default(),
            dialog: None,
            clipboard: None,
            file_exchange: Default::default(),
            text_copy: None,
            text_copy_token: 0,
            path_text_selected: false,
            path_cursor: 0,
            places,
            preview: None,
            preview_error: None,
            preview_loading: false,
            preview_offset: 0,
            preview_page: 10,
            help_offset: 0,
            help_max: 0,
            notice: String::new(),
            notice_error: false,
            busy: false,
            quit: false,
            hits: vec![],
            pointer: None,
            preview_area: Rect::default(),
            pane_areas: [Rect::default(); 2],
            file_areas: [Rect::default(); 2],
            marquee: None,
            operation_count: 0,
            operations: Default::default(),
            operations_selected: 0,
            operations_offset: 0,
            operations_capacity: 1,
            operations_detail_offset: 0,
            operations_detail_max: 0,
            operations_details_area: Rect::default(),
            running_operation: None,
            worker: Worker::new(),
            serial: 0,
            preview_token: 0,
            preview_path: None,
            preview_graphics: false,
            preview_cancel: Default::default(),
            directory_results: BTreeMap::new(),
            pending_directory_changes: BTreeSet::new(),
            conflict_previous: None,
            conflict_sequence: 0,
            session: None,
            journal_request: 0,
        };
        app.reload(0, 0);
        app.reload(1, 0);
        app.notice = format!(
            "{} — справка  ·  {} — выход",
            app.settings.shortcut(Action::Help),
            app.settings.shortcut(Action::Quit)
        );
        app
    }
    fn token(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
    pub fn file_tab(&self, pane: usize) -> &Tab {
        if self.trash_in(pane) {
            &self.trash.as_ref().unwrap().files
        } else {
            self.panes[pane].tab()
        }
    }
    pub fn file_tab_mut(&mut self, pane: usize) -> &mut Tab {
        if self.trash_in(pane) {
            &mut self.trash.as_mut().unwrap().files
        } else {
            self.panes[pane].tab_mut()
        }
    }
    pub fn tab(&self) -> &Tab {
        self.file_tab(self.active)
    }
    /// Resolve against this frame's active hit regions, including modal overlays.
    pub fn hovered_area(&self) -> Option<Rect> {
        if self.selection_area().is_some() {
            return None;
        }
        let pointer = self.pointer?;
        self.hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(pointer))
            .and_then(|(area, target)| {
                (!matches!(target, Target::MenuBackground | Target::DismissMenu)).then_some(*area)
            })
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        self.file_tab_mut(self.active)
    }
    pub fn loading(&self) -> bool {
        self.panes.iter().flat_map(|p| &p.tabs).any(|t| t.loading)
            || self.preview_loading
            || self.busy
            || self.clipboard_reads_pending()
            || self.history_storage.loading
            || self.trash.as_ref().is_some_and(|t| t.loading)
    }
    pub fn text_active(&self) -> bool {
        matches!(self.dialog, Some(Dialog::Edit { .. }))
    }
    pub fn message(&mut self, text: impl Into<String>, error: bool) {
        self.notice = fs::clean(&text.into());
        self.notice_error = error;
    }
    pub fn reload(&mut self, pane: usize, index: usize) {
        let token = self.token();
        let tab = &mut self.panes[pane].tabs[index];
        tab.scan_cancel.cancel();
        tab.scan_cancel = Default::default();
        tab.request = token;
        tab.loading = true;
        tab.refreshing = false;
        tab.error = None;
        self.worker
            .scan(token, tab.cwd.clone(), tab.scan_cancel.clone());
    }
    pub fn refresh(&mut self) {
        self.reload(self.active, self.panes[self.active].active);
    }
    pub fn navigate(&mut self, path: PathBuf, history: bool) {
        self.finish_marquee(true);
        if self.in_trash() {
            self.close_trash();
        }
        let tab = self.tab_mut();
        if history && tab.cwd != path {
            tab.back.push(tab.cwd.clone());
            tab.forward.clear();
        }
        tab.cwd = path;
        tab.entries.clear();
        tab.visible.clear();
        tab.marked.clear();
        tab.explicit_selection = false;
        tab.filter.clear();
        tab.selected = 0;
        tab.offset = 0;
        tab.follow_selection = true;
        self.refresh();
        self.update_preview();
    }
    pub fn tick(&mut self) {
        self.tick_trash();
        if matches!(&self.dialog,Some(Dialog::Paths(menu)) if !menu.owner.valid(self)) {
            self.dialog = None;
            self.hits.clear();
        }
        self.persist_session();
        self.validate_marquee();
        self.observe_directories();
        if let Some((token, progress)) = self.worker.take_progress() {
            self.accept_operation_progress(token, progress);
        }
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Trash { token, result } => self.accept_trash(token, result),
                Event::Journal { token, result } => self.accept_journal(token, result),
                Event::HistoryStorage { token, result } => {
                    self.accept_history_storage(token, result)
                }
                Event::Conflict {
                    token,
                    request,
                    conflict,
                    reply,
                } => self.accept_conflict(token, request, conflict, reply),
                Event::Directory { token, result } => {
                    self.directory_results.insert(token, result);
                }
                Event::Preview { token, result } if token == self.preview_token => {
                    self.preview_loading = false;
                    match result {
                        Ok(p) => {
                            self.preview = Some(p);
                            self.preview_error = None;
                        }
                        Err(e) => {
                            self.preview = None;
                            self.preview_error = Some(fs::clean(&e));
                        }
                    }
                }
                Event::Preview { .. } => {}
                Event::Operation { token, report } => self.finish_operation(token, report),
                Event::Open(result) => {
                    if let Err(e) = result {
                        self.message(e, true);
                    }
                }
            }
        }
        self.apply_directory_results();
        self.refresh_changed_directories();
        self.update_preview();
        self.apply_trash();
    }
    pub fn update_preview(&mut self) {
        let path = if self.settings.preview {
            self.tab().entry().map(|e| e.path.clone())
        } else {
            None
        };
        let graphics = self.icon_backend.supports_images();
        if self.preview_path == path && self.preview_graphics == graphics {
            return;
        }
        self.preview_path = path.clone();
        self.preview_graphics = graphics;
        self.preview_cancel.cancel();
        self.preview_cancel = Default::default();
        self.preview = None;
        self.preview_error = None;
        self.preview_offset = 0;
        self.preview_token = self.token();
        self.preview_loading = path.is_some();
        if let Some(path) = path {
            self.worker.preview_image(
                self.preview_token,
                path,
                graphics,
                self.preview_cancel.clone(),
            );
        }
    }
    pub fn open_selected(&mut self) {
        if self.in_trash() {
            self.confirm_restore_trash(false);
            return;
        }
        if let Some(entry) = self.tab().entry() {
            if entry.directory {
                self.navigate(entry.path.clone(), true);
            } else {
                self.worker.open(entry.path.clone());
            }
        }
    }
    pub fn home(&self) -> &Path {
        &self.places[0].path
    }
    pub fn persist_preferences(&mut self, path: Option<&Path>) {
        self.settings_save_available = path.is_some();
        if !self.settings_dirty || self.settings_save_error.is_some() {
            return;
        }
        if let Some(path) = path {
            match self.settings.save(path) {
                Ok(()) => self.settings_dirty = false,
                Err(e) => {
                    let message = format!("Не удалось сохранить настройки: {e:#}");
                    self.settings_save_error = Some(message.clone());
                    self.message(message, true);
                }
            }
        }
    }
}

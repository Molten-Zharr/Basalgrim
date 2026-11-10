mod actions;
mod archive;
mod batch;
mod folder_size;
mod search;
mod terminal;
pub use batch::BatchRenameView;
pub use search::SearchView;
mod clipboard;
mod commands;
mod conflicts;
pub use conflicts::ConflictDialog;
mod directories;
mod drag_drop;
pub use commands::{CommandItem, CommandMenu, MenuContext};
pub use drag_drop::DragFeedback;
mod editing;
mod events;
pub use editing::TextCommand;
mod journal;
mod storage;
pub use storage::StorageView;
mod operations;
mod path;
pub use path::{PathMenu, PathOwner};
mod places;
mod selection;
mod session;
pub use places::{PlaceControl, PlacesView};
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
    PlacesSection(crate::places::Section),
    SavedPlace(PathBuf),
    PlacesRow {
        revision: u64,
        index: usize,
    },
    PlacesControl {
        revision: u64,
        control: PlaceControl,
    },
    ArchiveFormat(crate::archive::Format),
    Terminal {
        id: u64,
        control: crate::terminal::Control,
    },
    FolderMeasure {
        owner: PathOwner,
        request: u64,
        path: PathBuf,
    },
    FolderStop(u64),
    FolderRetry(u64),
    FolderScroll(isize),
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
    TextSelect {
        area: Rect,
        start: usize,
    },
    TextCommand(TextCommand),
    TextMenuBackground,
    TextMenuDismiss,
    RecursiveSearch,
    SearchStop,
    SearchRetry,
    SearchMove(isize),
    SearchOpen(u64, usize),
    BatchNumber,
    BatchMode,
    BatchRefresh,
    BatchScroll(isize),
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
    EntryBackground,
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
    NewTab {
        pane: usize,
    },
    CloseTab {
        owner: PathOwner,
        request: u64,
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
    PlaceFilter,
    PlaceLabel {
        section: crate::places::Section,
        path: PathBuf,
    },
    ArchiveCreate {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    ArchiveExtract {
        request: crate::archive::Extraction,
        destination: PathBuf,
    },
    TrashSearch {
        original: String,
    },
    Search {
        original: String,
    },
    Path(PathBuf),
    Folder,
    File,
    Rename(PathBuf),
    RecursiveSearch,
    BatchRename,
    Setting(SettingsEdit),
}
impl EditKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::PlaceFilter => "Фильтр мест",
            Self::PlaceLabel { .. } => "Название места",
            Self::ArchiveCreate { .. } => "Создать архив",
            Self::ArchiveExtract { .. } => "Распаковать в папку",
            Self::TrashSearch { .. } => "Поиск в корзине",
            Self::Search { .. } => "Поиск в текущей папке",
            Self::Path(_) => "Перейти к папке",
            Self::Folder => "Новая папка",
            Self::File => "Новый файл",
            Self::Rename(_) => "Переименовать",
            Self::RecursiveSearch => "Поиск в подпапках",
            Self::BatchRename => "Массовое переименование",
            Self::Setting(edit) => edit.title(),
        }
    }
}
pub enum Dialog {
    Places,
    CloseTerminal,
    FolderSize,
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
    pub library: crate::places::Library,
    pub places_view: PlacesView,
    place_open: Option<places::PendingOpen>,
    pub trash: Option<TrashView>,
    pub trash_home_only: bool,
    pub icon_backend: crate::icons::IconBackend,
    pub icon_slots: Vec<crate::icons::IconSlot>,
    pub thumbnails: crate::thumbnails::Thumbnails,
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
    pub search: Option<SearchView>,
    pub folder_sizes: crate::folder_size::FolderSizes,
    pub terminal: crate::terminal::Terminal,
    pub folder_size_offset: usize,
    pub folder_size_max: usize,
    search_focus: Option<(u64, PathBuf)>,
    pub batch: Option<BatchRenameView>,
    pub clipboard: Option<Clipboard>,
    file_exchange: clipboard::Exchange,
    text_copy: Option<crate::clipboard::TextCopy>,
    text_copy_token: u64,
    pub editor: crate::text_edit::TextEdit,
    pub(crate) edit_owner: Option<PathOwner>,
    pub(crate) editor_menu: Option<(Position, usize)>,
    editor_drag: Option<Rect>,
    selection_extend: bool,
    pub(crate) rename_area: Option<Rect>,
    text_paste: Option<(u64, u64)>,
    text_paste_fallback: Option<u64>,
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
    file_drag: Option<drag_drop::FileDrag>,
    external_drop: drag_drop::ExternalDropState,
    pub(crate) drop_regions: Vec<drag_drop::DropRegion>,
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
            library: Default::default(),
            places_view: Default::default(),
            place_open: None,
            trash: None,
            trash_home_only: false,
            icon_backend: crate::icons::IconBackend::Symbols,
            icon_slots: Vec::new(),
            thumbnails: Default::default(),
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
            search: None,
            folder_sizes: Default::default(),
            terminal: Default::default(),
            folder_size_offset: 0,
            folder_size_max: 0,
            search_focus: None,
            batch: None,
            clipboard: None,
            file_exchange: Default::default(),
            text_copy: None,
            text_copy_token: 0,
            editor: Default::default(),
            edit_owner: None,
            editor_menu: None,
            editor_drag: None,
            selection_extend: false,
            rename_area: None,
            text_paste: None,
            text_paste_fallback: None,
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
            file_drag: None,
            external_drop: Default::default(),
            drop_regions: Vec::new(),
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
        app.notice = format!("{} — выход", app.settings.shortcut(Action::Quit));
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
        if self.selection_area().is_some() || self.dragging_files() || self.external_drag_active() {
            return None;
        }
        let pointer = self.pointer?;
        self.hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(pointer))
            .and_then(|(area, target)| {
                (!matches!(
                    target,
                    Target::Terminal {
                        control: crate::terminal::Control::Focus,
                        ..
                    } | Target::MenuBackground
                        | Target::DismissMenu
                        | Target::EntryBackground
                        | Target::TextMenuBackground
                        | Target::TextMenuDismiss
                ))
                .then_some(*area)
            })
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        self.file_tab_mut(self.active)
    }
    pub fn loading(&self) -> bool {
        if self.dragging_files() || self.external_drag_active() {
            return true;
        }
        // A completed snapshot held behind a dialog is idle, not pending I/O.
        self.panes.iter().flat_map(|p| &p.tabs).any(|t| {
            t.loading && !(t.refreshing && self.directory_results.contains_key(&t.request))
        }) || (self.terminal.visible && self.terminal.snapshot.running)
            || self.thumbnails.loading()
            || self.preview_loading
            || self.busy
            || self.clipboard_reads_pending()
            || self.history_storage.loading
            || !self.folder_sizes.result.done
            || self.search.as_ref().is_some_and(|v| !v.worker.results.done)
            || self.batch.as_ref().is_some_and(|v| v.pending)
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
        self.worker.scan_archive(
            token,
            tab.cwd.clone(),
            tab.archive.clone(),
            tab.scan_cancel.clone(),
        );
    }
    pub fn refresh(&mut self) {
        self.reload(self.active, self.panes[self.active].active);
    }
    pub fn navigate(&mut self, path: PathBuf, history: bool) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        if self.in_trash() {
            self.close_trash();
        }
        let tab = self.tab_mut();
        if history && tab.cwd != path {
            tab.back.push(tab.cwd.clone());
            tab.forward.clear();
        }
        if tab
            .archive
            .as_ref()
            .is_some_and(|a| !path.starts_with(&a.source))
        {
            tab.archive = None;
        }
        tab.cwd = path;
        tab.entries.clear();
        tab.visible.clear();
        tab.marked.clear();
        tab.explicit_selection = false;
        tab.filter.clear();
        tab.selected = 0;
        tab.reset_range();
        tab.offset = 0;
        tab.follow_selection = true;
        self.refresh();
        self.update_preview();
    }
    pub fn tick(&mut self) {
        if self.terminal.poll() && matches!(self.dialog, Some(Dialog::CloseTerminal)) {
            self.dialog = None;
        }
        self.tick_file_drag();
        self.tick_trash();
        if matches!(&self.dialog,Some(Dialog::Paths(menu)) if !menu.owner.valid(self)) {
            self.dialog = None;
            self.hits.clear();
        }
        self.poll_places();
        self.persist_session();
        self.validate_marquee();
        self.observe_directories();
        if let Some((token, progress)) = self.worker.take_progress() {
            self.accept_operation_progress(token, progress);
        }
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Trash { token, result } => self.accept_trash(token, result),
                Event::RenamePreview { token, result } => self.accept_rename_preview(token, result),
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
                Event::Operation { token, report } => self.finish_operation(token, *report),
                Event::Open { path, result } => match result {
                    Ok(()) => {
                        if let Some(path) = path.filter(|_| self.library.ready) {
                            self.library.send(crate::places::Command::Recent(path));
                        }
                    }
                    Err(e) => self.message(e, true),
                },
            }
        }
        self.apply_directory_results();
        self.poll_search();
        self.folder_sizes.poll();
        self.thumbnails.poll();
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
            self.worker.preview_archive(
                self.preview_token,
                path,
                graphics,
                self.tab().archive.clone(),
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
            if entry.directory
                || (self.tab().archive.is_none()
                    && !entry.symlink
                    && crate::archive::Format::of(&entry.path).is_some())
            {
                self.navigate(entry.path.clone(), true);
            } else if let Some(archive) = &self.tab().archive {
                self.worker
                    .open_archive(entry.path.clone(), archive.clone());
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

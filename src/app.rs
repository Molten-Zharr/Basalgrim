mod actions;
mod commands;
pub use commands::{CommandItem, CommandMenu};
mod events;
mod operations;
mod settings;
pub use settings::{
    SettingControl, SettingRow, SettingsEdit, SettingsFocus, SettingsSection, SettingsTab,
    SettingsView,
};

use crate::{
    fs::{self, Operation, Preview},
    input::Action,
    model::{Pane, Tab},
    settings::Settings,
    worker::{Event, Worker},
};
use ratatui::layout::{Position, Rect};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub enum Target {
    Command(usize),
    CommandScroll(isize),
    MenuBackground,
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
    Search { original: String },
    Path,
    Folder,
    File,
    Rename(PathBuf),
    Setting(SettingsEdit),
}
impl EditKind {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Search { .. } => "Поиск в текущей папке",
            Self::Path => "Перейти к папке",
            Self::Folder => "Новая папка",
            Self::File => "Новый файл",
            Self::Rename(_) => "Переименовать",
            Self::Setting(edit) => edit.title(),
        }
    }
}
pub enum Dialog {
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

pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}
pub struct Place {
    pub label: &'static str,
    pub path: PathBuf,
}

pub struct App {
    pub icon_backend: crate::icons::IconBackend,
    pub icon_slots: Vec<crate::icons::IconSlot>,
    pub panes: [Pane; 2],
    pub active: usize,
    pub settings: Settings,
    pub settings_dirty: bool,
    pub settings_view: SettingsView,
    pub settings_save_error: Option<String>,
    pub settings_save_available: bool,
    pub dialog: Option<Dialog>,
    pub clipboard: Option<Clipboard>,
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
            icon_backend: crate::icons::IconBackend::Symbols,
            icon_slots: Vec::new(),
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
            dialog: None,
            clipboard: None,
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
    pub fn tab(&self) -> &Tab {
        self.panes[self.active].tab()
    }
    /// Resolve against this frame's active hit regions, including modal overlays.
    pub fn hovered_area(&self) -> Option<Rect> {
        let pointer = self.pointer?;
        self.hits
            .iter()
            .rev()
            .find(|(area, _)| area.contains(pointer))
            .map(|(area, _)| *area)
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        self.panes[self.active].tab_mut()
    }
    pub fn loading(&self) -> bool {
        self.panes.iter().flat_map(|p| &p.tabs).any(|t| t.loading)
            || self.preview_loading
            || self.busy
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
        tab.request = token;
        tab.loading = true;
        tab.error = None;
        self.worker.scan(token, tab.cwd.clone());
    }
    pub fn refresh(&mut self) {
        self.reload(self.active, self.panes[self.active].active);
    }
    pub fn navigate(&mut self, path: PathBuf, history: bool) {
        let tab = self.tab_mut();
        if history && tab.cwd != path {
            tab.back.push(tab.cwd.clone());
            tab.forward.clear();
        }
        tab.cwd = path;
        tab.entries.clear();
        tab.visible.clear();
        tab.marked.clear();
        tab.filter.clear();
        tab.selected = 0;
        tab.offset = 0;
        self.refresh();
        self.update_preview();
    }
    pub fn tick(&mut self) {
        if let Some((token, progress)) = self.worker.take_progress() {
            self.accept_operation_progress(token, progress);
        }
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Directory { token, result } => {
                    for tab in self.panes.iter_mut().flat_map(|p| &mut p.tabs) {
                        if tab.request != token {
                            continue;
                        }
                        tab.loading = false;
                        match result {
                            Ok((cwd, entries)) => {
                                let previous = tab.entry().map(|e| e.path.clone());
                                tab.cwd = cwd;
                                tab.entries = entries;
                                // Old visible indices refer to the previous scan.
                                tab.visible.clear();
                                tab.rebuild(self.settings.hidden);
                                if let Some(index) = previous.and_then(|p| {
                                    tab.visible.iter().position(|&i| tab.entries[i].path == p)
                                }) {
                                    tab.selected = index;
                                }
                            }
                            Err(e) => {
                                tab.error = Some(fs::clean(&e));
                                tab.entries.clear();
                                tab.visible.clear();
                                tab.marked.clear();
                            }
                        }
                        break;
                    }
                }
                Event::Preview { token, result } if token == self.preview_token => {
                    self.preview_loading = false;
                    match result {
                        Ok(p) => self.preview = Some(p),
                        Err(e) => self.preview_error = Some(fs::clean(&e)),
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
        self.update_preview();
    }
    pub fn update_preview(&mut self) {
        let path = if self.settings.preview {
            self.tab().entry().map(|e| e.path.clone())
        } else {
            None
        };
        if self.preview_path == path {
            return;
        }
        self.preview_path = path.clone();
        self.preview = None;
        self.preview_error = None;
        self.preview_offset = 0;
        self.preview_token = self.token();
        self.preview_loading = path.is_some();
        if let Some(path) = path {
            self.worker.preview(self.preview_token, path);
        }
    }
    pub fn open_selected(&mut self) {
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

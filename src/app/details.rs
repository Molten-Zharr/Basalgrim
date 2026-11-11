use super::*;
use crate::{
    details::{Application, Event as DetailEvent, Properties, Source},
    input::{Key, KeyPress},
    operations::Cancellation,
};

#[derive(Clone, Copy)]
pub enum DetailControl {
    General,
    Permissions,
    Toggle(u32),
    Apply,
    Refresh,
    CopyPath,
    OpenWith,
    All,
    Row(usize),
    OpenRow(usize),
    Launch,
    Scroll(isize),
}
pub struct DetailView {
    pub owner: PathOwner,
    pub request: u64,
    pub source: Source,
    pub token: u64,
    pub revision: u64,
    pub properties: Option<Properties>,
    pub loading: bool,
    pub error: Option<String>,
    pub permissions: bool,
    pub mode: u32,
    pub bit: usize,
    pub scroll: usize,
    pub max_scroll: usize,
    pub chooser: bool,
    pub all: bool,
    pub applications: Vec<Application>,
    pub visible: Vec<usize>,
    pub filter: String,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    cancel: Cancellation,
}
impl App {
    pub fn details_dialog(&self) -> bool {
        matches!(
            self.dialog,
            Some(
                Dialog::Details
                    | Dialog::Edit {
                        kind: EditKind::ApplicationFilter,
                        ..
                    }
            )
        )
    }
    pub(crate) fn detail_source(&self, background: bool) -> Option<Source> {
        if self.in_trash() || self.tab().loading || self.tab().error.is_some() {
            return None;
        }
        let path = if background {
            self.tab().cwd.clone()
        } else {
            let paths = self.tab().targets();
            if paths.len() != 1 {
                return None;
            }
            paths[0].clone()
        };
        Some(Source {
            path,
            archive: self.tab().archive.clone(),
            readonly: self.in_archive(),
        })
    }
    pub(crate) fn can_open_with(&self) -> bool {
        self.detail_source(false).is_some_and(|s| {
            self.tab()
                .entries
                .iter()
                .any(|e| e.path == s.path && !e.directory && !e.symlink)
        })
    }
    pub fn open_details(&mut self, background: bool, chooser: bool) {
        let Some(source) = self.detail_source(background) else {
            return;
        };
        if chooser && !self.can_open_with() {
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.close_details();
        self.detail_view = Some(DetailView {
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            source,
            token: 0,
            revision: 0,
            properties: None,
            loading: true,
            error: None,
            permissions: false,
            mode: 0,
            bit: 0,
            scroll: 0,
            max_scroll: 0,
            chooser,
            all: false,
            applications: vec![],
            visible: vec![],
            filter: String::new(),
            selected: 0,
            offset: 0,
            page: 1,
            cancel: Default::default(),
        });
        if chooser {
            self.begin_edit(EditKind::ApplicationFilter, String::new(), 0..0);
        } else {
            self.dialog = Some(Dialog::Details);
        }
        self.reload_details();
    }
    pub(super) fn close_details(&mut self) {
        if let Some(view) = self.detail_view.take() {
            view.cancel.cancel();
        }
        if self.details_dialog() {
            self.dialog = None;
            self.editor_menu = None;
            self.editor_drag = None;
            self.text_paste = None;
            self.text_paste_fallback = None;
        }
    }
    fn reload_details(&mut self) {
        let token = self.token();
        let Some(v) = &mut self.detail_view else {
            return;
        };
        v.cancel.cancel();
        v.cancel = Default::default();
        v.token = token;
        v.revision = token;
        v.loading = true;
        v.error = None;
        v.scroll = 0;
        v.applications.clear();
        v.visible.clear();
        v.selected = 0;
        v.offset = 0;
        let result = if v.chooser {
            self.details
                .applications(token, v.source.clone(), v.all, v.cancel.clone())
        } else {
            self.details
                .properties(token, v.source.clone(), v.cancel.clone())
        };
        if let Err(e) = result {
            v.loading = false;
            v.error = Some(e.to_string());
        }
    }
    pub(crate) fn filter_applications(&mut self) {
        let filter = match &self.dialog {
            Some(Dialog::Edit {
                kind: EditKind::ApplicationFilter,
                text,
                ..
            }) => text.clone(),
            _ => String::new(),
        };
        let Some(v) = &mut self.detail_view else {
            return;
        };
        if v.filter != filter {
            v.filter = filter;
            v.selected = 0;
            v.offset = 0;
            v.revision += 1;
        }
        let query = v.filter.to_lowercase();
        v.visible = v
            .applications
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                format!("{} {}", a.name, a.description)
                    .to_lowercase()
                    .contains(&query)
            })
            .map(|(i, _)| i)
            .collect();
        v.selected = v.selected.min(v.visible.len().saturating_sub(1));
    }
    pub(super) fn poll_details(&mut self) {
        if self.detail_view.as_ref().is_some_and(|v| {
            !self.details_dialog()
                || !v.owner.valid(self)
                || v.owner.pane != self.active
                || v.request != self.tab().request
        }) {
            self.close_details();
        }
        for event in self.details.poll() {
            match event {
                DetailEvent::Launched(_, path, result) => match result {
                    Ok(()) => {
                        if let Some(path) = path.filter(|_| self.library.ready) {
                            self.library.send(crate::places::Command::Recent(path));
                        }
                        self.message("Файл передан выбранной программе", false);
                    }
                    Err(e) => self.message(e, true),
                },
                DetailEvent::Properties(id, result) => {
                    if let Some(v) = &mut self.detail_view
                        && v.token == id
                    {
                        v.loading = false;
                        v.revision += 1;
                        match result {
                            Ok(p) => {
                                v.mode = p.mode & 0o777;
                                v.properties = Some(p);
                            }
                            Err(e) => v.error = Some(e),
                        }
                    }
                }
                DetailEvent::Applications(id, result) => {
                    if let Some(v) = &mut self.detail_view
                        && v.token == id
                    {
                        v.loading = false;
                        v.revision += 1;
                        match result {
                            Ok((p, rows)) => {
                                v.properties = Some(p);
                                v.applications = rows;
                            }
                            Err(e) => v.error = Some(e),
                        }
                    }
                }
            }
        }
        self.filter_applications();
    }
    pub(super) fn detail_control(&mut self, revision: u64, control: DetailControl) {
        if !self.details_dialog() {
            return;
        }
        self.filter_applications();
        let Some(v) = &self.detail_view else { return };
        if v.revision != revision {
            return;
        }
        if !v.owner.valid(self) || v.owner.pane != self.active || v.request != self.tab().request {
            self.close_details();
            return;
        }
        use DetailControl::*;
        match control {
            Apply => {
                if self.busy
                    || self.devices.working.is_some()
                    || self.network.working.is_some()
                    || self.clipboard_reads_pending()
                {
                    return;
                }
                let Some(p) = v
                    .properties
                    .as_ref()
                    .filter(|_| !v.loading && v.error.is_none())
                else {
                    return;
                };
                if v.mode == p.mode & 0o777 {
                    return;
                }
                match p.change(v.mode) {
                    Ok(change) => {
                        self.start_operation(Operation::Permissions(change));
                        if self.busy {
                            self.close_details();
                        }
                    }
                    Err(e) => self.detail_view.as_mut().unwrap().error = Some(e.to_string()),
                }
            }
            CopyPath => {
                let path = v.source.path.clone();
                self.copy_path(&path);
            }
            OpenWith => {
                if !v.loading
                    && v.error.is_none()
                    && v.properties
                        .as_ref()
                        .is_some_and(|p| p.link.is_none() && p.kind.starts_with("Файл"))
                {
                    self.detail_view.as_mut().unwrap().chooser = true;
                    self.begin_edit(EditKind::ApplicationFilter, String::new(), 0..0);
                    self.reload_details();
                }
            }
            Refresh => self.reload_details(),
            Launch => {
                self.filter_applications();
                let v = self.detail_view.as_ref().unwrap();
                if !v.chooser || v.loading || v.error.is_some() {
                    return;
                }
                let Some(index) = v.visible.get(v.selected) else {
                    return;
                };
                match self.details.launch(v.token, *index, v.cancel.clone()) {
                    Ok(()) => {
                        self.detail_view = None;
                        self.dialog = None;
                        self.text_paste = None;
                        self.editor_menu = None;
                        self.message("Запуск выбранной программы…", false);
                    }
                    Err(e) => self.detail_view.as_mut().unwrap().error = Some(e.to_string()),
                }
            }
            All => {
                let v = self.detail_view.as_mut().unwrap();
                v.all = !v.all;
                self.reload_details();
            }
            Row(index) | OpenRow(index) => {
                if !v.chooser || v.loading || v.error.is_some() || index >= v.visible.len() {
                    return;
                }
                let v = self.detail_view.as_mut().unwrap();
                v.selected = index;
                v.revision += 1;
                let revision = v.revision;
                if matches!(control, OpenRow(_)) {
                    self.detail_control(revision, Launch);
                }
            }
            other => {
                let v = self.detail_view.as_mut().unwrap();
                match other {
                    General => {
                        v.permissions = false;
                        v.scroll = 0;
                    }
                    Permissions => {
                        v.permissions = true;
                        v.scroll = 0;
                    }
                    Toggle(bit)
                        if bit.count_ones() == 1
                            && bit <= 0o400
                            && !v.loading
                            && v.error.is_none()
                            && v.properties.as_ref().is_some_and(Properties::editable) =>
                    {
                        v.mode ^= bit;
                        v.bit = (0..9).find(|&i| 0o400 >> i == bit).unwrap_or(0);
                    }
                    Scroll(delta) => {
                        if v.chooser && v.error.is_none() {
                            v.selected = v
                                .selected
                                .saturating_add_signed(delta)
                                .min(v.visible.len().saturating_sub(1));
                        } else {
                            v.scroll = v.scroll.saturating_add_signed(delta).min(v.max_scroll);
                        }
                    }
                    _ => {}
                }
                v.revision += 1;
            }
        }
    }
    pub(super) fn details_scroll(&mut self, delta: isize) {
        if let Some(v) = &self.detail_view {
            self.detail_control(v.revision, DetailControl::Scroll(delta));
        }
    }
    pub(super) fn details_key(&mut self, press: KeyPress) -> bool {
        if !self.details_dialog() {
            return false;
        }
        self.filter_applications();
        let Some(v) = &self.detail_view else {
            return true;
        };
        let revision = v.revision;
        if press.key == Key::Escape {
            self.close_details();
            return true;
        }
        if self.settings.action_for(press) == Some(Action::Refresh) {
            self.detail_control(revision, DetailControl::Refresh);
            return true;
        }
        if press.key == Key::F(4) && !press.ctrl && !press.alt && !press.shift {
            self.detail_control(
                revision,
                if v.chooser {
                    DetailControl::All
                } else if v.permissions {
                    DetailControl::General
                } else {
                    DetailControl::Permissions
                },
            );
            return true;
        }
        if v.chooser {
            match press.key {
                Key::Up | Key::Down | Key::PageUp | Key::PageDown if !press.ctrl && !press.alt => {
                    self.details_scroll(match press.key {
                        Key::Up => -1,
                        Key::Down => 1,
                        Key::PageUp => -5,
                        _ => 5,
                    });
                    return true;
                }
                Key::Enter if !press.ctrl && !press.alt && !press.shift => {
                    self.detail_control(revision, DetailControl::Launch);
                    return true;
                }
                _ => return false,
            }
        }
        if self.settings.action_for(press) == Some(Action::Quit) {
            self.action(Action::Quit);
            return true;
        }
        if press.ctrl && press.key == Key::Char('s') && !press.alt {
            self.detail_control(revision, DetailControl::Apply);
            return true;
        }
        if press.ctrl || press.alt {
            return true;
        }
        let permissions = v.permissions;
        match press.key {
            Key::Tab => {
                let v = self.detail_view.as_mut().unwrap();
                v.bit = (v.bit + if press.shift { 8 } else { 1 }) % 9;
            }
            Key::Char(' ') if permissions => {
                let bit = 0o400 >> v.bit;
                self.detail_control(revision, DetailControl::Toggle(bit));
            }
            Key::Left | Key::Right if permissions => {
                let v = self.detail_view.as_mut().unwrap();
                v.bit = (v.bit + if press.key == Key::Left { 8 } else { 1 }) % 9;
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                self.details_scroll(match press.key {
                    Key::Up => -1,
                    Key::Down => 1,
                    Key::PageUp => -5,
                    _ => 5,
                })
            }
            Key::Enter if permissions => self.detail_control(revision, DetailControl::Apply),
            _ => {}
        }
        true
    }
}

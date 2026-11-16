use super::*;
use crate::{
    input::{Key, KeyPress},
    view::ViewMode,
};

pub struct ViewMenu {
    pub token: u64,
    pub owner: PathOwner,
    pub request: u64,
    pub selected: usize,
}
impl App {
    pub(super) fn duplicate_destination(&self) -> Option<PathBuf> {
        let sources = self.tab().targets();
        let parent = sources.first()?.parent()?;
        sources
            .iter()
            .all(|p| p.parent() == Some(parent))
            .then(|| parent.to_path_buf())
    }

    pub fn open_view_menu(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        self.finish_marquee(true);
        self.dialog = Some(Dialog::View(ViewMenu {
            token: self.token(),
            owner: PathOwner::current(self, self.active),
            request: self.panes[self.active].tab().request,
            selected: ViewMode::ALL
                .iter()
                .position(|v| *v == self.settings.view_mode())
                .unwrap(),
        }));
    }
    pub fn view_control(&mut self, token: u64, mode: Option<ViewMode>, delta: isize) {
        let Some(Dialog::View(menu)) = &self.dialog else {
            return;
        };
        if menu.token != token
            || !menu.owner.valid(self)
            || self.panes[menu.owner.pane].tab().request != menu.request
        {
            return;
        }
        if let Some(mode) = mode {
            self.dialog = None;
            self.select_view(mode);
        } else {
            self.settings.view_columns = self
                .settings
                .view_columns
                .saturating_add_signed(delta as i8)
                .clamp(1, 16);
            self.preferences_changed();
        }
    }
    pub fn select_view(&mut self, mode: ViewMode) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.settings.view = Some(mode);
        self.settings.grid = mode == ViewMode::Icons;
        let hidden = self.settings.hidden;
        for tab in self.panes.iter_mut().flat_map(|p| &mut p.tabs) {
            tab.enable_tree(mode == ViewMode::Tree, hidden);
            tab.offset = 0;
            tab.follow_selection = true;
            tab.column_rows = 0;
        }
        if let Some(trash) = &mut self.trash {
            trash.files.offset = 0;
            trash.files.column_rows = 0;
            trash.files.follow_selection = true;
        }
        self.hits.clear();
        self.drop_regions.clear();
        self.preferences_changed();
        self.update_preview();
    }
    pub(super) fn view_key(&mut self, press: KeyPress) -> bool {
        let Some(Dialog::View(menu)) = &mut self.dialog else {
            return false;
        };
        let token = menu.token;
        match press.key {
            Key::Escape => self.cancel_dialog(),
            Key::Up => menu.selected = menu.selected.saturating_sub(1),
            Key::Down => menu.selected = (menu.selected + 1).min(3),
            Key::Left => self.view_control(token, None, -1),
            Key::Right => self.view_control(token, None, 1),
            Key::Enter => {
                let mode = ViewMode::ALL[menu.selected];
                self.view_control(token, Some(mode), 0);
            }
            _ => {}
        }
        true
    }
    pub fn scroll_file_columns(&mut self, owner: &PathOwner, request: u64, delta: isize) {
        if self.dialog.is_none()
            && owner.valid(self)
            && self.panes[owner.pane].tab().request == request
            && self.settings.view_mode() == ViewMode::Columns
        {
            self.file_tab_mut(owner.pane).scroll_rows(delta);
        }
    }
}

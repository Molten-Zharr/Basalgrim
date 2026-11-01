use super::*;
use crate::{
    clipboard::{Destination, TextCopy},
    input::{Key, KeyPress},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathOwner {
    pub pane: usize,
    pub tab: usize,
    pub cwd: PathBuf,
}
impl PathOwner {
    pub fn current(app: &App, pane: usize) -> Self {
        Self {
            pane,
            tab: app.panes[pane].active,
            cwd: app.panes[pane].tab().cwd.clone(),
        }
    }
    pub fn valid(&self, app: &App) -> bool {
        app.panes.get(self.pane).is_some_and(|p| {
            p.active == self.tab && p.tabs.get(self.tab).is_some_and(|t| t.cwd == self.cwd)
        })
    }
}
pub struct PathMenu {
    pub owner: PathOwner,
    pub anchor: Position,
    pub parents: Vec<PathBuf>,
    pub selected: usize,
    pub offset: usize,
    pub capacity: usize,
}
/// Boundaries of the same graphemes Ratatui renders, expressed as UTF-8 offsets.
fn grapheme_offsets(text: &str) -> Vec<usize> {
    let span = ratatui::text::Span::raw(text);
    let mut offset = 0;
    let mut result = vec![0];
    for grapheme in span.styled_graphemes(ratatui::style::Style::default()) {
        offset += grapheme.symbol.len();
        result.push(offset);
    }
    result
}
impl App {
    pub fn take_text_copy(&mut self) -> Option<TextCopy> {
        self.text_copy.take()
    }
    pub fn text_copy_current(&self, id: u64) -> bool {
        self.text_copy_token == id
    }
    pub fn retry_text_copy(&mut self, request: TextCopy) {
        if self.text_copy_current(request.id) {
            self.text_copy = Some(request);
        }
    }
    pub fn finish_text_copy(&mut self, id: u64, result: Result<Destination, String>) {
        if self.text_copy_token != id {
            return;
        }
        self.text_copy_token = 0;
        match result {
            Ok(Destination::System) => self.message("Путь скопирован в буфер обмена", false),
            Ok(Destination::Terminal) => {
                self.message("Путь отправлен в буфер терминала (OSC 52)", false)
            }
            Err(error) => self.message(format!("Не удалось скопировать путь: {error}"), true),
        }
    }
    pub(super) fn request_text_copy(&mut self, text: String) {
        let id = self.token();
        self.text_copy_token = id;
        self.text_copy = Some(TextCopy { id, text });
        self.message("Копирование пути…", false);
    }
    pub fn copy_path(&mut self, path: &Path) {
        let Some(text) = path.to_str() else {
            self.text_copy_token = self.token();
            self.text_copy = None;
            self.message(
                "Путь содержит байты вне UTF-8; текстовый буфер не передаст его без искажения",
                true,
            );
            return;
        };
        self.request_text_copy(text.into());
    }
    pub(super) fn copy_path_text(&mut self) {
        if let Some(Dialog::Edit {
            kind: EditKind::Path(original),
            text,
            ..
        }) = &self.dialog
        {
            if text == &original.to_string_lossy() {
                self.copy_path(&original.clone());
            } else {
                self.request_text_copy(text.clone());
            }
        }
    }
    pub(super) fn edit_path(&mut self, owner: PathOwner) {
        if !owner.valid(self) {
            return;
        }
        self.active = owner.pane;
        self.path_text_selected = true;
        let text = if self.in_trash() {
            "trash:///".into()
        } else {
            owner.cwd.to_string_lossy().into_owned()
        };
        self.path_cursor = text.len();
        self.dialog = Some(Dialog::Edit {
            kind: EditKind::Path(owner.cwd.clone()),
            text,
            error: None,
        });
    }
    pub(super) fn position_path_cursor(&mut self, start: usize, column: u16, select_all: bool) {
        use unicode_width::UnicodeWidthStr;
        let Some(Dialog::Edit {
            kind: EditKind::Path(_),
            text,
            ..
        }) = &self.dialog
        else {
            return;
        };
        let boundaries = grapheme_offsets(text);
        let mut position = start.min(text.len());
        position = *boundaries
            .iter()
            .take_while(|&&i| i <= position)
            .last()
            .unwrap_or(&0);
        let mut width = 0;
        let first = position;
        for pair in boundaries.windows(2).filter(|pair| pair[0] >= first) {
            let w = text[pair[0]..pair[1]].width();
            if width + w > usize::from(column) {
                break;
            }
            width += w;
            position = pair[1];
        }
        self.path_cursor = position;
        self.path_text_selected = select_all;
    }
    pub(super) fn path_editor_key(&mut self, press: KeyPress) -> bool {
        if !matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::Path(_),
                ..
            })
        ) {
            return false;
        }
        if press == KeyPress::ctrl('c') {
            self.copy_path_text();
            return true;
        }
        if press == KeyPress::ctrl('a') {
            self.path_text_selected = true;
            return true;
        }
        if !press.ctrl && !press.alt && press.key == Key::Escape {
            self.cancel_dialog();
            return true;
        }
        if !press.ctrl && !press.alt && press.key == Key::Enter {
            self.confirm_dialog();
            return true;
        }
        let Some(Dialog::Edit { text, error, .. }) = &mut self.dialog else {
            unreachable!()
        };
        let cursor = self.path_cursor.min(text.len());
        let boundaries = grapheme_offsets(text);
        let previous = boundaries
            .iter()
            .copied()
            .take_while(|&i| i < cursor)
            .last()
            .unwrap_or(0);
        let next = boundaries
            .iter()
            .copied()
            .find(|&i| i > cursor)
            .unwrap_or(cursor);
        if press == KeyPress::ctrl('u')
            || (self.path_text_selected
                && !press.ctrl
                && !press.alt
                && matches!(press.key, Key::Backspace | Key::Delete))
        {
            text.clear();
            self.path_cursor = 0;
            self.path_text_selected = false;
            *error = None;
        } else if !press.ctrl && !press.alt {
            match press.key {
                Key::Left => self.path_cursor = if self.path_text_selected { 0 } else { previous },
                Key::Right => {
                    self.path_cursor = if self.path_text_selected {
                        text.len()
                    } else {
                        next
                    }
                }
                Key::Home => self.path_cursor = 0,
                Key::End => self.path_cursor = text.len(),
                Key::Backspace => {
                    text.replace_range(previous..cursor, "");
                    self.path_cursor = previous;
                }
                Key::Delete => {
                    text.replace_range(cursor..next, "");
                }
                _ => return true,
            }
            self.path_text_selected = false;
            *error = None;
        }
        true
    }
    pub(super) fn go_path(&mut self, owner: PathOwner, path: PathBuf) {
        if !owner.valid(self) {
            return;
        }
        self.active = owner.pane;
        if self.tab().cwd != path || self.in_trash() {
            self.navigate(path, true);
        } else {
            self.edit_path(owner);
        }
    }
    pub(super) fn path_parents(&mut self, owner: PathOwner, anchor: Position) {
        if !owner.valid(self) {
            return;
        }
        let mut parents: Vec<_> = owner
            .cwd
            .ancestors()
            .skip(1)
            .map(Path::to_path_buf)
            .collect();
        parents.reverse();
        let selected = parents.len().saturating_sub(1);
        self.dialog = Some(Dialog::Paths(PathMenu {
            owner,
            anchor,
            parents,
            selected,
            offset: 0,
            capacity: 1,
        }));
    }
    pub(super) fn choose_parent(&mut self, index: usize) {
        let Some(Dialog::Paths(menu)) = self.dialog.take() else {
            return;
        };
        if let Some(path) = menu.parents.get(index) {
            self.go_path(menu.owner, path.clone());
        }
    }
    pub(super) fn copy_menu_path(&mut self) {
        let Some(Dialog::Paths(menu)) = self.dialog.take() else {
            return;
        };
        if menu.owner.valid(self) {
            self.copy_path(&menu.owner.cwd);
        }
    }
    pub(super) fn edit_menu_path(&mut self) {
        let Some(Dialog::Paths(menu)) = self.dialog.take() else {
            return;
        };
        self.edit_path(menu.owner);
    }
    pub(super) fn scroll_paths(&mut self, delta: isize) {
        if let Some(Dialog::Paths(menu)) = &mut self.dialog {
            menu.selected = menu
                .selected
                .saturating_add_signed(delta)
                .min(menu.parents.len().saturating_sub(1));
        }
    }
    pub(super) fn path_menu_key(&mut self, press: KeyPress) {
        if self.settings.action_for(press) == Some(Action::CopyPath) || press == KeyPress::ctrl('c')
        {
            self.copy_menu_path();
            return;
        }
        if self.settings.action_for(press) == Some(Action::Quit) {
            self.action(Action::Quit);
            return;
        }
        if press.ctrl || press.alt || press.shift {
            return;
        }
        let Some(Dialog::Paths(menu)) = &mut self.dialog else {
            return;
        };
        match press.key {
            Key::Escape => self.cancel_dialog(),
            Key::Enter => {
                let index = menu.selected;
                self.choose_parent(index);
            }
            Key::Up => self.scroll_paths(-1),
            Key::Down => self.scroll_paths(1),
            Key::Home => menu.selected = 0,
            Key::End => menu.selected = menu.parents.len().saturating_sub(1),
            Key::PageUp => {
                let page = menu.capacity as isize;
                self.scroll_paths(-page);
            }
            Key::PageDown => {
                let page = menu.capacity as isize;
                self.scroll_paths(page);
            }
            _ => {}
        }
    }
}

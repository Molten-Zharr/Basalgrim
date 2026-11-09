use super::*;
use crate::{
    input::{Input, Key, KeyPress},
    text_edit::TextEdit,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextCommand {
    Cut,
    Copy,
    Paste,
    SelectAll,
}
impl TextCommand {
    pub const ALL: [Self; 4] = [Self::Cut, Self::Copy, Self::Paste, Self::SelectAll];
    pub fn label(self) -> &'static str {
        match self {
            Self::Cut => "Вырезать",
            Self::Copy => "Копировать",
            Self::Paste => "Вставить",
            Self::SelectAll => "Выделить всё",
        }
    }
}

impl App {
    pub(super) fn begin_edit(
        &mut self,
        kind: EditKind,
        text: String,
        selection: std::ops::Range<usize>,
    ) {
        self.editor.reset(&text, selection);
        self.edit_owner = Some(PathOwner::current(self, self.active));
        self.editor_menu = None;
        self.editor_drag = None;
        self.text_paste = None;
        self.text_paste_fallback = None;
        self.dialog = Some(Dialog::Edit {
            kind,
            text,
            error: None,
        });
    }
    pub fn text_paste_pending(&self) -> bool {
        self.text_active()
            && self
                .text_paste
                .is_some_and(|(_, revision)| revision == self.editor.revision)
    }
    pub fn take_text_paste_fallback(&mut self) -> Option<u64> {
        self.text_paste_fallback.take().filter(|id| {
            self.text_paste_pending() && self.text_paste.is_some_and(|(current, _)| current == *id)
        })
    }
    pub(super) fn request_text_paste(&mut self) {
        let id = self.token();
        self.text_paste = Some((id, self.editor.revision));
        if self.native_text_paste() {
            self.queue_text_read(id);
        } else {
            self.text_paste_fallback = Some(id);
        }
    }
    pub fn finish_text_paste(&mut self, id: u64, result: Result<String, String>) {
        let Some((current, revision)) = self.text_paste else {
            return;
        };
        if current != id {
            return;
        }
        self.text_paste = None;
        self.text_paste_fallback = None;
        if !self.text_active() || revision != self.editor.revision {
            return;
        }
        match result {
            Ok(text) => self.insert_text(&text),
            Err(error) => self.message(error, true),
        }
    }
    pub fn gui_pasted_text(&mut self, text: String) {
        if let Some((id, _)) = self.text_paste {
            self.finish_text_paste(id, Ok(text));
        } else if self.text_active() {
            self.insert_text(&text);
        }
    }
    pub(crate) fn validate_editor(&mut self) {
        if matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::Rename(_) | EditKind::BatchRename,
                ..
            })
        ) && self
            .edit_owner
            .as_ref()
            .is_none_or(|owner| !owner.valid(self) || owner.pane != self.active)
        {
            self.cancel_dialog();
            self.message(
                "Переименование отменено: папка или вкладка изменились",
                true,
            );
        }
        if !self.text_active() {
            self.editor_menu = None;
            self.editor_drag = None;
            self.text_paste = None;
        }
    }
    pub(crate) fn renaming(&self, pane: usize, path: &Path) -> bool {
        self.edit_owner
            .as_ref()
            .is_some_and(|o| o.pane == pane && o.valid(self))
            && matches!(&self.dialog, Some(Dialog::Edit { kind: EditKind::Rename(source), .. }) if source == path)
    }
    pub(super) fn position_text_cursor(
        &mut self,
        start: usize,
        column: u16,
        select_all: bool,
        extend: bool,
    ) {
        if let Some(Dialog::Edit { text, .. }) = &self.dialog {
            if select_all {
                self.editor.select_all(text);
            } else {
                self.editor
                    .move_to(text, TextEdit::position(text, start, column), extend);
            }
        }
    }
    pub(super) fn editor_pointer(&mut self, input: &Input) -> bool {
        if !self.text_active() {
            return false;
        }
        match *input {
            Input::PointerDown { x, y, double, .. } => {
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains((x, y).into()))
                    .map(|(_, t)| t.clone());
                if let Some(Target::TextSelect { area, start }) = target {
                    self.position_text_cursor(
                        start,
                        x.saturating_sub(area.x),
                        double,
                        self.selection_extend,
                    );
                    self.editor_drag = (!double).then_some(area);
                    return true;
                }
            }
            Input::Drag { x, .. } | Input::PointerUp { x, .. } if self.editor_drag.is_some() => {
                let area = self.editor_drag.unwrap();
                if let Some(start) = self.hits.iter().find_map(|(_, t)| match t {
                    Target::TextSelect {
                        area: current,
                        start,
                    } if *current == area => Some(*start),
                    _ => None,
                }) {
                    self.position_text_cursor(
                        start,
                        x.saturating_sub(area.x).min(area.width),
                        false,
                        true,
                    );
                }
                if matches!(input, Input::PointerUp { .. }) {
                    self.editor_drag = None;
                }
                return true;
            }
            Input::CancelDrag => {
                self.editor_drag = None;
                self.editor_menu = None;
                self.text_paste = None;
            }
            Input::Scroll { delta, .. } => {
                if self.batch_editing() && self.editor_menu.is_none() {
                    self.scroll_batch(delta as isize * 3);
                }
                return true;
            }
            _ => {}
        }
        false
    }
    pub(super) fn editor_command(&mut self, command: TextCommand) {
        self.editor_menu = None;
        let Some(Dialog::Edit { kind, text, .. }) = &self.dialog else {
            return;
        };
        let selected = self.editor.selection(text);
        match command {
            TextCommand::SelectAll => self.editor.select_all(text),
            TextCommand::Paste => self.request_text_paste(),
            TextCommand::Copy | TextCommand::Cut if !selected.is_empty() => {
                if let EditKind::Path(original) = kind
                    && selected == (0..text.len())
                    && text == &original.to_string_lossy()
                    && original.to_str().is_none()
                {
                    self.copy_path(&original.clone());
                    return;
                }
                self.request_text_copy(text[selected].to_owned());
                if command == TextCommand::Cut
                    && let Some(Dialog::Edit { text, error, .. }) = &mut self.dialog
                {
                    self.editor.delete(text, true, false);
                    *error = None;
                    self.live_search();
                }
            }
            _ => {}
        }
    }
    pub(super) fn editor_key(&mut self, press: KeyPress) -> bool {
        if !self.text_active() {
            return false;
        }
        if press.key == Key::F(10) && press.shift && !press.ctrl && !press.alt {
            let anchor = self
                .hits
                .iter()
                .find_map(|(r, t)| {
                    matches!(t, Target::TextSelect { .. }).then_some((r.x, r.y + 1).into())
                })
                .unwrap_or_default();
            self.editor_menu = Some((anchor, 0));
            return true;
        }
        if let Some((_, selected)) = &mut self.editor_menu {
            match press.key {
                Key::Escape => {
                    self.editor_menu = None;
                    return true;
                }
                Key::Up => {
                    *selected = selected.saturating_sub(1);
                    return true;
                }
                Key::Down => {
                    *selected = (*selected + 1).min(3);
                    return true;
                }
                Key::Enter => {
                    let command = TextCommand::ALL[*selected];
                    self.editor_command(command);
                    return true;
                }
                _ => self.editor_menu = None,
            }
        }
        if self.batch_editing() && !press.ctrl && !press.alt {
            let page = self.batch.as_ref().map_or(1, |v| v.page) as isize;
            let delta = match press.key {
                Key::Up => Some(-1),
                Key::Down => Some(1),
                Key::PageUp => Some(-page),
                Key::PageDown => Some(page),
                _ => None,
            };
            if let Some(delta) = delta {
                self.scroll_batch(delta);
                return true;
            }
            if press.key == Key::F(5) {
                self.update_batch_preview(true);
                return true;
            }
            if !press.shift && press.key == Key::F(4) {
                self.toggle_batch_mode();
                return true;
            }
            if !press.shift
                && press.key == Key::F(3)
                && self.batch.as_ref().is_some_and(|view| !view.advanced)
            {
                self.number_batch();
                return true;
            }
        }
        let command = [
            (KeyPress::ctrl('a'), TextCommand::SelectAll),
            (KeyPress::ctrl('c'), TextCommand::Copy),
            (KeyPress::ctrl('x'), TextCommand::Cut),
            (KeyPress::ctrl('v'), TextCommand::Paste),
        ]
        .into_iter()
        .find_map(|(key, command)| (press == key).then_some(command));
        let command = command.or(match press {
            KeyPress {
                key: Key::Insert,
                shift: true,
                ctrl: false,
                alt: false,
            } => Some(TextCommand::Paste),
            KeyPress {
                key: Key::Insert,
                ctrl: true,
                shift: false,
                alt: false,
            } => Some(TextCommand::Copy),
            KeyPress {
                key: Key::Delete,
                shift: true,
                ctrl: false,
                alt: false,
            } => Some(TextCommand::Cut),
            _ => None,
        });
        if let Some(command) = command {
            self.editor_command(command);
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
        if self.settings.action_for(press) == Some(Action::Quit) {
            self.action(Action::Quit);
            return true;
        }
        if (press.ctrl || press.alt)
            && self.settings.action_for(press) == Some(Action::CancelOperation)
        {
            self.cancel_operation();
            return true;
        }
        let Some(Dialog::Edit { text, error, .. }) = &mut self.dialog else {
            unreachable!()
        };
        if press.alt {
            return true;
        }
        let selected = self.editor.selection(text);
        let position = match press.key {
            Key::Home => Some(0),
            Key::End => Some(text.len()),
            Key::Left if !press.shift && !press.ctrl && !selected.is_empty() => {
                Some(selected.start)
            }
            Key::Right if !press.shift && !press.ctrl && !selected.is_empty() => Some(selected.end),
            Key::Left => Some(if press.ctrl {
                self.editor.word(text, false)
            } else {
                self.editor.previous(text)
            }),
            Key::Right => Some(if press.ctrl {
                self.editor.word(text, true)
            } else {
                self.editor.next(text)
            }),
            _ => None,
        };
        if let Some(position) = position {
            self.editor.move_to(text, position, press.shift);
        } else if press == KeyPress::ctrl('u') {
            text.clear();
            self.editor.reset(text, 0..0);
            *error = None;
        } else if matches!(press.key, Key::Backspace | Key::Delete) {
            self.editor
                .delete(text, press.key == Key::Backspace, press.ctrl);
            *error = None;
        }
        self.live_search();
        true
    }
}

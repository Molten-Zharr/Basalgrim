use super::*;
use crate::input::{Input, Key, KeyPress};
use ratatui::layout::Position;

impl App {
    pub fn handle(&mut self, input: Input) {
        match input {
            Input::PointerDown {
                x,
                y,
                double,
                toggle,
            } => {
                self.primary_down(x, y, double, toggle);
                return;
            }
            Input::Drag { x, y } => {
                self.drag_selection(x, y);
                return;
            }
            Input::PointerUp { x, y } => {
                self.drag_selection(x, y);
                self.finish_marquee(false);
                return;
            }
            Input::CancelDrag => {
                self.finish_marquee(true);
                return;
            }
            _ => {}
        }
        if matches!(
            input,
            Input::Key(_) | Input::SecondaryClick { .. } | Input::Scroll { .. }
        ) {
            if self.marquee.is_some()
                && matches!(
                    input,
                    Input::Key(KeyPress {
                        key: Key::Escape,
                        ..
                    })
                )
            {
                self.finish_marquee(true);
                return;
            }
            self.finish_marquee(false);
        }
        if let Input::Click { x, y, .. }
        | Input::SecondaryClick { x, y }
        | Input::Scroll { x, y, .. } = &input
        {
            self.pointer = Some(Position::new(*x, *y));
        }
        match input {
            Input::Pointer(position) => {
                self.pointer = position;
                return;
            }
            Input::Text(text) => self.insert_text(&text),
            Input::Key(key) => self.key(key),
            Input::SecondaryClick { .. } if matches!(self.dialog, Some(Dialog::Commands(_))) => {
                self.dialog = None;
            }
            Input::SecondaryClick { x, y } if self.dialog.is_none() => {
                let point = Position::new(x, y);
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(point))
                    .map(|(_, t)| t.clone());
                if let Some(Target::Entry { pane, index } | Target::EntryMark { pane, index }) =
                    target
                {
                    self.active = pane;
                    self.tab_mut().selected = index;
                    if self
                        .tab()
                        .entry()
                        .is_some_and(|e| !self.tab().marked.contains(&e.path))
                    {
                        self.tab_mut().marked.clear();
                        self.tab_mut().explicit_selection = false;
                    }
                    self.open_commands(point, MenuContext::Entries);
                } else if let Some(pane) = self.pane_areas.iter().position(|r| r.contains(point)) {
                    self.active = pane;
                    self.open_commands(point, MenuContext::Directory);
                }
            }
            Input::Click {
                x,
                y,
                double,
                toggle,
            } => {
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(Position::new(x, y)))
                    .map(|(_, t)| t.clone());
                match target {
                    Some(Target::DismissMenu)
                        if matches!(self.dialog, Some(Dialog::Commands(_))) =>
                    {
                        self.cancel_dialog()
                    }
                    Some(Target::Command(index)) => self.choose_command(index),
                    Some(Target::CommandScroll(delta))
                        if matches!(self.dialog, Some(Dialog::Commands(_))) =>
                    {
                        self.scroll_commands(delta)
                    }
                    Some(Target::Operation(index))
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.operations_selected =
                            index.min(self.operations.len().saturating_sub(1));
                        self.operations_detail_offset = 0;
                    }
                    Some(Target::Action(Action::CancelOperation))
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.cancel_operation()
                    }
                    Some(Target::Confirm) => self.confirm_dialog(),
                    Some(Target::Cancel) => self.cancel_dialog(),
                    Some(Target::SettingsSection(section))
                        if matches!(self.dialog, Some(Dialog::Settings)) =>
                    {
                        self.settings_section(section)
                    }
                    Some(Target::SettingsTab(tab))
                        if matches!(self.dialog, Some(Dialog::Settings)) =>
                    {
                        self.settings_tab(tab)
                    }
                    Some(Target::Setting {
                        index,
                        activate,
                        direction,
                    }) if matches!(self.dialog, Some(Dialog::Settings)) => {
                        self.setting(index, activate, direction)
                    }
                    Some(Target::RetrySave) => {
                        self.settings_save_error = None;
                        self.settings_dirty = true;
                    }
                    Some(Target::BindingModifier(index))
                        if matches!(self.dialog, Some(Dialog::CaptureBinding { .. })) =>
                    {
                        self.settings_view.modifiers[index] = !self.settings_view.modifiers[index]
                    }
                    Some(Target::BindingKey(key)) => {
                        if let Some(Dialog::CaptureBinding { index, .. }) = self.dialog {
                            let [ctrl, alt, shift] = self.settings_view.modifiers;
                            self.capture_binding(
                                index,
                                KeyPress {
                                    key,
                                    ctrl,
                                    alt,
                                    shift,
                                },
                            );
                        }
                    }
                    _ if self.dialog.is_some() => {}
                    Some(Target::Action(a)) => self.action(a),
                    Some(Target::Place(path)) => self.navigate(path, true),
                    Some(Target::Tab { pane, index }) => {
                        self.active = pane;
                        self.panes[pane].active = index;
                    }
                    Some(Target::Entry { pane, index }) => {
                        self.active = pane;
                        if !toggle && self.tab().explicit_selection {
                            self.tab_mut().marked.clear();
                            self.tab_mut().explicit_selection = false;
                        }
                        self.tab_mut().selected = index;
                        if toggle {
                            self.toggle_mark();
                        } else if double {
                            self.open_selected();
                        }
                    }
                    Some(Target::EntryMark { pane, index }) => {
                        self.active = pane;
                        self.tab_mut().selected = index;
                        self.toggle_mark();
                    }
                    _ => {}
                }
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Commands(_))) => {
                self.scroll_commands(delta as isize * 3)
            }
            Input::Scroll { x, y, delta } if matches!(self.dialog, Some(Dialog::Operations)) => {
                if self.operations_details_area.contains(Position::new(x, y)) {
                    self.operations_detail_offset = self
                        .operations_detail_offset
                        .saturating_add_signed(delta as isize * 3)
                        .min(self.operations_detail_max);
                } else {
                    self.operations_selected = self
                        .operations_selected
                        .saturating_add_signed(delta as isize)
                        .min(self.operations.len().saturating_sub(1));
                    self.operations_detail_offset = 0;
                }
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Help)) => {
                self.help_offset = self
                    .help_offset
                    .saturating_add_signed(delta as isize * 3)
                    .min(self.help_max);
            }
            Input::Scroll { x, y, delta } if matches!(self.dialog, Some(Dialog::Settings)) => {
                if self
                    .settings_view
                    .parameters_area
                    .contains(Position::new(x, y))
                {
                    self.settings_view.focus = SettingsFocus::Parameters;
                } else if self.settings_view.tabs_area.contains(Position::new(x, y)) {
                    self.settings_view.focus = SettingsFocus::Tabs;
                } else {
                    self.settings_view.focus = SettingsFocus::Menu;
                }
                self.settings_move(delta as isize);
            }
            Input::Scroll { delta, .. }
                if matches!(self.dialog, Some(Dialog::CaptureBinding { .. })) =>
            {
                let max = crate::input::shortcut_choices()
                    .len()
                    .saturating_sub(self.settings_view.binding_capacity.max(1));
                self.settings_view.binding_offset = self
                    .settings_view
                    .binding_offset
                    .saturating_add_signed(delta as isize * 5)
                    .min(max);
            }
            Input::Scroll { x, y, delta } if self.dialog.is_none() => {
                if self.preview_area.contains(Position::new(x, y)) {
                    let max = self
                        .preview
                        .as_ref()
                        .map_or(0, |p| p.lines.len().saturating_sub(self.preview_page));
                    self.preview_offset = self
                        .preview_offset
                        .saturating_add_signed(delta as isize * 3)
                        .min(max);
                } else {
                    if let Some((_, Target::Entry { pane, .. })) =
                        self.hits.iter().find(|(r, t)| {
                            r.contains(Position::new(x, y)) && matches!(t, Target::Entry { .. })
                        })
                    {
                        self.active = *pane;
                    }
                    self.move_selection(delta as isize * self.tab().columns as isize);
                }
            }
            _ => {}
        }
        self.update_preview();
    }
    fn insert_text(&mut self, text: &str) {
        if let Some(Dialog::Edit {
            text: value, error, ..
        }) = &mut self.dialog
        {
            let remaining = 4096usize.saturating_sub(value.chars().count());
            value.extend(text.chars().filter(|c| !c.is_control()).take(remaining));
            *error = None;
        }
        self.live_search();
    }
    fn live_search(&mut self) {
        if let Some(Dialog::Edit {
            kind: EditKind::Search { .. },
            text,
            ..
        }) = &self.dialog
        {
            self.panes[self.active].tab_mut().filter = text.clone();
            let hidden = self.settings.hidden;
            self.tab_mut().rebuild(hidden);
        }
    }
    fn move_selection(&mut self, delta: isize) {
        let tab = self.tab_mut();
        if tab.explicit_selection {
            tab.marked.clear();
            tab.explicit_selection = false;
        }
        tab.selected = tab
            .selected
            .saturating_add_signed(delta)
            .min(tab.visible.len().saturating_sub(1));
    }
    pub(super) fn toggle_mark(&mut self) {
        if let Some(path) = self.tab().entry().map(|e| e.path.clone())
            && !self.tab_mut().marked.remove(&path)
        {
            self.tab_mut().marked.insert(path);
        }
    }
    fn key(&mut self, press: KeyPress) {
        let press = press.normalized();
        if let Some(Dialog::CaptureBinding { index, .. }) = self.dialog {
            if press.key == Key::Escape {
                self.dialog = Some(Dialog::Settings);
            } else {
                self.capture_binding(index, press);
            }
            return;
        }
        if matches!(self.dialog, Some(Dialog::Settings)) {
            self.settings_key(press);
            return;
        }
        if matches!(self.dialog, Some(Dialog::Commands(_))) {
            self.commands_key(press);
            return;
        }
        if matches!(self.dialog, Some(Dialog::Operations)) {
            self.operations_key(press);
            return;
        }
        let KeyPress {
            key,
            ctrl,
            alt,
            shift,
        } = press;
        if self.settings.action_for(press) == Some(Action::CancelOperation)
            && (self.dialog.is_none() || ctrl || alt)
        {
            self.cancel_operation();
            return;
        }
        if self.settings.action_for(press) == Some(Action::Quit)
            && (self.dialog.is_none() || ctrl || alt)
        {
            self.action(Action::Quit);
            return;
        }
        if self.dialog.is_some() {
            if matches!(self.dialog, Some(Dialog::Help)) {
                let delta = match key {
                    Key::Down => 1,
                    Key::Up => -1,
                    Key::PageDown => 10,
                    Key::PageUp => -10,
                    _ => 0,
                };
                self.help_offset = self
                    .help_offset
                    .saturating_add_signed(delta)
                    .min(self.help_max);
                if key == Key::Home {
                    self.help_offset = 0;
                }
                if key == Key::End {
                    self.help_offset = self.help_max;
                }
            }
            match key {
                Key::Escape => self.cancel_dialog(),
                Key::Enter => self.confirm_dialog(),
                Key::Backspace => {
                    if let Some(Dialog::Edit { text, error, .. }) = &mut self.dialog {
                        text.pop();
                        *error = None;
                    }
                    self.live_search();
                }
                Key::Char('u') if ctrl => {
                    if let Some(Dialog::Edit { text, .. }) = &mut self.dialog {
                        text.clear();
                    }
                    self.live_search();
                }
                _ => {}
            }
            return;
        }
        let action = self.settings.action_for(press);
        if let Some(a) = action {
            self.action(a);
            return;
        }
        let columns = self.tab().columns as isize;
        if ctrl || alt || shift {
            return;
        }
        match key {
            Key::Up => self.move_selection(-columns),
            Key::Down => self.move_selection(columns),
            Key::Left if self.settings.grid => self.move_selection(-1),
            Key::Right if self.settings.grid => self.move_selection(1),
            Key::Home => self.move_selection(isize::MIN),
            Key::End => self.move_selection(isize::MAX),
            Key::PageUp => self.move_selection(-(self.tab().page_size as isize)),
            Key::PageDown => self.move_selection(self.tab().page_size as isize),
            Key::Escape => {
                self.tab_mut().marked.clear();
                self.tab_mut().filter.clear();
                let h = self.settings.hidden;
                self.tab_mut().rebuild(h);
            }
            _ => {}
        }
    }
}

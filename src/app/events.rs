use super::*;
use crate::input::{Input, Key, KeyPress};
use ratatui::layout::Position;

impl App {
    pub fn handle(&mut self, input: Input) {
        if self.terminal_input(&input) {
            return;
        }
        if matches!(
            input,
            Input::Key(KeyPress {
                key: Key::Escape,
                ..
            })
        ) && self.external_drag_active()
        {
            self.cancel_external_drop();
            self.cancel_file_drag();
            self.finish_marquee(true);
            return;
        }
        if let Input::SelectionModifier(extend) = input {
            self.selection_extend = extend;
            return;
        }
        if self.editor_pointer(&input) {
            return;
        }
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
                if self.file_drag.is_some() {
                    self.move_file_drag(x, y);
                } else {
                    self.drag_selection(x, y);
                }
                return;
            }
            Input::PointerUp { x, y } => {
                if self.file_drag.is_some() {
                    self.release_file_drag(x, y);
                    return;
                }
                self.drag_selection(x, y);
                self.finish_marquee(false);
                return;
            }
            Input::CancelDrag => {
                self.cancel_file_drag();
                self.finish_marquee(true);
                return;
            }
            Input::DragModifiers { copy } => {
                self.set_drag_copy(copy);
                return;
            }
            _ => {}
        }
        if matches!(
            input,
            Input::Key(_) | Input::SecondaryClick { .. } | Input::Scroll { .. }
        ) {
            if self.file_drag.is_some() && !matches!(input, Input::Scroll { .. }) {
                let stop = matches!(
                    input,
                    Input::Key(KeyPress {
                        key: Key::Escape,
                        ..
                    }) | Input::SecondaryClick { .. }
                );
                self.cancel_file_drag();
                if stop {
                    return;
                }
            }
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
            Input::Text(text) | Input::Paste(text) => self.insert_text(&text),
            Input::Key(key) => self.key(key),
            Input::SecondaryClick { .. }
                if matches!(
                    self.dialog,
                    Some(
                        Dialog::Commands(_)
                            | Dialog::Paths(_)
                            | Dialog::Edit {
                                kind: EditKind::Path(_),
                                ..
                            }
                    )
                ) =>
            {
                self.dialog = None;
            }
            Input::SecondaryClick { x, y } if self.text_active() => {
                let point = Position::new(x, y);
                if self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(point))
                    .is_some_and(|(_, t)| matches!(t, Target::TextSelect { .. }))
                {
                    self.editor_drag = None;
                    self.editor_menu = Some((point, 0));
                }
            }
            Input::SecondaryClick { x, y } if self.dialog.is_none() => {
                let point = Position::new(x, y);
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(point))
                    .map(|(_, t)| t.clone());
                if let Some(
                    Target::PathGo { owner, .. }
                    | Target::PathCopy(owner)
                    | Target::PathEdit(owner)
                    | Target::PathParents { owner, .. },
                ) = &target
                {
                    self.path_parents(owner.clone(), point);
                    return;
                }
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
                        self.tab_mut().reset_range();
                    }
                    self.open_commands(
                        point,
                        if self.in_trash() {
                            MenuContext::Trash
                        } else {
                            MenuContext::Entries
                        },
                    );
                } else if let Some(pane) = self.pane_areas.iter().position(|r| r.contains(point)) {
                    self.active = pane;
                    self.open_commands(
                        point,
                        if self.in_trash() {
                            MenuContext::TrashBackground
                        } else {
                            MenuContext::Directory
                        },
                    );
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
                    Some(Target::DeviceRow { revision, index }) => {
                        self.device_select(revision, index, double)
                    }
                    Some(Target::DeviceControl { revision, control }) => {
                        self.device_control(revision, control)
                    }
                    Some(Target::DeviceOpen { revision, key }) if self.dialog.is_none() => {
                        self.open_device(revision, key)
                    }
                    Some(Target::PlacesSection(section))
                        if matches!(self.dialog, None | Some(Dialog::Places)) =>
                    {
                        self.open_places(section)
                    }
                    Some(Target::PlacesRow { revision, index }) => {
                        self.place_select(revision, index, double)
                    }
                    Some(Target::PlacesControl { revision, control }) => {
                        self.place_control(revision, control)
                    }
                    Some(Target::FolderMeasure {
                        owner,
                        request,
                        path,
                    }) if self.dialog.is_none()
                        && owner.valid(self)
                        && self.panes[owner.pane].tab().request == request =>
                    {
                        self.open_folder_size(path)
                    }
                    Some(Target::FolderStop(id))
                        if matches!(self.dialog, Some(Dialog::FolderSize))
                            && id == self.folder_sizes.id =>
                    {
                        self.folder_sizes.stop()
                    }
                    Some(Target::FolderRetry(id))
                        if matches!(self.dialog, Some(Dialog::FolderSize))
                            && id == self.folder_sizes.id =>
                    {
                        self.retry_folder_size()
                    }
                    Some(Target::FolderScroll(delta))
                        if matches!(self.dialog, Some(Dialog::FolderSize)) =>
                    {
                        self.scroll_folder_size(delta)
                    }
                    Some(Target::RecursiveSearch)
                        if matches!(
                            self.dialog,
                            Some(Dialog::Edit {
                                kind: EditKind::Search { .. },
                                ..
                            })
                        ) =>
                    {
                        self.begin_recursive_search()
                    }
                    Some(Target::SearchRetry) => {
                        if let Some(view) = &mut self.search {
                            view.query.clear();
                        }
                        self.update_recursive_search();
                    }
                    Some(Target::SearchStop) => {
                        if let Some(view) = &mut self.search {
                            view.worker.stop();
                        }
                    }
                    Some(Target::SearchMove(delta)) => self.search_move(delta),
                    Some(Target::SearchOpen(id, index)) => {
                        if self.search.as_ref().is_some_and(|s| s.worker.id == id) {
                            self.search_open(index);
                        }
                    }
                    Some(Target::BatchNumber) if self.batch_editing() => self.number_batch(),
                    Some(Target::BatchMode) if self.batch_editing() => self.toggle_batch_mode(),
                    Some(Target::BatchRefresh) if self.batch_editing() => {
                        self.update_batch_preview(true)
                    }
                    Some(Target::BatchScroll(delta)) if self.batch_editing() => {
                        self.scroll_batch(delta)
                    }
                    Some(Target::JournalRefresh)
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.refresh_journal()
                    }
                    Some(Target::ConflictChoice(choice)) => self.choose_conflict(choice),
                    Some(Target::ConflictRemaining) => self.toggle_conflict_remaining(),
                    Some(Target::PathParent(index))
                        if matches!(self.dialog, Some(Dialog::Paths(_))) =>
                    {
                        self.choose_parent(index)
                    }
                    Some(Target::PathScroll(delta))
                        if matches!(self.dialog, Some(Dialog::Paths(_))) =>
                    {
                        self.scroll_paths(delta)
                    }
                    Some(Target::PathMenuCopy) if matches!(self.dialog, Some(Dialog::Paths(_))) => {
                        self.copy_menu_path()
                    }
                    Some(Target::PathMenuEdit) if matches!(self.dialog, Some(Dialog::Paths(_))) => {
                        self.edit_menu_path()
                    }
                    Some(Target::PathTextCopy) => self.copy_path_text(),
                    Some(Target::TextSelect { area, start }) if self.text_active() => self
                        .position_text_cursor(
                            start,
                            x.saturating_sub(area.x),
                            double,
                            self.selection_extend,
                        ),
                    Some(Target::TextCommand(command)) if self.text_active() => {
                        self.editor_command(command)
                    }
                    Some(Target::TextMenuDismiss) => self.editor_menu = None,
                    Some(Target::DismissMenu)
                        if matches!(
                            self.dialog,
                            Some(Dialog::Commands(_) | Dialog::Paths(_) | Dialog::Edit { .. })
                        ) =>
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
                    Some(Target::Action(Action::HistoryStorage))
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.open_history_storage()
                    }
                    Some(Target::StorageRefresh)
                        if matches!(self.dialog, Some(Dialog::HistoryStorage)) =>
                    {
                        self.refresh_history_storage()
                    }
                    Some(Target::StoragePolicy)
                        if matches!(self.dialog, Some(Dialog::HistoryStorage)) =>
                    {
                        self.cycle_storage_policy()
                    }
                    Some(Target::StorageClear)
                        if matches!(self.dialog, Some(Dialog::HistoryStorage)) =>
                    {
                        self.confirm_history_cleanup()
                    }
                    Some(Target::Action(Action::CancelOperation))
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.cancel_operation()
                    }
                    Some(Target::Action(action @ (Action::Undo | Action::Redo)))
                        if matches!(self.dialog, Some(Dialog::Operations)) =>
                    {
                        self.action(action)
                    }
                    Some(Target::Confirm) => self.confirm_dialog(),
                    Some(Target::ArchiveFormat(format)) => self.archive_format(format),
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
                    Some(Target::PathGo { owner, path }) => self.go_path(owner, path),
                    Some(Target::PathCopy(owner)) if owner.valid(self) => {
                        if self.trash_in(owner.pane) {
                            self.request_text_copy("trash:///".into());
                        } else {
                            self.copy_path(&owner.cwd);
                        }
                    }
                    Some(Target::PathEdit(owner)) => self.edit_path(owner),
                    Some(Target::PathParents { owner, anchor }) => self.path_parents(owner, anchor),
                    Some(Target::Action(a)) => {
                        if let Some(pane) = self
                            .pane_areas
                            .iter()
                            .position(|r| r.contains(Position::new(x, y)))
                        {
                            self.active = pane;
                        }
                        self.action(a);
                    }
                    Some(Target::Place(path)) => self.navigate(path, true),
                    Some(Target::SavedPlace(path)) => self.place_open(path, false),
                    Some(Target::CloseTab { owner, request })
                        if owner.valid(self)
                            && (self.settings.dual || owner.pane == self.active)
                            && self.panes[owner.pane].tabs.len() > 1
                            && self.panes[owner.pane].tab().request == request =>
                    {
                        self.active = owner.pane;
                        self.action(Action::CloseTab);
                    }
                    Some(Target::Tab { pane, index })
                        if self.panes.get(pane).is_some_and(|p| index < p.tabs.len())
                            && (self.settings.dual || pane == self.active) =>
                    {
                        self.active = pane;
                        self.panes[pane].active = index;
                    }
                    Some(Target::NewTab { pane })
                        if pane < self.panes.len()
                            && (self.settings.dual || pane == self.active) =>
                    {
                        self.active = pane;
                        self.action(Action::NewTab);
                    }
                    Some(Target::Entry { pane, index }) => {
                        self.select_entry(pane, index, toggle, double, self.selection_extend);
                    }
                    Some(Target::EntryMark { pane, index }) => {
                        self.select_entry(
                            pane,
                            index,
                            toggle || !self.selection_extend,
                            false,
                            self.selection_extend,
                        );
                    }
                    _ => {}
                }
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Devices)) => {
                self.device_move(delta as isize * 3)
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Places)) => {
                self.place_move(delta as isize * 3)
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Commands(_))) => {
                self.scroll_commands(delta as isize * 3)
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::Paths(_))) => {
                self.scroll_paths(delta as isize * 3)
            }
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::HistoryStorage)) => {
                self.history_storage.offset = self
                    .history_storage
                    .offset
                    .saturating_add_signed(delta as isize)
                    .min(self.history_storage.maximum);
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
            Input::Scroll { delta, .. } if matches!(self.dialog, Some(Dialog::FolderSize)) => {
                self.scroll_folder_size(delta as isize * 3)
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
                } else if delta != 0
                    && let Some(pane) = self
                        .pane_areas
                        .iter()
                        .position(|r| r.contains(Position::new(x, y)))
                    && !self.file_areas[pane].is_empty()
                {
                    let step = if self.settings.grid { 1 } else { 3 };
                    self.file_tab_mut(pane)
                        .scroll_rows((delta as isize).saturating_mul(step));
                }
            }
            _ => {}
        }
        self.update_preview();
    }
    pub(super) fn insert_text(&mut self, text: &str) {
        if let Some(Dialog::Edit {
            text: value, error, ..
        }) = &mut self.dialog
        {
            self.editor.replace(value, text);
            *error = None;
        }
        self.live_search();
    }
    pub(super) fn live_search(&mut self) {
        self.update_recursive_search();
        self.update_batch_preview(false);
        if let Some(Dialog::Edit {
            kind: EditKind::TrashSearch { .. },
            text,
            ..
        }) = &self.dialog
        {
            if let Some(view) = &mut self.trash {
                view.filter = text.clone();
                view.follow_selection = true;
                view.rebuild();
            }
            return;
        }
        if let Some(Dialog::Edit {
            kind: EditKind::Search { .. },
            text,
            ..
        }) = &self.dialog
        {
            self.panes[self.active].tab_mut().filter = text.clone();
            self.tab_mut().follow_selection = true;
            let hidden = self.settings.hidden;
            self.tab_mut().rebuild(hidden);
        }
    }
    pub(super) fn select_entry(
        &mut self,
        pane: usize,
        index: usize,
        toggle: bool,
        double: bool,
        extend: bool,
    ) {
        self.active = pane;
        if index >= self.tab().visible.len() {
            return;
        }
        if extend {
            self.tab_mut().select_range(index, toggle);
        } else {
            if !toggle && self.tab().explicit_selection {
                self.tab_mut().marked.clear();
                self.tab_mut().explicit_selection = false;
            }
            self.tab_mut().selected = index;
            self.tab_mut().reset_range();
            if toggle {
                self.tab_mut().explicit_selection = true;
                self.toggle_mark();
            } else if double {
                self.open_selected();
            }
        }
    }
    fn move_selection(&mut self, delta: isize, extend: bool, additive: bool) {
        let tab = self.tab_mut();
        tab.follow_selection = true;
        let index = tab
            .selected
            .saturating_add_signed(delta)
            .min(tab.visible.len().saturating_sub(1));
        if extend {
            tab.select_range(index, additive);
            return;
        }
        if tab.explicit_selection {
            tab.marked.clear();
            tab.explicit_selection = false;
        }
        tab.selected = index;
        tab.reset_range();
    }
    pub(super) fn toggle_mark(&mut self) {
        if self.in_trash() {
            self.tab_mut().explicit_selection = true;
        }
        if let Some(path) = self.tab().entry().map(|e| e.path.clone())
            && !self.tab_mut().marked.remove(&path)
        {
            self.tab_mut().marked.insert(path);
        }
        self.tab_mut().reset_range();
    }
    fn key(&mut self, press: KeyPress) {
        if matches!(self.dialog, Some(Dialog::Devices)) {
            self.devices_key(press);
            return;
        }
        if matches!(self.dialog, Some(Dialog::Places)) {
            self.places_key(press);
            return;
        }
        let press = press.normalized();
        if matches!(self.dialog, Some(Dialog::FolderSize)) {
            self.folder_size_key(press);
            return;
        }
        if self.editor_key(press) {
            return;
        }
        if matches!(self.dialog, Some(Dialog::Paths(_))) {
            self.path_menu_key(press);
            return;
        }
        if matches!(self.dialog, Some(Dialog::Conflict(_))) {
            self.conflict_key(press);
            return;
        }
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
        if matches!(self.dialog, Some(Dialog::HistoryStorage)) {
            self.storage_key(press);
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
        if alt || (ctrl && !shift) {
            return;
        }
        match key {
            Key::Up => self.move_selection(-columns, shift, ctrl),
            Key::Down => self.move_selection(columns, shift, ctrl),
            Key::Left if self.settings.grid => self.move_selection(-1, shift, ctrl),
            Key::Right if self.settings.grid => self.move_selection(1, shift, ctrl),
            Key::Home => self.move_selection(isize::MIN, shift, ctrl),
            Key::End => self.move_selection(isize::MAX, shift, ctrl),
            Key::PageUp => self.move_selection(-(self.tab().page_size as isize), shift, ctrl),
            Key::PageDown => self.move_selection(self.tab().page_size as isize, shift, ctrl),
            Key::Escape if !shift && !ctrl => {
                if self.in_trash() {
                    self.tab_mut().explicit_selection = false;
                }
                self.tab_mut().marked.clear();
                self.tab_mut().reset_range();
                self.tab_mut().filter.clear();
                let h = self.settings.hidden;
                if self.in_trash() {
                    self.trash.as_mut().unwrap().rebuild();
                } else {
                    self.tab_mut().rebuild(h);
                }
            }
            _ => {}
        }
    }
}

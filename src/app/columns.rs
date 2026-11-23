use super::*;
use crate::{
    columns::{Column, Layout as ColumnLayout},
    input::{Input, Key},
};

#[derive(Clone)]
pub struct ColumnOwner {
    pub owner: PathOwner,
    pub request: u64,
    pub revision: u64,
    pub area: Rect,
}
pub struct ColumnsMenu {
    pub token: u64,
    pub context: ColumnOwner,
    pub anchor: Position,
    pub selected: usize,
}
pub(super) struct ColumnDrag {
    context: ColumnOwner,
    original: ColumnLayout,
    index: usize,
    start: u16,
}
impl App {
    pub fn resizing_columns(&self) -> bool {
        self.column_drag
            .as_ref()
            .is_some_and(|d| d.original != self.settings.list_columns)
    }

    pub fn column_resize_cursor(&self) -> bool {
        if self.column_drag.is_some() {
            return true;
        }
        let Some(point) = self.pointer.filter(|_| self.dialog.is_none()) else {
            return false;
        };
        let Some((_, Target::ColumnBoundary { context, .. })) =
            self.hits.iter().rev().find(|(r, _)| r.contains(point))
        else {
            return false;
        };
        self.column_owner_valid(context)
    }
    fn column_owner_valid(&self, c: &ColumnOwner) -> bool {
        c.owner.valid(self)
            && self.file_tab(c.owner.pane).request == c.request
            && self.settings.view_mode() == crate::view::ViewMode::List
            && c.revision == self.columns_revision
            && self.column_headers[c.owner.pane] == c.area
    }
    pub(crate) fn column_owner(&self, pane: usize, area: Rect) -> ColumnOwner {
        ColumnOwner {
            owner: PathOwner::current(self, pane),
            request: self.file_tab(pane).request,
            revision: self.columns_revision,
            area,
        }
    }
    pub(crate) fn set_column_header(&mut self, pane: usize, area: Rect) {
        if self
            .column_drag
            .as_ref()
            .is_some_and(|d| d.context.owner.pane == pane && d.context.area != area)
        {
            self.finish_column_drag(true);
        }
        if matches!(&self.dialog,Some(Dialog::Columns(m)) if m.context.owner.pane==pane && m.context.area!=area)
        {
            self.dialog = None;
            self.hits.clear();
        }
        self.column_headers[pane] = area;
    }
    pub(crate) fn validate_column_drag(&mut self) {
        if matches!(&self.dialog,Some(Dialog::Columns(m)) if !self.column_owner_valid(&m.context)) {
            self.dialog = None;
            self.hits.clear();
        }
        if self
            .column_drag
            .as_ref()
            .is_some_and(|d| self.dialog.is_some() || !self.column_owner_valid(&d.context))
        {
            self.finish_column_drag(true);
        }
    }
    pub(super) fn finish_column_drag(&mut self, cancel: bool) {
        let Some(d) = self.column_drag.take() else {
            return;
        };
        let changed = self.settings.list_columns != d.original;
        if cancel {
            if d.context.revision != self.columns_revision {
                return;
            }
            self.settings.list_columns = d.original;
            if changed {
                self.columns_revision = self.columns_revision.wrapping_add(1);
                self.hits.clear();
            }
        } else if changed {
            self.preferences_changed();
        }
    }
    fn move_column_boundary(&mut self, x: u16) {
        self.validate_column_drag();
        let Some(d) = &mut self.column_drag else {
            return;
        };
        let mut layout = d.original.clone();
        layout.resize(
            d.index,
            d.context.area.width,
            i32::from(x) - i32::from(d.start),
        );
        if self.settings.list_columns != layout {
            self.settings.list_columns = layout;
            self.columns_revision = self.columns_revision.wrapping_add(1);
            d.context.revision = self.columns_revision;
        }
    }
    fn column_reset(&mut self, c: ColumnOwner, column: Column) {
        if self.dialog.is_some() || !self.column_owner_valid(&c) {
            return;
        }
        self.settings.list_columns.reset(column);
        self.columns_revision = self.columns_revision.wrapping_add(1);
        self.hits.clear();
        self.preferences_changed();
    }
    fn column_sort(&mut self, c: ColumnOwner, column: Column) {
        if self.dialog.is_some() || !self.column_owner_valid(&c) {
            return;
        }
        self.active = c.owner.pane;
        let (current, descending) = self.tab().sort.field();
        self.apply_sort(fs::Sort::Field(column, current == column && !descending));
    }
    pub(super) fn apply_sort(&mut self, sort: fs::Sort) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        let hidden = self.settings.hidden;
        self.tab_mut().sort = sort;
        self.tab_mut().follow_selection = true;
        if self.in_trash() {
            self.trash.as_mut().unwrap().rebuild();
        } else {
            self.tab_mut().rebuild(hidden);
        }
        self.hits.clear();
        self.drop_regions.clear();
        self.update_preview();
    }
    fn column_menu(&mut self, c: ColumnOwner, anchor: Position) {
        if self.dialog.is_some() || !self.column_owner_valid(&c) {
            return;
        }
        self.active = c.owner.pane;
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.dialog = Some(Dialog::Columns(ColumnsMenu {
            token: self.token(),
            context: c,
            anchor,
            selected: 0,
        }));
        self.hits.clear();
    }
    fn column_toggle(&mut self, token: u64, revision: u64, column: Column) {
        let Some(Dialog::Columns(menu)) = &self.dialog else {
            return;
        };
        if menu.token != token
            || revision != self.columns_revision
            || !self.column_owner_valid(&menu.context)
        {
            return;
        }
        if column == Column::Name {
            return;
        }
        self.settings.list_columns.toggle(column);
        self.columns_revision = self.columns_revision.wrapping_add(1);
        if let Some(Dialog::Columns(menu)) = &mut self.dialog {
            menu.context.revision = self.columns_revision;
            menu.selected = Column::ALL.iter().position(|c| *c == column).unwrap();
        }
        self.hits.clear();
        self.preferences_changed();
    }
    pub(super) fn columns_input(&mut self, input: &Input) -> bool {
        self.validate_column_drag();
        if self.column_drag.is_some() {
            match input {
                Input::Drag { x, .. } => {
                    self.move_column_boundary(*x);
                    return true;
                }
                Input::PointerUp { x, .. } => {
                    self.move_column_boundary(*x);
                    self.finish_column_drag(false);
                    return true;
                }
                Input::CancelDrag | Input::Pointer(None) | Input::SecondaryClick { .. } => {
                    self.finish_column_drag(true);
                    return true;
                }
                Input::Key(k) => {
                    let escape = k.key == Key::Escape;
                    self.finish_column_drag(true);
                    if escape {
                        return true;
                    }
                }
                Input::Scroll { .. } => return true,
                Input::PointerDown { .. } => self.finish_column_drag(true),
                _ => {}
            }
        }
        if let Some(Dialog::Columns(menu)) = &mut self.dialog {
            match input {
                Input::Key(k) => {
                    let token = menu.token;
                    let revision = menu.context.revision;
                    match k.key {
                        Key::Escape => self.cancel_dialog(),
                        Key::Up => menu.selected = menu.selected.saturating_sub(1),
                        Key::Down => menu.selected = (menu.selected + 1).min(Column::ALL.len() - 1),
                        Key::Enter | Key::Char(' ') => {
                            let column = Column::ALL[menu.selected];
                            self.column_toggle(token, revision, column);
                        }
                        _ => {}
                    }
                    return true;
                }
                Input::SecondaryClick { .. } => {
                    self.cancel_dialog();
                    return true;
                }
                _ => {}
            }
        }
        let (x, y) = match input {
            Input::PointerDown { x, y, .. }
            | Input::Click { x, y, .. }
            | Input::SecondaryClick { x, y } => (*x, *y),
            _ => return false,
        };
        let target = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(Position::new(x, y)))
            .map(|(_, t)| t.clone());
        match (input, target) {
            (
                Input::Click { .. },
                Some(Target::ColumnToggle {
                    token,
                    revision,
                    column,
                }),
            ) => self.column_toggle(token, revision, column),
            (
                Input::SecondaryClick { .. },
                Some(Target::ColumnHeader { context, .. } | Target::ColumnBoundary { context, .. }),
            ) => self.column_menu(context, Position::new(x, y)),
            (Input::Click { .. }, Some(Target::ColumnHeader { context, column })) => {
                self.column_sort(context, column)
            }
            (
                Input::PointerDown { double, .. },
                Some(Target::ColumnBoundary { context, column }),
            ) => {
                if *double {
                    self.column_reset(context, column);
                } else if self.dialog.is_none() && self.column_owner_valid(&context) {
                    self.cancel_file_drag();
                    self.finish_marquee(true);
                    self.active = context.owner.pane;
                    let Some(index) = self
                        .settings
                        .list_columns
                        .columns
                        .iter()
                        .position(|s| s.column == column)
                    else {
                        return true;
                    };
                    self.column_drag = Some(ColumnDrag {
                        context,
                        original: self.settings.list_columns.clone(),
                        index,
                        start: x,
                    });
                }
            }
            (
                Input::Click { double: true, .. },
                Some(Target::ColumnBoundary { context, column }),
            ) => self.column_reset(context, column),
            _ => return false,
        }
        self.pointer = Some(Position::new(x, y));
        true
    }
}

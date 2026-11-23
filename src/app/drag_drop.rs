use super::*;
use crate::dnd::{DropAction, Outgoing};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct DropRegion {
    pub area: Rect,
    pane: usize,
    tab: usize,
    cwd: PathBuf,
    request: u64,
    destination: Option<PathBuf>,
    tab_target: bool,
}
pub(super) struct FileDrag {
    pane: usize,
    tab: usize,
    cwd: PathBuf,
    request: u64,
    viewport: Rect,
    origin: Position,
    current: Position,
    sources: Vec<PathBuf>,
    folders: bool,
    entry_press: Option<EntryPress>,
    copy: bool,
    started: bool,
    external: Option<u64>,
    last_scroll: Instant,
}
struct EntryPress {
    initial: BTreeSet<PathBuf>,
    initial_explicit: bool,
    initial_focus: Option<PathBuf>,
    initial_range: crate::model::RangeSelection,
    index: usize,
    toggle: bool,
    double: bool,
    extend: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookmarkDrop {
    Pin,
    FoldersOnly,
    Unavailable,
    ChooseTarget,
}
impl BookmarkDrop {
    pub fn hint(self) -> &'static str {
        match self {
            Self::Pin => "Закрепить в быстром доступе",
            Self::FoldersOnly => "В быстром доступе можно закрепить только папки",
            Self::Unavailable => "Закладки сейчас недоступны",
            Self::ChooseTarget => "Перетащите папку в быстрый доступ",
        }
    }
}
pub struct DragFeedback {
    pub bookmark: Option<BookmarkDrop>,
    pub area: Option<Rect>,
    pub destination: Option<PathBuf>,
    pub count: usize,
    pub copy: bool,
}
struct ExternalHover {
    id: u64,
    point: Position,
    action: DropAction,
    region: DropRegion,
}
#[derive(Default)]
pub(super) struct ExternalDropState {
    hover: Option<ExternalHover>,
    pending: Option<ExternalHover>,
    operation: Option<(u64, u64, DropAction)>,
    cancelled: Vec<u64>,
    last_scroll: Option<Instant>,
}
impl App {
    fn drop_region_valid(&self, r: &DropRegion) -> bool {
        self.dialog.is_none()
            && !self.busy
            && !self.clipboard_reads_pending()
            && (r.pane == 0 || self.settings.dual)
            && self
                .panes
                .get(r.pane)
                .and_then(|p| p.tabs.get(r.tab))
                .is_some_and(|t| {
                    t.cwd == r.cwd
                        && t.archive.as_ref().is_none_or(|a| {
                            r.destination
                                .as_ref()
                                .is_some_and(|p| !p.starts_with(&a.source))
                        })
                        && t.request == r.request
                        && t.error.is_none()
                        && (!t.loading || t.refreshing)
                })
            && (r.tab_target || self.panes[r.pane].active == r.tab)
            && r.destination.is_some()
            && self.drop_regions.iter().any(|current| {
                current.area == r.area
                    && current.pane == r.pane
                    && current.tab == r.tab
                    && current.request == r.request
                    && current.destination == r.destination
            })
    }
    pub(crate) fn refresh_external_hover(&mut self) {
        if let Some(mut hover) = self.external_drop.hover.take()
            && let Some(region) = self
                .drop_regions
                .iter()
                .rev()
                .find(|r| r.area.contains(hover.point))
                .filter(|r| self.drop_region_valid(r))
        {
            hover.region = region.clone();
            self.external_drop.hover = Some(hover);
        }
    }
    pub fn cancel_external_drop(&mut self) {
        for hover in [
            self.external_drop.hover.take(),
            self.external_drop.pending.take(),
        ]
        .into_iter()
        .flatten()
        {
            if !self.external_drop.cancelled.contains(&hover.id) {
                self.external_drop.cancelled.push(hover.id);
            }
        }
    }
    pub fn take_drop_cancellations(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.external_drop.cancelled)
    }
    pub fn hover_external_drop(
        &mut self,
        id: u64,
        point: Option<Position>,
        allowed: u32,
        preferred: DropAction,
    ) -> Option<DropAction> {
        self.external_drop.hover = None;
        let point = point?;
        let action = DropAction::choose(allowed, preferred)?;
        let region = self
            .drop_regions
            .iter()
            .rev()
            .find(|r| r.area.contains(point))?
            .clone();
        if !self.drop_region_valid(&region)
            || self.external_drop.pending.is_some()
            || self.external_drop.operation.is_some()
        {
            return None;
        }
        self.external_drop.hover = Some(ExternalHover {
            id,
            point,
            action,
            region,
        });
        Some(action)
    }
    pub fn leave_external_drop(&mut self, id: u64) {
        if self
            .external_drop
            .hover
            .as_ref()
            .is_some_and(|h| h.id == id)
        {
            self.external_drop.hover = None;
        }
    }
    pub fn capture_external_drop(&mut self, id: u64) -> bool {
        let Some(hover) = self
            .external_drop
            .hover
            .take()
            .filter(|h| h.id == id && self.drop_region_valid(&h.region))
        else {
            return false;
        };
        self.external_drop.pending = Some(hover);
        true
    }
    pub fn receive_external_drop(&mut self, id: u64, result: Result<Vec<PathBuf>, String>) -> bool {
        let Some(ticket) = self.external_drop.pending.take() else {
            return false;
        };
        if ticket.id != id {
            self.external_drop.pending = Some(ticket);
            return false;
        }
        if !self.drop_region_valid(&ticket.region) {
            self.message("Папка назначения изменилась; перетащите файлы снова", true);
            return false;
        }
        let paths = match result {
            Ok(paths)
                if !paths.is_empty()
                    && paths.len() <= crate::clipboard::files::MAX_PATHS
                    && paths.iter().all(|p| p.is_absolute()) =>
            {
                paths
            }
            Ok(_) => {
                self.message("Некорректный список перетаскивания", true);
                return false;
            }
            Err(error) => {
                self.message(error, true);
                return false;
            }
        };
        let destination = ticket.region.destination.unwrap();
        if paths.iter().any(|p| {
            destination.starts_with(p)
                || (ticket.action == DropAction::Move && p.parent() == Some(destination.as_path()))
        }) {
            self.message("Нельзя перенести файлы в них самих или в ту же папку", true);
            return false;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.start_operation(Operation::Transfer {
            sources: paths,
            destination,
            cut: ticket.action == DropAction::Move,
        });
        if let Some(operation) = self.operations.front().filter(|_| self.busy) {
            self.external_drop.operation = Some((id, operation.id, ticket.action));
            true
        } else {
            false
        }
    }
    pub fn external_drop_result(&mut self) -> Option<(u64, Option<DropAction>)> {
        let (id, operation, action) = self.external_drop.operation?;
        let record = self.operations.iter().find(|r| r.id == operation)?;
        record.elapsed?;
        let success = !record.cancelled
            && record.error.is_none()
            && record.completed_paths.len() == record.progress.items_total;
        self.external_drop.operation = None;
        Some((id, success.then_some(action)))
    }
    pub fn external_drag_active(&self) -> bool {
        self.external_drop.hover.is_some()
            || self.external_drop.pending.is_some()
            || self
                .file_drag
                .as_ref()
                .is_some_and(|d| d.external.is_some())
    }
    pub fn external_drop_reading(&self) -> bool {
        self.external_drop.pending.is_some()
    }
    pub fn outgoing_drag_id(&self) -> Option<u64> {
        self.file_drag.as_ref()?.external
    }
    pub fn export_file_drag(&mut self) -> Option<Outgoing> {
        self.validate_file_drag();
        if let Some(origin) = self
            .file_drag
            .as_ref()
            .filter(|d| !d.started && d.entry_press.is_some())
            .map(|d| d.origin)
        {
            self.move_file_drag(origin.x.saturating_add(1), origin.y);
        }
        let id = self.token().wrapping_mul(2);
        let drag = self
            .file_drag
            .as_mut()
            .filter(|d| d.started && d.external.is_none() && d.entry_press.is_some())?;
        drag.external = Some(id);
        Some(Outgoing {
            id,
            paths: drag.sources.clone(),
            copy: drag.copy,
        })
    }
    pub fn export_file_drag_at(&mut self, point: Position) -> Option<Outgoing> {
        if self.file_drag.is_none() {
            let (pane, index) = self.hits.iter().rev().find_map(|(area, target)| {
                if area.contains(point)
                    && let Target::Entry { pane, index } = target
                {
                    Some((*pane, *index))
                } else {
                    None
                }
            })?;
            if self.dialog.is_some()
                || !self.begin_file_drag(point, pane, index, false, false, false)
            {
                return None;
            }
        }
        self.export_file_drag()
    }
    pub fn finish_export(&mut self, id: u64, result: Result<(), String>) {
        if self.outgoing_drag_id() != Some(id) {
            return;
        }
        let drag = self.file_drag.take().unwrap();
        self.restore_file_drag(&drag);
        self.pending_directory_changes.insert(drag.cwd);
        match result {
            Ok(()) => self.message("Файлы переданы другому приложению", false),
            Err(error) => self.message(error, true),
        }
    }
    /// Registered from the actual rendered pane/tab/entry, never inferred from pixels in a frontend.
    pub(crate) fn drop_region(
        &mut self,
        area: Rect,
        pane: usize,
        tab: usize,
        destination: Option<PathBuf>,
        tab_target: bool,
    ) {
        let t = &self.panes[pane].tabs[tab];
        let usable = !self
            .trash
            .as_ref()
            .is_some_and(|v| v.owner.pane == pane && v.owner.tab == tab)
            && t.error.is_none()
            && t.archive.as_ref().is_none_or(|a| {
                destination
                    .as_ref()
                    .is_some_and(|p| !p.starts_with(&a.source))
            })
            && (!t.loading || t.refreshing);
        self.drop_regions.push(DropRegion {
            area,
            pane,
            tab,
            cwd: t.cwd.clone(),
            request: t.request,
            destination: destination.filter(|_| usable),
            tab_target,
        });
    }
    pub(super) fn begin_file_drag(
        &mut self,
        point: Position,
        pane: usize,
        index: usize,
        toggle: bool,
        double: bool,
        extend: bool,
    ) -> bool {
        if self.busy
            || self.clipboard_reads_pending()
            || self.trash_in(pane)
            || self.file_tab(pane).archive.is_some()
        {
            return false;
        }
        let tab = self.file_tab(pane);
        if tab.error.is_some() || (tab.loading && !tab.refreshing) {
            return false;
        }
        let Some(entry) = tab.visible.get(index).and_then(|&i| tab.entries.get(i)) else {
            return false;
        };
        let press = EntryPress {
            index,
            toggle,
            double,
            extend,
            initial: tab.marked.clone(),
            initial_explicit: tab.explicit_selection,
            initial_focus: tab.entry().map(|e| e.path.clone()),
            initial_range: tab.range.clone(),
        };
        let (sources, folders) = if !extend && !tab.marked.contains(&entry.path) {
            (vec![entry.path.clone()], entry.directory)
        } else {
            if extend {
                self.file_tab_mut(pane).select_range(index, toggle);
            }
            let tab = self.file_tab(pane);
            let sources = tab.targets();
            let mut remaining: BTreeSet<_> = sources.iter().collect();
            for entry in tab.entries.iter().filter(|e| e.directory) {
                remaining.remove(&entry.path);
            }
            if let Some(run) = &tab.filter_run {
                for path in &run.marked_directories {
                    remaining.remove(path);
                }
            }
            let folders = remaining.is_empty();
            (sources, folders)
        };
        if sources.is_empty() {
            return false;
        }
        self.start_drag(point, pane, sources, folders, Some(press));
        self.active = pane;
        if extend {
            self.update_preview();
        }
        true
    }
    pub(super) fn begin_breadcrumb_drag(
        &mut self,
        point: Position,
        owner: &PathOwner,
        path: PathBuf,
    ) -> bool {
        if !owner.valid(self)
            || self.trash_in(owner.pane)
            || self.busy
            || self.clipboard_reads_pending()
        {
            return false;
        }
        let tab = self.file_tab(owner.pane);
        if tab.archive.is_some() || tab.error.is_some() || (tab.loading && !tab.refreshing) {
            return false;
        }
        self.start_drag(point, owner.pane, vec![path], true, None);
        true
    }
    fn start_drag(
        &mut self,
        point: Position,
        pane: usize,
        sources: Vec<PathBuf>,
        folders: bool,
        entry_press: Option<EntryPress>,
    ) {
        let tab = self.file_tab(pane);
        self.file_drag = Some(FileDrag {
            pane,
            tab: self.panes[pane].active,
            cwd: tab.cwd.clone(),
            request: tab.request,
            viewport: self.file_areas[pane],
            origin: point,
            current: point,
            sources,
            folders,
            copy: entry_press.as_ref().is_some_and(|p| p.toggle),
            entry_press,
            started: false,
            external: None,
            last_scroll: Instant::now(),
        });
    }
    pub fn dragging_files(&self) -> bool {
        self.file_drag.as_ref().is_some_and(|d| d.started)
    }
    pub fn gesture_area(&self) -> Option<Rect> {
        self.selection_area().or_else(|| self.drag_feedback()?.area)
    }
    pub fn drag_feedback(&self) -> Option<DragFeedback> {
        if let Some(hover) = self
            .external_drop
            .hover
            .as_ref()
            .filter(|h| self.drop_region_valid(&h.region))
        {
            return Some(DragFeedback {
                bookmark: None,
                area: Some(hover.region.area),
                destination: hover.region.destination.clone(),
                count: 0,
                copy: hover.action == DropAction::Copy,
            });
        }
        let drag = self
            .file_drag
            .as_ref()
            .filter(|d| d.started && d.external.is_none())?;
        let over_bookmarks = self.bookmark_drop_area.contains(drag.current);
        if over_bookmarks || drag.entry_press.is_none() {
            let effect = if !over_bookmarks {
                BookmarkDrop::ChooseTarget
            } else if !drag.folders {
                BookmarkDrop::FoldersOnly
            } else if !self.library.writable() {
                BookmarkDrop::Unavailable
            } else {
                BookmarkDrop::Pin
            };
            return Some(DragFeedback {
                bookmark: Some(effect),
                area: (effect == BookmarkDrop::Pin).then_some(self.bookmark_drop_area),
                destination: None,
                count: drag.sources.len(),
                copy: false,
            });
        }
        let region = self
            .drop_regions
            .iter()
            .rev()
            .find(|r| r.area.contains(drag.current));
        let valid = region.filter(|r| {
            self.panes
                .get(r.pane)
                .and_then(|p| p.tabs.get(r.tab))
                .is_some_and(|t| {
                    t.cwd == r.cwd
                        && t.request == r.request
                        && t.error.is_none()
                        && (!t.loading || t.refreshing)
                })
                && (r.tab_target || self.panes[r.pane].active == r.tab)
                && r.destination.as_ref().is_some_and(|path| {
                    !drag.sources.iter().any(|source| path.starts_with(source))
                        && (drag.copy
                            || drag
                                .sources
                                .iter()
                                .any(|source| source.parent() != Some(path.as_path())))
                })
        });
        Some(DragFeedback {
            bookmark: None,
            area: valid.map(|r| r.area),
            destination: valid.and_then(|r| r.destination.clone()),
            count: drag.sources.len(),
            copy: drag.copy,
        })
    }
    pub(super) fn move_file_drag(&mut self, x: u16, y: u16) {
        self.validate_file_drag();
        let Some(drag) = &mut self.file_drag else {
            return;
        };
        if drag.external.is_some() {
            return;
        }
        drag.current = Position::new(x, y);
        self.pointer = Some(drag.current);
        if !drag.started && drag.current != drag.origin {
            drag.started = true;
            if drag.entry_press.is_none() {
                return;
            }
            let pane = drag.pane;
            let sources = drag.sources.clone();
            let tab = self.file_tab_mut(pane);
            if let Some(index) = tab
                .visible
                .iter()
                .position(|&i| tab.entries[i].path == sources[0])
            {
                tab.selected = index;
            }
            tab.marked = sources.into_iter().collect();
            tab.explicit_selection = true;
            tab.reset_range();
        }
    }
    pub(super) fn set_drag_copy(&mut self, copy: bool) {
        if let Some(drag) = &mut self.file_drag {
            drag.copy = copy;
        }
    }
    pub(super) fn release_file_drag(&mut self, x: u16, y: u16) {
        if self.outgoing_drag_id().is_some() {
            return;
        }
        self.move_file_drag(x, y);
        let feedback = self.drag_feedback();
        let Some(drag) = self.file_drag.take() else {
            return;
        };
        if !drag.started {
            if let Some(press) = drag.entry_press {
                self.select_entry(
                    drag.pane,
                    press.index,
                    press.toggle,
                    press.double,
                    press.extend,
                );
                self.update_preview();
            } else {
                let owner = PathOwner::current(self, drag.pane);
                let path = drag.sources.into_iter().next().unwrap();
                if path == owner.cwd {
                    self.edit_path(owner);
                } else {
                    self.go_path(owner, path);
                }
            }
        } else if let Some(effect) = feedback.as_ref().and_then(|f| f.bookmark) {
            self.restore_file_drag(&drag);
            if effect == BookmarkDrop::Pin {
                self.pin_folders(drag.sources);
            } else {
                self.message(effect.hint(), false);
            }
        } else if let Some(destination) = feedback.and_then(|f| f.destination) {
            self.start_operation(Operation::Transfer {
                sources: drag.sources,
                destination,
                cut: !drag.copy,
            });
        } else {
            self.restore_file_drag(&drag);
            self.message("Перетаскивание отменено: выберите другую папку", false);
        }
    }
    fn restore_file_drag(&mut self, drag: &FileDrag) {
        let Some(press) = &drag.entry_press else {
            return;
        };
        if self.panes[drag.pane].active != drag.tab || self.file_tab(drag.pane).cwd != drag.cwd {
            return;
        }
        let tab = self.file_tab_mut(drag.pane);
        if tab.filter_run.is_some() && tab.request == drag.request {
            tab.marked.clone_from(&press.initial);
        } else {
            tab.marked = tab
                .visible
                .iter()
                .map(|&i| &tab.entries[i].path)
                .filter(|p| press.initial.contains(*p))
                .cloned()
                .collect();
        }
        tab.explicit_selection = press.initial_explicit;
        tab.range = press.initial_range.clone();
        tab.retain_range();
        if let Some(index) = press
            .initial_focus
            .as_ref()
            .and_then(|p| tab.visible.iter().position(|&i| tab.entries[i].path == *p))
        {
            tab.selected = index;
        }
    }
    pub(super) fn cancel_file_drag(&mut self) {
        if let Some(drag) = self.file_drag.take() {
            self.restore_file_drag(&drag);
        }
    }
    pub(crate) fn validate_file_drag(&mut self) {
        if self.file_drag.as_ref().is_some_and(|d| {
            self.dialog.is_some()
                || self.busy
                || self.clipboard_reads_pending()
                || self.panes[d.pane].active != d.tab
                || self.file_tab(d.pane).cwd != d.cwd
                || self.file_tab(d.pane).request != d.request
                || self.file_areas[d.pane] != d.viewport
        }) {
            self.cancel_file_drag();
        }
    }
    pub(super) fn tick_file_drag(&mut self) {
        self.validate_file_drag();
        if self
            .external_drop
            .last_scroll
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(100))
            && let Some(hover) = &self.external_drop.hover
        {
            let point = hover.point;
            if let Some(pane) = self.file_areas.iter().position(|r| r.contains(point)) {
                let area = self.file_areas[pane];
                let horizontal = self.file_tab(pane).column_rows > 0;
                let delta = if if horizontal {
                    point.x == area.x
                } else {
                    point.y == area.y
                } {
                    -1
                } else if if horizontal {
                    point.x + 1 == area.right()
                } else {
                    point.y + 1 == area.bottom()
                } {
                    1
                } else {
                    0
                };
                if delta != 0 {
                    self.file_tab_mut(pane).scroll_rows(delta);
                    self.external_drop.last_scroll = Some(Instant::now());
                }
            }
        }
        let Some(drag) = &mut self.file_drag else {
            return;
        };
        if drag.entry_press.is_none()
            || !drag.started
            || drag.last_scroll.elapsed() < Duration::from_millis(100)
        {
            return;
        }
        let Some(pane) = self
            .file_areas
            .iter()
            .position(|r| r.contains(drag.current))
        else {
            return;
        };
        let area = self.file_areas[pane];
        let horizontal = self.settings.view_mode() == crate::view::ViewMode::Columns;
        let delta = if if horizontal {
            drag.current.x == area.x
        } else {
            drag.current.y == area.y
        } {
            -1
        } else if if horizontal {
            drag.current.x + 1 == area.right()
        } else {
            drag.current.y + 1 == area.bottom()
        } {
            1
        } else {
            return;
        };
        drag.last_scroll = Instant::now();
        self.file_tab_mut(pane).scroll_rows(delta);
    }
}

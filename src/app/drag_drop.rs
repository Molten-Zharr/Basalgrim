use super::*;
use crate::input::Input;
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
    initial: BTreeSet<PathBuf>,
    initial_explicit: bool,
    initial_focus: Option<PathBuf>,
    toggle: bool,
    double: bool,
    copy: bool,
    started: bool,
    last_scroll: Instant,
}
pub struct DragFeedback {
    pub area: Option<Rect>,
    pub destination: Option<PathBuf>,
    pub count: usize,
    pub copy: bool,
}
impl App {
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
    ) -> bool {
        if self.busy || self.clipboard_reads_pending() || self.trash_in(pane) {
            return false;
        }
        let tab = self.file_tab(pane);
        if tab.error.is_some() || (tab.loading && !tab.refreshing) {
            return false;
        }
        let Some(entry) = tab.visible.get(index).and_then(|&i| tab.entries.get(i)) else {
            return false;
        };
        let sources = if tab.marked.contains(&entry.path) {
            tab.targets()
        } else {
            vec![entry.path.clone()]
        };
        self.file_drag = Some(FileDrag {
            pane,
            tab: self.panes[pane].active,
            cwd: tab.cwd.clone(),
            request: tab.request,
            viewport: self.file_areas[pane],
            origin: point,
            current: point,
            sources,
            initial: tab.marked.clone(),
            initial_explicit: tab.explicit_selection,
            initial_focus: tab.entry().map(|e| e.path.clone()),
            toggle,
            double,
            copy: toggle,
            started: false,
            last_scroll: Instant::now(),
        });
        self.active = pane;
        true
    }
    pub fn dragging_files(&self) -> bool {
        self.file_drag.as_ref().is_some_and(|d| d.started)
    }
    pub fn gesture_area(&self) -> Option<Rect> {
        self.selection_area().or_else(|| self.drag_feedback()?.area)
    }
    pub fn drag_feedback(&self) -> Option<DragFeedback> {
        let drag = self.file_drag.as_ref().filter(|d| d.started)?;
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
                        && (drag.copy || path != &drag.cwd)
                })
        });
        Some(DragFeedback {
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
        drag.current = Position::new(x, y);
        self.pointer = Some(drag.current);
        if !drag.started && drag.current != drag.origin {
            drag.started = true;
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
        }
    }
    pub(super) fn set_drag_copy(&mut self, copy: bool) {
        if let Some(drag) = &mut self.file_drag {
            drag.copy = copy;
        }
    }
    pub(super) fn release_file_drag(&mut self, x: u16, y: u16) {
        self.move_file_drag(x, y);
        let feedback = self.drag_feedback();
        let Some(drag) = self.file_drag.take() else {
            return;
        };
        if !drag.started {
            self.handle(Input::Click {
                x: drag.origin.x,
                y: drag.origin.y,
                double: drag.double,
                toggle: drag.toggle,
            });
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
        if self.panes[drag.pane].active != drag.tab || self.file_tab(drag.pane).cwd != drag.cwd {
            return;
        }
        let tab = self.file_tab_mut(drag.pane);
        let visible: BTreeSet<_> = tab
            .visible
            .iter()
            .map(|&i| tab.entries[i].path.clone())
            .collect();
        tab.marked = drag.initial.intersection(&visible).cloned().collect();
        tab.explicit_selection = drag.initial_explicit;
        if let Some(index) = drag
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
        let Some(drag) = &mut self.file_drag else {
            return;
        };
        if !drag.started || drag.last_scroll.elapsed() < Duration::from_millis(100) {
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
        let delta = if drag.current.y == area.y {
            -1
        } else if drag.current.y + 1 == area.bottom() {
            1
        } else {
            return;
        };
        drag.last_scroll = Instant::now();
        self.file_tab_mut(pane).scroll_rows(delta);
    }
}

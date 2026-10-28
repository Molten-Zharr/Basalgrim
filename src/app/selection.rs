use super::*;
use crate::input::Input;
use std::collections::BTreeSet;

pub(super) struct Marquee {
    pane: usize,
    tab: usize,
    cwd: PathBuf,
    request: u64,
    grid: bool,
    viewport: Rect,
    origin: Position,
    current: Position,
    initial: BTreeSet<PathBuf>,
    initial_explicit: bool,
    initial_focused: Option<PathBuf>,
    items: Vec<(Rect, PathBuf)>,
    additive: bool,
    dragging: bool,
}

impl App {
    pub fn selection_area(&self) -> Option<Rect> {
        let drag = self.marquee.as_ref().filter(|d| d.dragging)?;
        Some(
            Rect::new(
                drag.origin.x.min(drag.current.x),
                drag.origin.y.min(drag.current.y),
                drag.origin.x.abs_diff(drag.current.x).saturating_add(1),
                drag.origin.y.abs_diff(drag.current.y).saturating_add(1),
            )
            .intersection(drag.viewport),
        )
    }

    pub(crate) fn set_file_area(&mut self, pane: usize, area: Rect) {
        if self
            .marquee
            .as_ref()
            .is_some_and(|d| d.pane == pane && d.viewport != area)
        {
            self.finish_marquee(true);
        }
        self.file_areas[pane] = area;
    }

    pub(super) fn primary_down(&mut self, x: u16, y: u16, double: bool, toggle: bool) {
        self.finish_marquee(false);
        let point = Position::new(x, y);
        let target = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(point))
            .map(|(_, t)| t);
        let pane = self.file_areas.iter().position(|r| r.contains(point));
        let candidate = pane
            .filter(|_| {
                self.dialog.is_none()
                    && !double
                    && matches!(target, None | Some(Target::Entry { .. }))
            })
            .map(|pane| {
                let tab = self.panes[pane].tab();
                Marquee {
                    pane,
                    tab: self.panes[pane].active,
                    cwd: tab.cwd.clone(),
                    request: tab.request,
                    grid: self.settings.grid,
                    viewport: self.file_areas[pane],
                    origin: point,
                    current: point,
                    initial: tab.marked.clone(),
                    initial_explicit: tab.explicit_selection,
                    initial_focused: tab.entry().map(|entry| entry.path.clone()),
                    items: self
                        .hits
                        .iter()
                        .filter_map(|(area, target)| {
                            if let Target::Entry { pane: p, index } = target
                                && *p == pane
                            {
                                let entry = &tab.entries[*tab.visible.get(*index)?];
                                Some((*area, entry.path.clone()))
                            } else {
                                None
                            }
                        })
                        .collect(),
                    additive: toggle,
                    dragging: false,
                }
            });
        let background = target.is_none();
        self.handle(Input::Click {
            x,
            y,
            double,
            toggle,
        });
        if let Some(candidate) = candidate {
            self.active = candidate.pane;
            if background && !toggle {
                self.tab_mut().marked.clear();
                self.tab_mut().explicit_selection = true;
            }
            self.marquee = Some(candidate);
        }
    }

    pub(super) fn drag_selection(&mut self, x: u16, y: u16) {
        self.validate_marquee();
        let Some(drag) = &mut self.marquee else {
            return;
        };
        self.pointer = Some(Position::new(x, y));
        drag.current = Position::new(
            x.clamp(drag.viewport.x, drag.viewport.right() - 1),
            y.clamp(drag.viewport.y, drag.viewport.bottom() - 1),
        );
        drag.dragging |= drag.current != drag.origin;
        if !drag.dragging {
            return;
        }
        let pane = drag.pane;
        let area = self.selection_area().unwrap();
        let drag = self.marquee.as_ref().unwrap();
        let mut marked = if drag.additive {
            drag.initial.clone()
        } else {
            BTreeSet::new()
        };
        if drag.additive && marked.is_empty() && !drag.initial_explicit {
            marked.extend(drag.initial_focused.iter().cloned());
        }
        marked.extend(
            drag.items
                .iter()
                .filter(|(r, _)| !r.intersection(area).is_empty())
                .map(|(_, p)| p.clone()),
        );
        self.panes[pane].tab_mut().marked = marked;
        self.panes[pane].tab_mut().explicit_selection = true;
    }

    pub(super) fn finish_marquee(&mut self, restore: bool) {
        let Some(drag) = self.marquee.take() else {
            return;
        };
        if restore
            && drag.dragging
            && let Some(tab) = self.panes[drag.pane].tabs.get_mut(drag.tab)
            && tab.cwd == drag.cwd
        {
            let visible: BTreeSet<_> = tab
                .visible
                .iter()
                .map(|&i| tab.entries[i].path.clone())
                .collect();
            tab.marked = drag.initial.intersection(&visible).cloned().collect();
            tab.explicit_selection = drag.initial_explicit;
            if let Some(index) = drag.initial_focused.as_ref().and_then(|path| {
                tab.visible
                    .iter()
                    .position(|&i| tab.entries[i].path == *path)
            }) {
                tab.selected = index;
            } else if tab.marked.is_empty() {
                tab.explicit_selection = true;
            }
            if self.active == drag.pane && self.panes[drag.pane].active == drag.tab {
                self.update_preview();
            }
        }
    }

    pub(crate) fn validate_marquee(&mut self) {
        if self.marquee.as_ref().is_some_and(|d| {
            self.dialog.is_some()
                || self.active != d.pane
                || self.panes[d.pane].active != d.tab
                || self.panes[d.pane].tab().cwd != d.cwd
                || self.panes[d.pane].tab().request != d.request
                || self.settings.grid != d.grid
                || self.file_areas[d.pane] != d.viewport
        }) {
            self.finish_marquee(true);
        }
    }
}

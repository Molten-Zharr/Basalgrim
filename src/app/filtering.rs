use super::*;
use crate::{
    filter::{Kind, Options, Run},
    input::{Input, Key, KeyPress},
};
#[derive(Clone)]
pub struct FilterOwner {
    pub owner: PathOwner,
    pub request: u64,
}
impl FilterOwner {
    pub fn current(app: &App, pane: usize) -> Self {
        Self {
            owner: PathOwner::current(app, pane),
            request: app.file_tab(pane).request,
        }
    }
    pub fn valid(&self, app: &App) -> bool {
        self.owner.valid(app) && app.file_tab(self.owner.pane).request == self.request
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterControl {
    Name,
    Extension,
    DepthMenu,
    OptionsMenu,
    Depth(Option<u8>),
    Case,
    Kind(Kind),
    Keep,
    Clear,
    Stop,
    Retry,
    SeparateSearch,
    Page(isize),
}
pub struct FilterMenu {
    pub token: u64,
    pub owner: FilterOwner,
    pub anchor: Position,
    pub depth: bool,
    pub selected: usize,
}
impl App {
    pub fn inline_filter(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::Search { .. }
                    | EditKind::TrashSearch { .. }
                    | EditKind::FilterExtension { .. },
                ..
            })
        )
    }
    pub fn filter_status(&self, pane: usize) -> String {
        let tab = self.file_tab(pane);
        if let Some(run) = &tab.filter_run {
            let r = &run.worker.results;
            if let Some(error) = &r.failure {
                return format!("Ошибка: {error}");
            }
            format!(
                "{} · {}–{} / {} · пропущено {}",
                r.status(),
                if tab.entries.is_empty() {
                    0
                } else {
                    run.page + 1
                },
                (run.page + tab.entries.len()).min(r.total),
                r.total,
                r.errors
            )
        } else {
            format!(
                "{} элементов · {}",
                tab.visible.len(),
                tab.filter_options.hint()
            )
        }
    }
    pub fn filter_deep_allowed(&self, pane: usize) -> bool {
        !self.trash_in(pane) && self.file_tab(pane).archive.is_none()
    }
    pub(super) fn start_pane_filter(&mut self, pane: usize, index: usize, reset: bool) {
        let token = self.token();
        let hidden = self.settings.hidden;
        let tab = &mut self.panes[pane].tabs[index];
        tab.scan_cancel.cancel();
        tab.request = token;
        tab.error = None;
        tab.loading = true;
        tab.refreshing = !reset && !tab.entries.is_empty();
        tab.tree = None;
        let mut run = tab.filter_run.take().unwrap_or_else(|| {
            Run::new(
                tab.filter.clone(),
                tab.filter_options.clone(),
                hidden,
                tab.sort,
                tab.grouping,
            )
        });
        run.query = tab.filter.clone();
        run.options = tab.filter_options.clone();
        run.hidden = hidden;
        run.sort = tab.sort;
        run.grouping = tab.grouping;
        run.worker.start_filter(
            tab.cwd.clone(),
            tab.filter.clone(),
            hidden,
            tab.filter_options.clone(),
            tab.sort,
            tab.grouping,
        );
        run.accepted.clear();
        run.accepted_page = usize::MAX;
        if reset {
            run.page = 0;
            run.marked_directories.clear();
            tab.entries.clear();
            tab.visible.clear();
            tab.marked.clear();
            tab.explicit_selection = false;
            tab.offset = 0;
            tab.selected = 0;
            tab.group_view = None;
            tab.groups.clear();
        }
        run.worker.request_page(run.page);
        tab.filter_run = Some(run);
        self.hits.clear();
        self.drop_regions.clear();
    }
    pub(super) fn filter_changed(&mut self, reset: bool) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.typeahead = None;
        if self.in_trash() {
            self.tab_mut().filter_options.depth = Some(0);
            self.trash.as_mut().unwrap().rebuild();
        } else if self.tab().filter_options.deep() && self.in_archive() {
            self.tab_mut().filter_options.depth = Some(0);
            let hidden = self.settings.hidden;
            self.tab_mut().rebuild(hidden);
        } else if self.tab().filter_options.deep() {
            self.start_pane_filter(self.active, self.panes[self.active].active, reset);
        } else if self.tab_mut().filter_run.take().is_some() {
            self.reload(self.active, self.panes[self.active].active);
        } else {
            let hidden = self.settings.hidden;
            self.tab_mut().rebuild(hidden);
        }
        self.tab_mut().follow_selection = true;
        self.hits.clear();
        self.drop_regions.clear();
        self.update_preview();
    }
    pub(super) fn poll_filters(&mut self) {
        // A filter editor is inline, so its result canvas remains live. Actual
        // file gestures, rename and operation dialogs keep their captured rows.
        if self.busy
            || self.column_drag.is_some()
            || self.marquee.is_some()
            || self.file_drag.is_some()
            || self.external_drag_active()
            || (self.dialog.is_some() && !self.inline_filter())
        {
            return;
        }
        let mut restart = vec![];
        let mut changed = false;
        for pane in 0..self.panes.len() {
            for index in 0..self.panes[pane].tabs.len() {
                let tab = &mut self.panes[pane].tabs[index];
                let Some(run) = &mut tab.filter_run else {
                    if tab.filter_options.deep()
                        && !tab.loading
                        && tab.error.is_none()
                        && tab.archive.is_none()
                    {
                        restart.push((pane, index));
                    }
                    continue;
                };
                if run.query != tab.filter
                    || run.options != tab.filter_options
                    || run.hidden != self.settings.hidden
                    || run.sort != tab.sort
                    || run.grouping != tab.grouping
                {
                    restart.push((pane, index));
                    continue;
                }
                run.worker.poll();
                let result = &run.worker.results;
                if result.offset != run.page {
                    continue;
                }
                if result.done && result.total > 0 && run.page >= result.total {
                    run.page =
                        (result.total - 1) / crate::search::PAGE_SIZE * crate::search::PAGE_SIZE;
                    run.worker.request_page(run.page);
                    continue;
                }
                tab.loading = !result.done && result.entries.is_empty();
                tab.refreshing = false;
                if run.accepted_page == result.offset && run.accepted == result.entries {
                    continue;
                }
                run.marked_directories.extend(
                    tab.entries
                        .iter()
                        .filter(|e| e.directory && tab.marked.contains(&e.path))
                        .map(|e| e.path.clone()),
                );
                run.marked_directories.retain(|p| tab.marked.contains(p));
                let entries = result.entries.clone();
                run.accepted = entries.clone();
                run.accepted_page = result.offset;
                let focus = tab.entry().map(|e| e.path.clone());
                let top = tab
                    .visible
                    .get(tab.offset)
                    .map(|&i| tab.entries[i].path.clone());
                tab.entries = entries;
                tab.visible.clear();
                tab.rebuild(self.settings.hidden);
                if let Some(i) =
                    focus.and_then(|p| tab.visible.iter().position(|&i| tab.entries[i].path == p))
                {
                    tab.selected = i;
                }
                if !tab.follow_selection
                    && let Some(i) =
                        top.and_then(|p| tab.visible.iter().position(|&i| tab.entries[i].path == p))
                {
                    tab.offset = i;
                }
                changed = true;
                // Reject coordinates captured before this page/sorted snapshot.
                self.serial += 1;
                tab.request = self.serial;
            }
        }
        for (p, i) in restart {
            self.start_pane_filter(p, i, false);
        }
        if changed {
            self.hits.clear();
            self.drop_regions.clear();
            self.update_preview();
        }
    }
    pub fn filter_control(
        &mut self,
        owner: FilterOwner,
        menu: Option<u64>,
        control: FilterControl,
    ) {
        if !owner.valid(self) {
            return;
        }
        if let Some(token) = menu {
            if !self
                .filter_menu_items()
                .iter()
                .any(|(_, c, enabled)| *c == control && *enabled)
            {
                return;
            }
            if !matches!(&self.dialog,Some(Dialog::FilterMenu(m)) if m.token==token&&m.owner.valid(self))
            {
                return;
            }
        } else if self.dialog.is_some() {
            return;
        }
        self.active = owner.owner.pane;
        match control {
            FilterControl::Name | FilterControl::Extension => {
                self.dialog = None;
                let ext = control == FilterControl::Extension;
                let text = if ext {
                    self.tab().filter_options.extension.clone()
                } else {
                    self.tab().filter.clone()
                };
                let kind = if ext {
                    EditKind::FilterExtension {
                        original: text.clone(),
                    }
                } else if self.in_trash() {
                    EditKind::TrashSearch {
                        original: text.clone(),
                    }
                } else {
                    EditKind::Search {
                        original: text.clone(),
                    }
                };
                let len = text.len();
                self.begin_edit(kind, text, 0..len);
                return;
            }
            FilterControl::DepthMenu | FilterControl::OptionsMenu => {
                self.dialog = Some(Dialog::FilterMenu(FilterMenu {
                    token: self.token(),
                    owner,
                    anchor: self.pointer.unwrap_or(Position::new(2, 4)),
                    depth: control == FilterControl::DepthMenu,
                    selected: if control == FilterControl::DepthMenu {
                        self.tab().filter_options.depth.map_or(7, usize::from)
                    } else {
                        0
                    },
                }));
                return;
            }
            FilterControl::Keep => {
                self.settings.keep_filters_on_navigation =
                    !self.settings.keep_filters_on_navigation;
                self.preferences_changed();
            }
            FilterControl::Depth(depth) => {
                if depth.is_some_and(|d| d > 6)
                    || (depth != Some(0) && !self.filter_deep_allowed(self.active))
                {
                    return;
                }
                self.tab_mut().filter_options.depth = depth;
                self.dialog = None;
                self.filter_changed(true);
                return;
            }
            FilterControl::Case => {
                let next = !self.tab().filter_options.case_sensitive;
                self.tab_mut().filter_options.case_sensitive = next;
                self.filter_changed(true);
            }
            FilterControl::Kind(kind) => {
                self.tab_mut().filter_options.kind = kind;
                self.filter_changed(true);
            }
            FilterControl::Clear => {
                self.tab_mut().filter.clear();
                self.tab_mut().filter_options = Options::default();
                self.dialog = None;
                self.filter_changed(true);
                return;
            }
            FilterControl::Retry => {
                self.dialog = None;
                self.filter_changed(false);
                return;
            }
            FilterControl::SeparateSearch => {
                self.dialog = None;
                let text = self.tab().filter.clone();
                self.begin_edit(
                    EditKind::Search {
                        original: text.clone(),
                    },
                    text.clone(),
                    0..text.len(),
                );
                self.begin_recursive_search();
                return;
            }
            FilterControl::Stop => {
                if let Some(run) = &mut self.tab_mut().filter_run {
                    run.worker.stop();
                }
                self.dialog = None;
                return;
            }
            FilterControl::Page(delta) => {
                self.dialog = None;
                self.cancel_file_drag();
                self.finish_marquee(true);
                let tab = self.tab_mut();
                let Some(run) = &mut tab.filter_run else {
                    return;
                };
                let last = run.worker.results.total.saturating_sub(1) / crate::search::PAGE_SIZE;
                let page = (run.page / crate::search::PAGE_SIZE)
                    .saturating_add_signed(delta)
                    .min(last)
                    * crate::search::PAGE_SIZE;
                if page == run.page {
                    return;
                }
                run.marked_directories.extend(
                    tab.entries
                        .iter()
                        .filter(|e| e.directory && tab.marked.contains(&e.path))
                        .map(|e| e.path.clone()),
                );
                run.page = page;
                run.worker.request_page(page);
                run.accepted_page = usize::MAX;
                tab.entries.clear();
                tab.visible.clear();
                tab.offset = 0;
                tab.selected = 0;
                tab.loading = true;
                tab.refreshing = false;
                tab.follow_selection = true;
                tab.group_view = None;
                let token = self.token();
                self.tab_mut().request = token;
                self.hits.clear();
                self.drop_regions.clear();
                return;
            }
        }
        let current = FilterOwner::current(self, self.active);
        let token = self.token();
        if let Some(Dialog::FilterMenu(m)) = &mut self.dialog {
            m.owner = current;
            m.token = token;
        }
    }
    pub fn filter_menu_items(&self) -> Vec<(String, FilterControl, bool)> {
        let Some(Dialog::FilterMenu(menu)) = &self.dialog else {
            return vec![];
        };
        let tab = self.file_tab(menu.owner.owner.pane);
        let o = &tab.filter_options;
        if menu.depth {
            let mut rows: Vec<_> = (0..8)
                .map(|n| {
                    let depth = (n < 7).then_some(n);
                    let label = Options { depth, ..o.clone() }.hint();
                    (
                        format!("{} {label}", if o.depth == depth { "●" } else { "○" }),
                        FilterControl::Depth(depth),
                        depth == Some(0) || self.filter_deep_allowed(menu.owner.owner.pane),
                    )
                })
                .collect();
            rows.push((
                "Отдельный поиск · F3".into(),
                FilterControl::SeparateSearch,
                self.filter_deep_allowed(menu.owner.owner.pane) && !self.busy,
            ));
            return rows;
        }
        let check = |b| if b { "√" } else { "□" };
        let mut rows = vec![
            (
                format!("{} С учётом регистра", check(o.case_sensitive)),
                FilterControl::Case,
                true,
            ),
            (
                format!(
                    "{} Сохранять при переходе",
                    check(self.settings.keep_filters_on_navigation)
                ),
                FilterControl::Keep,
                true,
            ),
        ];
        rows.extend([Kind::All, Kind::Files, Kind::Folders].map(|kind| {
            (
                format!(
                    "{} {}",
                    if o.kind == kind { "●" } else { "○" },
                    kind.label()
                ),
                FilterControl::Kind(kind),
                true,
            )
        }));
        rows.push((
            format!("Расширение: .*{}*", o.extension),
            FilterControl::Extension,
            true,
        ));
        rows.push(("Сбросить фильтр".into(), FilterControl::Clear, true));
        rows.push(("Обновить результаты".into(), FilterControl::Retry, true));
        if let Some(run) = &tab.filter_run {
            rows.push((
                "Остановить поиск".into(),
                FilterControl::Stop,
                !run.worker.results.done,
            ));
            rows.push((
                "Предыдущая страница".into(),
                FilterControl::Page(-1),
                run.page > 0,
            ));
            rows.push((
                "Следующая страница".into(),
                FilterControl::Page(1),
                run.page + crate::search::PAGE_SIZE < run.worker.results.total,
            ));
        }
        rows
    }
    pub(super) fn filter_input(&mut self, input: &Input) -> bool {
        if self.inline_filter() && self.editor_menu.is_none() {
            let point = match input {
                Input::PointerDown { x, y, .. }
                | Input::Click { x, y, .. }
                | Input::SecondaryClick { x, y } => Some(Position::new(*x, *y)),
                _ => None,
            };
            if let Some(point) = point
                && self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(point))
                    .is_none_or(|(_, t)| !matches!(t, Target::TextSelect { .. }))
            {
                self.confirm_dialog();
            }
        }
        if let Some(Dialog::FilterMenu(m)) = &self.dialog
            && !m.owner.valid(self)
        {
            self.dialog = None;
            self.hits.clear();
            return true;
        }
        let rows = self.filter_menu_items();
        let Some(Dialog::FilterMenu(m)) = &mut self.dialog else {
            return false;
        };
        if let Input::Key(KeyPress { key, .. }) = input {
            match key {
                Key::Escape => self.cancel_dialog(),
                Key::Home => m.selected = 0,
                Key::End => m.selected = rows.len().saturating_sub(1),
                Key::Up => m.selected = m.selected.saturating_sub(1),
                Key::Down => m.selected = (m.selected + 1).min(rows.len().saturating_sub(1)),
                Key::Enter | Key::Char(' ') => {
                    let owner = m.owner.clone();
                    let token = m.token;
                    if let Some((_, c, true)) = rows.get(m.selected) {
                        self.filter_control(owner, Some(token), *c);
                    }
                }
                _ => {}
            }
            return true;
        }
        false
    }
}

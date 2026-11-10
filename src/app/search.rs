use super::*;
pub struct SearchView {
    pub worker: crate::search::Search,
    pub owner: PathOwner,
    pub request: u64,
    pub query: String,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
}
impl App {
    pub fn recursive_editing(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::RecursiveSearch,
                ..
            })
        )
    }
    pub fn begin_recursive_search(&mut self) {
        if self.in_trash() || self.busy {
            return;
        }
        let query = match &self.dialog {
            Some(Dialog::Edit { text, .. }) => text.clone(),
            _ => String::new(),
        };
        self.cancel_dialog();
        self.search = Some(SearchView {
            worker: Default::default(),
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            query: String::new(),
            selected: 0,
            offset: 0,
            page: 1,
        });
        self.begin_edit(EditKind::RecursiveSearch, query.clone(), 0..query.len());
        self.update_recursive_search();
    }
    pub(super) fn update_recursive_search(&mut self) {
        let Some(Dialog::Edit {
            kind: EditKind::RecursiveSearch,
            text,
            ..
        }) = &self.dialog
        else {
            return;
        };
        if let Some(view) = &mut self.search
            && view.query != *text
        {
            view.query = text.clone();
            view.selected = 0;
            view.offset = 0;
            view.worker
                .start(view.owner.cwd.clone(), text.clone(), self.settings.hidden);
        }
    }
    pub(super) fn poll_search(&mut self) {
        if let Some(view) = &self.search
            && (!self.recursive_editing()
                || !view.owner.valid(self)
                || view.owner.pane != self.active
                || view.request != self.tab().request)
        {
            self.cancel_dialog();
        }
        if let Some(view) = &mut self.search {
            view.worker.poll();
        }
        if let Some((request, path)) = self.search_focus.clone() {
            if self.tab().request != request {
                self.search_focus = None;
            } else if !self.tab().loading {
                if let Some(index) = self
                    .tab()
                    .visible
                    .iter()
                    .position(|&i| self.tab().entries[i].path == path)
                {
                    self.tab_mut().selected = index;
                    self.tab_mut().follow_selection = true;
                }
                self.search_focus = None;
            }
        }
    }
    pub(super) fn search_move(&mut self, delta: isize) {
        if let Some(view) = &mut self.search {
            view.selected = view
                .selected
                .saturating_add_signed(delta)
                .min(view.worker.results.total.saturating_sub(1));
        }
    }
    pub(super) fn search_open(&mut self, index: usize) {
        let Some(view) = &self.search else {
            return;
        };
        if !view.owner.valid(self)
            || view.owner.pane != self.active
            || view.request != self.tab().request
        {
            self.cancel_dialog();
            return;
        }
        let Some(path) = view.worker.results.path(index).cloned() else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        let parent = parent.to_path_buf();
        self.cancel_dialog();
        self.navigate(parent, true);
        self.tab_mut().filter.clear();
        self.search_focus = Some((self.tab().request, path));
    }
}

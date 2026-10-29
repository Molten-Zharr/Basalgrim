use crate::fs::{Entry, Sort};
use crate::operations::Cancellation;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
}
impl Pane {
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

pub struct Tab {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub marked: BTreeSet<PathBuf>,
    /// An explicit set can be empty; keyboard focus still has a position.
    pub explicit_selection: bool,
    pub filter: String,
    pub sort: Sort,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub loading: bool,
    pub refreshing: bool,
    pub error: Option<String>,
    pub request: u64,
    pub offset: usize,
    /// Keyboard navigation reveals focus; wheel scrolling controls only the viewport.
    pub follow_selection: bool,
    pub columns: usize,
    pub page_size: usize,
    pub(crate) scan_cancel: Cancellation,
}

impl Tab {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            entries: vec![],
            visible: vec![],
            selected: 0,
            marked: BTreeSet::new(),
            explicit_selection: false,
            filter: String::new(),
            sort: Sort::Name,
            back: vec![],
            forward: vec![],
            loading: true,
            refreshing: false,
            error: None,
            request: 0,
            offset: 0,
            follow_selection: true,
            columns: 1,
            page_size: 10,
            scan_cancel: Cancellation::default(),
        }
    }
    pub fn entry(&self) -> Option<&Entry> {
        self.visible.get(self.selected).map(|&i| &self.entries[i])
    }
    pub fn targets(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() && !self.explicit_selection {
            self.entry()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            self.marked.iter().cloned().collect()
        }
    }
    pub fn selection_count(&self) -> usize {
        if self.explicit_selection || !self.marked.is_empty() {
            self.marked.len()
        } else {
            usize::from(self.entry().is_some())
        }
    }
    pub fn scroll_rows(&mut self, rows: isize) {
        let columns = self.columns.max(1);
        let page_rows = (self.page_size / columns).max(1);
        let max = self
            .visible
            .len()
            .div_ceil(columns)
            .saturating_sub(page_rows);
        let top = (self.offset / columns).saturating_add_signed(rows).min(max);
        self.offset = top * columns;
        self.follow_selection = false;
    }
    pub fn rebuild(&mut self, hidden: bool) {
        let selected = self.entry().map(|e| e.path.clone());
        self.sort.sort(&mut self.entries);
        let query = self.filter.to_lowercase();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| (hidden || !e.hidden) && e.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.selected = selected
            .and_then(|p| self.visible.iter().position(|&i| self.entries[i].path == p))
            .unwrap_or(self.selected)
            .min(self.visible.len().saturating_sub(1));
        let visible: BTreeSet<_> = self
            .visible
            .iter()
            .map(|&i| self.entries[i].path.clone())
            .collect();
        self.marked.retain(|p| visible.contains(p));
        self.offset = self.offset.min(self.visible.len().saturating_sub(1));
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.scan_cancel.cancel();
    }
}

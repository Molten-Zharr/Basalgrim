use crate::fs::{Entry, Sort};
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
    pub filter: String,
    pub sort: Sort,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub loading: bool,
    pub error: Option<String>,
    pub request: u64,
    pub offset: usize,
    pub columns: usize,
    pub page_size: usize,
}

impl Tab {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            entries: vec![],
            visible: vec![],
            selected: 0,
            marked: BTreeSet::new(),
            filter: String::new(),
            sort: Sort::Name,
            back: vec![],
            forward: vec![],
            loading: true,
            error: None,
            request: 0,
            offset: 0,
            columns: 1,
            page_size: 10,
        }
    }
    pub fn entry(&self) -> Option<&Entry> {
        self.visible.get(self.selected).map(|&i| &self.entries[i])
    }
    pub fn targets(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.entry()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            self.marked.iter().cloned().collect()
        }
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

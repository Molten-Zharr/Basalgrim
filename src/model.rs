use crate::fs::{Entry, Sort};
use crate::operations::Cancellation;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

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
    pub grouping: crate::grouping::Grouping,
    pub groups: Vec<crate::grouping::Group>,
    pub group_view: Option<crate::grouping::Viewport>,
    pub tree: Option<crate::tree::Tree>,
    pub archive: Option<std::sync::Arc<crate::archive::Catalog>>,
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub marked: BTreeSet<PathBuf>,
    /// An explicit set can be empty; keyboard focus still has a position.
    pub explicit_selection: bool,
    pub(crate) range: RangeSelection,
    pub filter: String,
    pub(crate) revealed: BTreeSet<PathBuf>,
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
    /// Nonzero for column-major horizontal layout: entries per column.
    pub column_rows: usize,
    pub page_size: usize,
    pub(crate) scan_cancel: Cancellation,
}

/// Paths, rather than row numbers, survive sorting and background refreshes.
#[derive(Clone, Default)]
pub(crate) struct RangeSelection {
    anchor: Option<PathBuf>,
    additive_base: Option<BTreeSet<PathBuf>>,
}

impl Tab {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            grouping: Default::default(),
            groups: vec![],
            group_view: None,
            tree: None,
            archive: None,
            cwd,
            entries: vec![],
            visible: vec![],
            selected: 0,
            marked: BTreeSet::new(),
            explicit_selection: false,
            range: RangeSelection::default(),
            filter: String::new(),
            revealed: BTreeSet::new(),
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
            column_rows: 0,
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
            let parents: BTreeSet<_> = self
                .entries
                .iter()
                .filter(|e| e.directory && !e.symlink && self.marked.contains(&e.path))
                .map(|e| e.path.as_path())
                .collect();
            self.marked
                .iter()
                .filter(|p| {
                    !p.ancestors()
                        .skip(1)
                        .any(|ancestor| parents.contains(ancestor))
                })
                .cloned()
                .collect()
        }
    }
    pub fn selection_count(&self) -> usize {
        if self.explicit_selection || !self.marked.is_empty() {
            self.marked.len()
        } else {
            usize::from(self.entry().is_some())
        }
    }
    pub(crate) fn reset_range(&mut self) {
        self.range = RangeSelection {
            anchor: self.entry().map(|e| e.path.clone()),
            additive_base: None,
        };
    }
    pub(crate) fn select_range(&mut self, index: usize, additive: bool) {
        if index >= self.visible.len() {
            return;
        }
        let anchor = self
            .range
            .anchor
            .as_ref()
            .and_then(|p| {
                self.visible
                    .iter()
                    .position(|&i| self.entries[i].path == *p)
            })
            .unwrap_or(self.selected.min(self.visible.len() - 1));
        self.range.anchor = Some(self.entries[self.visible[anchor]].path.clone());
        let mut marked = if additive {
            if self.range.additive_base.is_none() {
                self.range.additive_base = Some(self.targets().into_iter().collect());
            }
            self.range.additive_base.clone().unwrap()
        } else {
            self.range.additive_base = None;
            BTreeSet::new()
        };
        marked.extend(
            self.visible[anchor.min(index)..=anchor.max(index)]
                .iter()
                .map(|&i| self.entries[i].path.clone()),
        );
        self.marked = marked;
        self.explicit_selection = true;
        self.selected = index;
    }
    pub(crate) fn retain_range(&mut self) {
        let visible: BTreeSet<_> = self
            .visible
            .iter()
            .map(|&i| &self.entries[i].path)
            .collect();
        if self
            .range
            .anchor
            .as_ref()
            .is_some_and(|p| !visible.contains(p))
        {
            self.range = RangeSelection::default();
        } else if let Some(base) = &mut self.range.additive_base {
            base.retain(|p| visible.contains(p));
        }
    }
    pub(crate) fn rename_range(&mut self, source: &Path, destination: &Path) {
        if let Some(path) = &self.range.anchor
            && let Ok(relative) = path.strip_prefix(source)
        {
            self.range.anchor = Some(destination.join(relative));
        }
        if let Some(base) = &mut self.range.additive_base {
            *base = base
                .iter()
                .map(|p| {
                    p.strip_prefix(source)
                        .map(|r| destination.join(r))
                        .unwrap_or_else(|_| p.clone())
                })
                .collect();
        }
    }
    pub fn scroll_rows(&mut self, rows: isize) {
        if let Some(view) = &mut self.group_view {
            view.scroll(rows);
            self.offset = view.first();
            self.follow_selection = false;
            return;
        }
        if self.column_rows > 0 {
            let stride = self.column_rows;
            let max = self
                .visible
                .len()
                .div_ceil(stride)
                .saturating_sub(self.columns);
            self.offset = (self.offset / stride).saturating_add_signed(rows).min(max) * stride;
            self.follow_selection = false;
            return;
        }
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
    pub fn enable_tree(&mut self, enabled: bool, hidden: bool) {
        if enabled == self.tree.is_some() {
            return;
        }
        let focus = self.entry().map(|e| e.path.clone());
        if enabled {
            self.tree = Some(crate::tree::Tree {
                roots: self.entries.clone(),
                ..Default::default()
            });
        } else if let Some(tree) = self.tree.take() {
            self.entries = tree.roots;
            self.visible.clear();
        }
        self.rebuild(hidden);
        if let Some(path) = focus {
            self.selected = path
                .ancestors()
                .find_map(|p| self.visible.iter().position(|&i| self.entries[i].path == p))
                .unwrap_or(0);
        }
    }
    pub fn regroup(&mut self) {
        self.groups = crate::grouping::project(
            &self.entries,
            &mut self.visible,
            self.grouping,
            self.tree.as_ref().map(|_| self.cwd.as_path()),
        );
        self.group_view = None;
    }
    pub fn rebuild(&mut self, hidden: bool) {
        let top = self
            .visible
            .get(self.offset)
            .map(|&i| self.entries[i].path.clone());
        let selected = self.entry().map(|e| e.path.clone());
        let tree = self.tree.is_some();
        if let Some(tree) = &mut self.tree {
            self.entries = tree.flatten(self.sort, hidden, &self.filter, &self.revealed);
            self.visible.clear();
        } else {
            self.sort.sort(&mut self.entries);
        }
        let query = self.filter.to_lowercase();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (hidden || !e.hidden || self.revealed.contains(&e.path))
                    && (tree || e.name.to_lowercase().contains(&query))
            })
            .map(|(i, _)| i)
            .collect();
        self.regroup();
        if self.grouping.enabled
            && let Some(top) =
                top.and_then(|p| self.visible.iter().position(|&i| self.entries[i].path == p))
        {
            self.offset = top;
        }
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
        self.retain_range();
        self.offset = self.offset.min(self.visible.len().saturating_sub(1));
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.scan_cancel.cancel();
    }
}

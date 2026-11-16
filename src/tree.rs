//! Lazy directory snapshots. Only expanded branches are read and retained.
use crate::{
    fs::{Entry, Sort},
    operations::Cancellation,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[derive(Default)]
pub struct Tree {
    pub roots: Vec<Entry>,
    pub branches: BTreeMap<PathBuf, Branch>,
    pub revision: u64,
}
pub struct Branch {
    pub entries: Vec<Entry>,
    pub request: u64,
    pub cancel: Cancellation,
    pub loading: bool,
    pub dirty: bool,
    pub error: Option<String>,
}
impl Default for Branch {
    fn default() -> Self {
        Self {
            entries: vec![],
            request: 0,
            cancel: Cancellation::default(),
            loading: false,
            dirty: true,
            error: None,
        }
    }
}
impl Drop for Branch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Tree {
    pub fn dirty(&mut self) {
        for branch in self.branches.values_mut() {
            branch.cancel.cancel();
            branch.request = 0;
            branch.loading = false;
            branch.dirty = true;
        }
    }
    pub fn collapse(&mut self, path: &Path) {
        self.branches.retain(|p, _| !p.starts_with(path));
    }
    pub fn flatten(
        &mut self,
        sort: Sort,
        hidden: bool,
        filter: &str,
        revealed: &BTreeSet<PathBuf>,
    ) -> Vec<Entry> {
        sort.sort(&mut self.roots);
        for branch in self.branches.values_mut() {
            sort.sort(&mut branch.entries);
        }
        let mut entries = Vec::new();
        let mut reachable = BTreeSet::new();
        let mut stack: Vec<_> = self.roots.iter().rev().collect();
        while let Some(entry) = stack.pop() {
            // A hidden branch must not leak descendants through the projection.
            if !hidden && entry.hidden && !revealed.contains(&entry.path) {
                continue;
            }
            entries.push(entry.clone());
            if entry.directory
                && !entry.symlink
                && let Some(branch) = self.branches.get(&entry.path)
            {
                reachable.insert(entry.path.clone());
                stack.extend(branch.entries.iter().rev());
            }
        }
        self.branches.retain(|p, _| reachable.contains(p));
        if !filter.is_empty() {
            let query = filter.to_lowercase();
            let mut keep = BTreeSet::new();
            for e in &entries {
                if e.name.to_lowercase().contains(&query) {
                    keep.extend(e.path.ancestors().map(Path::to_path_buf));
                }
            }
            entries.retain(|e| keep.contains(&e.path));
        }
        self.revision = self.revision.wrapping_add(1);
        entries
    }
    pub fn renamed(&mut self, source: &Path, destination: &Path) {
        // Never retain a pending read under a renamed directory. Preserve completed
        // child snapshots under the published path until the ordinary refresh.
        for entry in self.roots.iter_mut().chain(
            self.branches
                .values_mut()
                .flat_map(|b| b.entries.iter_mut()),
        ) {
            if let Ok(relative) = entry.path.strip_prefix(source) {
                entry.path = destination.join(relative);
                entry.name =
                    crate::fs::clean(&entry.path.file_name().unwrap_or_default().to_string_lossy());
                entry.hidden = entry
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .as_encoded_bytes()
                    .starts_with(b".");
            }
        }
        let old = std::mem::take(&mut self.branches);
        for (path, mut branch) in old {
            let path = path
                .strip_prefix(source)
                .map(|r| destination.join(r))
                .unwrap_or(path);
            branch.cancel.cancel();
            branch.request = 0;
            branch.loading = false;
            branch.dirty = true;
            self.branches.insert(path, branch);
        }
    }
}

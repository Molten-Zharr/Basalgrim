use super::*;
use std::hash::{Hash, Hasher};

// Order-independent metadata fingerprint for cache invalidation only. Watch
// registration/polling also requests scans; those hints are not proof of a change.
pub(super) fn fingerprint(entries: &[fs::Entry]) -> (usize, u64, u64) {
    let (mut sum, mut xor) = (0u64, 0u64);
    for entry in entries {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        // Reading a preview or counting a folder can update atime. Display-only
        // metadata must not invalidate a cached size as if content had changed.
        (
            &entry.path,
            &entry.name,
            entry.directory,
            entry.symlink,
            entry.hidden,
            entry.size,
            entry.modified,
        )
            .hash(&mut hash);
        let hash = hash.finish();
        sum = sum.wrapping_add(hash);
        xor ^= hash;
    }
    (entries.len(), sum, xor)
}

impl App {
    pub(super) fn refresh_tab(&mut self, pane: usize, index: usize) {
        let tab = &self.panes[pane].tabs[index];
        // Keep an existing snapshot, including an empty directory or an error.
        // An initial scan or pending navigation has no snapshot to retain.
        let refreshing = !tab.loading || tab.refreshing;
        let error = tab.error.clone();
        self.reload(pane, index);
        let tab = &mut self.panes[pane].tabs[index];
        tab.refreshing = refreshing;
        tab.error = error;
    }

    pub(super) fn observe_directories(&mut self) {
        let paths: BTreeSet<_> = self
            .panes
            .iter()
            .flat_map(|p| &p.tabs)
            .flat_map(|t| {
                let mut paths = vec![
                    t.archive
                        .as_ref()
                        .and_then(|a| a.source.parent())
                        .unwrap_or(&t.cwd)
                        .to_path_buf(),
                ];
                if t.archive.is_none()
                    && let Some(tree) = &t.tree
                {
                    paths.extend(tree.branches.keys().cloned());
                }
                paths
            })
            .collect();
        self.worker.watch(paths.clone());
        let changes = self.worker.directory_changes();
        self.pending_directory_changes.extend(changes.paths);
        self.pending_directory_changes.retain(|p| paths.contains(p));
        if let Some(error) = changes.error {
            self.message(
                format!("Наблюдение за папками: {error}. Используется периодическое обновление."),
                true,
            );
        }
    }

    pub(super) fn directory_updates_deferred(&self) -> bool {
        self.column_drag.is_some()
            || self.dialog.is_some()
            || self.marquee.is_some()
            || self.file_drag.is_some()
            || self.external_drag_active()
            || self.busy
    }

    pub(super) fn refresh_changed_directories(&mut self) {
        if self.directory_updates_deferred() {
            return;
        }
        let ready: Vec<_> = self
            .pending_directory_changes
            .iter()
            .filter(|path| {
                !self.panes.iter().flat_map(|p| &p.tabs).any(|t| {
                    t.archive
                        .as_ref()
                        .and_then(|a| a.source.parent())
                        .unwrap_or(&t.cwd)
                        == *path
                        && t.loading
                })
            })
            .cloned()
            .collect();
        for path in ready {
            self.pending_directory_changes.remove(&path);
            for pane in 0..self.panes.len() {
                for index in 0..self.panes[pane].tabs.len() {
                    let tab = &self.panes[pane].tabs[index];
                    if tab.archive.is_none() && tab.cwd != path {
                        if let Some(branch) = self.panes[pane].tabs[index]
                            .tree
                            .as_mut()
                            .and_then(|t| t.branches.get_mut(&path))
                        {
                            branch.cancel.cancel();
                            branch.request = 0;
                            branch.loading = false;
                            branch.dirty = true;
                        }
                        continue;
                    }
                    if tab
                        .archive
                        .as_ref()
                        .and_then(|a| a.source.parent())
                        .unwrap_or(&tab.cwd)
                        == path
                    {
                        self.refresh_tab(pane, index);
                    }
                }
            }
        }
    }

    pub(super) fn apply_directory_results(&mut self) {
        let deferred = self.directory_updates_deferred();
        let requests: BTreeSet<_> = self
            .panes
            .iter()
            .flat_map(|p| &p.tabs)
            .flat_map(|t| {
                std::iter::once(t.request).chain(
                    t.tree
                        .iter()
                        .flat_map(|t| t.branches.values().map(|b| b.request)),
                )
            })
            .collect();
        self.directory_results
            .retain(|token, _| requests.contains(token));
        let mut preview_changed = false;
        let mut projection_changed = false;
        for (pane_index, pane) in self.panes.iter_mut().enumerate() {
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                if tab.refreshing && deferred {
                    continue;
                }
                let Some(result) = self.directory_results.remove(&tab.request) else {
                    continue;
                };
                projection_changed = true;
                tab.loading = false;
                tab.refreshing = false;
                match result {
                    Ok((cwd, entries, archive)) => {
                        tab.archive = archive;
                        if fingerprint(tab.tree.as_ref().map_or(&tab.entries, |t| &t.roots))
                            != fingerprint(&entries)
                        {
                            self.folder_sizes.invalidate(&cwd);
                        }
                        let focused = tab.entry().map(|e| e.path.clone());
                        let top = tab
                            .visible
                            .get(tab.offset)
                            .map(|&i| tab.entries[i].path.clone());
                        tab.cwd = cwd;
                        if let Some(tree) = &mut tab.tree {
                            tree.roots = entries;
                            tree.dirty();
                        } else {
                            tab.entries = entries;
                        }
                        // Old visible indices refer to the previous snapshot.
                        tab.visible.clear();
                        tab.error = None;
                        tab.rebuild(self.settings.hidden);
                        if let Some(selected) = focused.and_then(|path| {
                            tab.visible
                                .iter()
                                .position(|&i| tab.entries[i].path == path)
                        }) {
                            tab.selected = selected;
                        }
                        if let Some(top) = top.and_then(|path| {
                            tab.visible
                                .iter()
                                .position(|&i| tab.entries[i].path == path)
                        }) {
                            let stride = if tab.column_rows > 0 {
                                tab.column_rows
                            } else {
                                tab.columns.max(1)
                            };
                            tab.offset = if tab.grouping.enabled {
                                top
                            } else {
                                top / stride * stride
                            };
                        }
                        preview_changed |= pane_index == self.active && index == pane.active;
                    }
                    Err(error) => {
                        self.folder_sizes.invalidate(&tab.cwd);
                        tab.error = Some(fs::clean(&error));
                        tab.entries.clear();
                        if let Some(tree) = &mut tab.tree {
                            tree.roots.clear();
                            tree.branches.clear();
                        }
                        tab.visible.clear();
                        tab.marked.clear();
                        tab.reset_range();
                        tab.offset = 0;
                    }
                }
            }
        }
        preview_changed |= self.apply_tree_results();
        if projection_changed || preview_changed {
            self.invalidate_tree_hits();
        }
        if preview_changed
            && self.preview_path == self.tab().entry().map(|e| e.path.clone())
            && let Some(path) = self.preview_path.clone()
        {
            // Same-path writes and permission changes refresh preview content,
            // retaining its scroll position and the old preview while reading.
            self.preview_token = self.token();
            self.preview_loading = true;
            self.preview_cancel.cancel();
            self.preview_cancel = Default::default();
            self.worker.preview_archive(
                self.preview_token,
                path,
                self.icon_backend.supports_images(),
                self.tab().archive.clone(),
                self.preview_cancel.clone(),
            );
        }
    }
}

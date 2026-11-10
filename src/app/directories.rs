use super::*;
use std::hash::{Hash, Hasher};

// Order-independent metadata fingerprint for cache invalidation only. Watch
// registration/polling also requests scans; those hints are not proof of a change.
fn fingerprint(entries: &[fs::Entry]) -> (usize, u64, u64) {
    let (mut sum, mut xor) = (0u64, 0u64);
    for entry in entries {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        entry.hash(&mut hash);
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
            .map(|t| t.cwd.clone())
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

    fn directory_updates_deferred(&self) -> bool {
        self.dialog.is_some()
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
                !self
                    .panes
                    .iter()
                    .flat_map(|p| &p.tabs)
                    .any(|t| &t.cwd == *path && t.loading)
            })
            .cloned()
            .collect();
        for path in ready {
            self.pending_directory_changes.remove(&path);
            for pane in 0..self.panes.len() {
                for index in 0..self.panes[pane].tabs.len() {
                    if self.panes[pane].tabs[index].cwd == path {
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
            .map(|t| t.request)
            .collect();
        self.directory_results
            .retain(|token, _| requests.contains(token));
        let mut preview_changed = false;
        for (pane_index, pane) in self.panes.iter_mut().enumerate() {
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                if tab.refreshing && deferred {
                    continue;
                }
                let Some(result) = self.directory_results.remove(&tab.request) else {
                    continue;
                };
                tab.loading = false;
                tab.refreshing = false;
                match result {
                    Ok((cwd, entries)) => {
                        if fingerprint(&tab.entries) != fingerprint(&entries) {
                            self.folder_sizes.invalidate(&cwd);
                        }
                        let focused = tab.entry().map(|e| e.path.clone());
                        let top = tab
                            .visible
                            .get(tab.offset)
                            .map(|&i| tab.entries[i].path.clone());
                        tab.cwd = cwd;
                        tab.entries = entries;
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
                            tab.offset = top / tab.columns.max(1) * tab.columns.max(1);
                        }
                        preview_changed |= pane_index == self.active && index == pane.active;
                    }
                    Err(error) => {
                        self.folder_sizes.invalidate(&tab.cwd);
                        tab.error = Some(fs::clean(&error));
                        tab.entries.clear();
                        tab.visible.clear();
                        tab.marked.clear();
                        tab.reset_range();
                        tab.offset = 0;
                    }
                }
            }
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
            self.worker.preview_image(
                self.preview_token,
                path,
                self.icon_backend.supports_images(),
                self.preview_cancel.clone(),
            );
        }
    }
}

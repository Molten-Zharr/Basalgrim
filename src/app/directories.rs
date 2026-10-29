use super::*;

impl App {
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
        self.dialog.is_some() || self.marquee.is_some() || self.busy
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
                        let error = self.panes[pane].tabs[index].error.clone();
                        self.reload(pane, index);
                        self.panes[pane].tabs[index].refreshing = true;
                        self.panes[pane].tabs[index].error = error;
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
                        tab.error = Some(fs::clean(&error));
                        tab.entries.clear();
                        tab.visible.clear();
                        tab.marked.clear();
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
            self.worker.preview(self.preview_token, path);
        }
    }
}

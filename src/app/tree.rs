use super::*;
use crate::view::ViewMode;
impl App {
    pub fn toggle_tree(&mut self, owner: PathOwner, request: u64, revision: u64, path: PathBuf) {
        if self.dialog.is_some()
            || self.busy
            || !owner.valid(self)
            || self.trash_in(owner.pane)
            || self.settings.view_mode() != ViewMode::Tree
        {
            return;
        }
        let tab = self.panes[owner.pane].tab();
        if tab.filtered()
            || tab.request != request
            || tab.tree.as_ref().is_none_or(|t| t.revision != revision)
            || !tab
                .entries
                .iter()
                .any(|e| e.path == path && e.directory && !e.symlink)
        {
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.active = owner.pane;
        let hidden = self.settings.hidden;
        let tab = self.panes[owner.pane].tab_mut();
        let selected = tab.entry().map(|e| e.path.clone());
        let tree = tab.tree.as_mut().unwrap();
        if tree.branches.contains_key(&path) {
            tree.collapse(&path);
        } else {
            tree.branches.insert(path.clone(), Default::default());
        }
        tab.rebuild(hidden);
        if let Some(selected) = selected {
            tab.selected = selected
                .ancestors()
                .find_map(|p| tab.visible.iter().position(|&i| tab.entries[i].path == p))
                .unwrap_or(0);
        }
        self.invalidate_tree_hits();
        self.schedule_tree_scans();
        self.update_preview();
    }
    pub(super) fn invalidate_tree_hits(&mut self) {
        self.hits.clear();
        self.drop_regions.clear();
    }
    pub(super) fn schedule_tree_scans(&mut self) {
        if self.directory_updates_deferred() {
            return;
        }
        let mut pending = vec![];
        for (pane, p) in self.panes.iter().enumerate() {
            for (index, tab) in p.tabs.iter().enumerate() {
                if tab.loading || tab.filtered() {
                    continue;
                }
                if let Some(tree) = &tab.tree {
                    for (path, b) in &tree.branches {
                        if b.dirty {
                            pending.push((pane, index, path.clone()));
                        }
                    }
                }
            }
        }
        for (pane, index, path) in pending {
            let token = self.token();
            let tab = &mut self.panes[pane].tabs[index];
            let branch = tab.tree.as_mut().unwrap().branches.get_mut(&path).unwrap();
            branch.cancel.cancel();
            branch.cancel = Default::default();
            branch.request = token;
            branch.loading = true;
            branch.dirty = false;
            branch.error = None;
            self.worker
                .scan_tree(token, path, tab.archive.clone(), branch.cancel.clone());
        }
    }
    pub(super) fn apply_tree_results(&mut self) -> bool {
        if self.directory_updates_deferred() {
            return false;
        }
        let mut changed = false;
        let mut preview = false;
        let mut errors = vec![];
        for (pane_index, pane) in self.panes.iter_mut().enumerate() {
            for (index, tab) in pane.tabs.iter_mut().enumerate() {
                let focus = tab.entry().map(|e| e.path.clone());
                let top = tab
                    .visible
                    .get(tab.offset)
                    .map(|&i| tab.entries[i].path.clone());
                let Some(tree) = &mut tab.tree else {
                    continue;
                };
                let mut updated = false;
                for (path, branch) in &mut tree.branches {
                    let Some(result) = self.directory_results.remove(&branch.request) else {
                        continue;
                    };
                    updated = true;
                    branch.loading = false;
                    match result {
                        Ok((cwd, entries, _)) if cwd == *path => {
                            if super::directories::fingerprint(&branch.entries)
                                != super::directories::fingerprint(&entries)
                            {
                                self.folder_sizes
                                    .listing_changed(path, &branch.entries, &entries);
                            }
                            branch.entries = entries;
                            branch.error = None;
                        }
                        Ok(_) => {
                            branch.entries.clear();
                            branch.error = Some("Папка изменилась во время чтения".into());
                        }
                        Err(e) => {
                            branch.entries.clear();
                            branch.error = Some(fs::clean(&e));
                        }
                    }
                    if let Some(e) = &branch.error {
                        self.folder_sizes.invalidate(path);
                        errors.push(e.clone());
                    }
                }
                if !updated {
                    continue;
                }
                changed = true;
                tab.rebuild(self.settings.hidden);
                if let Some(focus) = focus {
                    tab.selected = focus
                        .ancestors()
                        .find_map(|p| tab.visible.iter().position(|&i| tab.entries[i].path == p))
                        .unwrap_or(0);
                }
                if let Some(top) =
                    top.and_then(|p| tab.visible.iter().position(|&i| tab.entries[i].path == p))
                {
                    tab.offset = top;
                }
                preview |= pane_index == self.active && index == pane.active;
            }
        }
        if changed {
            self.invalidate_tree_hits();
        }
        if let Some(error) = errors.first() {
            self.message(format!("Раскрытие папки: {error}"), true);
        }
        preview
    }
    pub(super) fn tree_arrow(&mut self, right: bool) -> bool {
        if self.settings.view_mode() != ViewMode::Tree || self.in_trash() || self.tab().filtered() {
            return false;
        }
        let Some(entry) = self.tab().entry() else {
            return true;
        };
        let path = entry.path.clone();
        let directory = entry.directory && !entry.symlink;
        let Some(tree) = &self.tab().tree else {
            return true;
        };
        let open = tree.branches.contains_key(&path);
        let revision = tree.revision;
        if directory && ((right && !open) || (!right && open)) {
            self.toggle_tree(
                PathOwner::current(self, self.active),
                self.tab().request,
                revision,
                path,
            );
        } else if right && open {
            if self
                .tab()
                .visible
                .get(self.tab().selected + 1)
                .is_some_and(|&i| self.tab().entries[i].path.parent() == Some(path.as_path()))
            {
                self.move_selection(1, false, false);
            }
        } else if !right
            && let Some(parent) = path.parent()
            && let Some(index) = self
                .tab()
                .visible
                .iter()
                .position(|&i| self.tab().entries[i].path == parent)
        {
            let delta = index as isize - self.tab().selected as isize;
            self.move_selection(delta, false, false);
        }
        true
    }
}

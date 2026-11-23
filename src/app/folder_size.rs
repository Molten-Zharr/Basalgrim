use super::*;
use crate::input::{Key, KeyPress};

impl App {
    fn measurable_folder(&self, pane: usize, entry: &fs::Entry) -> bool {
        entry.directory
            && !entry.symlink
            && !self.trash_in(pane)
            && self.file_tab(pane).archive.is_none()
    }
    pub(crate) fn folder_size_label(&self, pane: usize, entry: &fs::Entry) -> Option<String> {
        (self.settings.auto_folder_sizes && self.measurable_folder(pane, entry))
            .then(|| self.folder_sizes.row_label(&entry.path))
    }
    pub(crate) fn update_visible_folder_sizes(&mut self) {
        let mut paths = BTreeSet::new();
        if self.settings.auto_folder_sizes && self.dialog.is_none() && !self.busy {
            for (_, target) in &self.hits {
                match target {
                    Target::Entry { pane, index } => {
                        let tab = self.file_tab(*pane);
                        let entry = &tab.entries[tab.visible[*index]];
                        if self.measurable_folder(*pane, entry) {
                            paths.insert(entry.path.clone());
                        }
                    }
                    Target::FolderMeasure { path, .. } => {
                        paths.insert(path.clone());
                    }
                    _ => {}
                }
            }
        }
        self.folder_sizes.update_visible(paths);
    }
    pub(super) fn single_folder(&self) -> Option<PathBuf> {
        let targets = self.tab().targets();
        if targets.len() != 1 {
            return None;
        }
        self.tab()
            .entries
            .iter()
            .find(|e| e.path == targets[0] && e.directory && !e.symlink)
            .map(|e| e.path.clone())
    }
    pub(super) fn folder_size_target(&self, background: bool) -> Option<PathBuf> {
        if self.in_trash()
            || self.tab().archive.is_some()
            || self.busy
            || self.tab().loading
            || self.tab().error.is_some()
        {
            return None;
        }
        if background {
            return Some(self.tab().cwd.clone());
        }
        // A group/explicit empty selection must not silently measure a focused item.
        if self.tab().explicit_selection || !self.tab().marked.is_empty() {
            return self.single_folder();
        }
        Some(
            self.single_folder()
                .unwrap_or_else(|| self.tab().cwd.clone()),
        )
    }
    pub fn open_folder_size(&mut self, path: PathBuf) {
        if self.busy || self.in_trash() || self.tab().archive.is_some() {
            return;
        }
        self.folder_sizes.start(path, false);
        self.folder_size_offset = 0;
        self.dialog = Some(Dialog::FolderSize);
    }
    pub fn retry_folder_size(&mut self) {
        if let Some(path) = self.folder_sizes.path.clone() {
            self.folder_sizes.start(path, true);
            self.folder_size_offset = 0;
        }
    }
    pub(super) fn folder_size_key(&mut self, key: KeyPress) {
        if self.settings.action_for(key) == Some(Action::Quit) {
            self.action(Action::Quit);
            return;
        }
        if self.settings.action_for(key) == Some(Action::Refresh) {
            self.retry_folder_size();
            return;
        }
        if key.ctrl || key.alt || key.shift {
            return;
        }
        match key.key {
            Key::Escape => self.cancel_dialog(),
            Key::Char(' ') => self.folder_sizes.stop(),
            Key::Up => self.scroll_folder_size(-1),
            Key::Down => self.scroll_folder_size(1),
            Key::PageUp => self.scroll_folder_size(-5),
            Key::PageDown => self.scroll_folder_size(5),
            _ => {}
        }
    }
    pub(super) fn scroll_folder_size(&mut self, delta: isize) {
        self.folder_size_offset = self
            .folder_size_offset
            .saturating_add_signed(delta)
            .min(self.folder_size_max);
    }
}

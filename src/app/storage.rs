use super::*;
use crate::{
    input::{Key, KeyPress},
    journal::storage::{Catalog, Selection},
    operations::Cancellation,
};

#[derive(Default)]
pub struct StorageView {
    pub catalog: Option<Catalog>,
    pub loading: bool,
    pub error: Option<String>,
    pub offset: usize,
    pub maximum: usize,
    request: u64,
    cancel: Cancellation,
}
impl Drop for StorageView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl StorageView {
    pub(super) fn cancel_request(&mut self) {
        self.cancel.cancel();
        self.loading = false;
        self.catalog = None;
    }
}
impl App {
    pub fn open_history_storage(&mut self) {
        self.dialog = Some(Dialog::HistoryStorage);
        self.refresh_history_storage();
    }
    pub fn refresh_history_storage(&mut self) {
        self.history_storage.cancel.cancel();
        let request = self.token();
        let view = &mut self.history_storage;
        view.request = request;
        view.catalog = None;
        view.error = None;
        view.offset = 0;
        view.cancel = Cancellation::default();
        if !self.journal_enabled || self.journal_error.is_some() {
            view.loading = false;
            view.error = Some("Журнал недоступен".into());
            return;
        }
        view.loading = self.worker.history_storage(request, view.cancel.clone());
        if !view.loading {
            view.error = Some("Исполнитель журнала недоступен".into());
        }
    }
    pub(super) fn accept_history_storage(&mut self, token: u64, result: Result<Catalog, String>) {
        if token != self.history_storage.request || self.history_storage.cancel.is_cancelled() {
            return;
        }
        self.history_storage.loading = false;
        match result {
            Ok(c) => self.history_storage.catalog = Some(c),
            Err(e) => self.history_storage.error = Some(e),
        }
    }
    pub fn storage_selection(&self) -> Selection {
        self.history_storage
            .catalog
            .as_ref()
            .map(|c| c.select(self.settings.history_retention))
            .unwrap_or_default()
    }
    pub fn cycle_storage_policy(&mut self) {
        self.settings.history_retention = self.settings.history_retention.next();
        self.settings_dirty = true;
    }
    pub fn confirm_history_cleanup(&mut self) {
        if self.busy || self.history_storage.loading {
            return;
        }
        let selection = self.storage_selection();
        if !selection.entries.is_empty() {
            self.dialog = Some(Dialog::CleanupHistory(selection));
        }
    }
    pub(super) fn storage_key(&mut self, press: KeyPress) {
        if self.settings.action_for(press) == Some(Action::Quit) {
            self.action(Action::Quit);
            return;
        }
        if self.settings.action_for(press) == Some(Action::Refresh) {
            self.refresh_history_storage();
            return;
        }
        if press.ctrl || press.alt || press.shift {
            return;
        }
        match press.key {
            Key::Escape => self.cancel_dialog(),
            Key::Left | Key::Right | Key::Char('p') => self.cycle_storage_policy(),
            Key::Enter | Key::Delete => self.confirm_history_cleanup(),
            Key::Down => {
                self.history_storage.offset =
                    (self.history_storage.offset + 1).min(self.history_storage.maximum)
            }
            Key::Up => self.history_storage.offset = self.history_storage.offset.saturating_sub(1),
            Key::PageDown => {
                self.history_storage.offset =
                    (self.history_storage.offset + 5).min(self.history_storage.maximum)
            }
            Key::PageUp => {
                self.history_storage.offset = self.history_storage.offset.saturating_sub(5)
            }
            _ => {}
        }
    }
}

use super::*;
use crate::{
    batch_rename::{self, Preview},
    operations::Cancellation,
};
use std::sync::Arc;

pub struct BatchRenameView {
    sources: Arc<Vec<PathBuf>>,
    pattern: String,
    token: u64,
    cancel: Cancellation,
    pub pending: bool,
    pub preview: Option<Preview>,
    pub error: Option<String>,
    pub offset: usize,
    pub page: usize,
    pub scroll_total: usize,
    pub advanced: bool,
    pub numbered: bool,
    simple_text: String,
    advanced_text: Option<String>,
}
impl Drop for BatchRenameView {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl BatchRenameView {
    pub fn count(&self) -> usize {
        self.sources.len()
    }
}

impl App {
    pub fn batch_editing(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::BatchRename,
                ..
            })
        )
    }
    pub(super) fn begin_batch_rename(&mut self) {
        if self.busy || self.clipboard_reads_pending() || self.in_trash() {
            return;
        }
        let tab = self.tab();
        if tab.selection_count() > batch_rename::MAX_ITEMS {
            self.message(
                format!(
                    "Можно переименовать до {} файлов за раз",
                    batch_rename::MAX_ITEMS
                ),
                true,
            );
            return;
        }
        let selected: BTreeSet<_> = tab.targets().into_iter().collect();
        let sources: Vec<_> = tab
            .visible
            .iter()
            .map(|&i| &tab.entries[i].path)
            .filter(|p| selected.contains(*p))
            .take(batch_rename::MAX_ITEMS + 1)
            .cloned()
            .collect();
        if sources.is_empty() || sources.len() > batch_rename::MAX_ITEMS {
            self.message(
                format!(
                    "Можно переименовать до {} файлов за раз",
                    batch_rename::MAX_ITEMS
                ),
                true,
            );
            return;
        }
        self.batch = Some(BatchRenameView {
            sources: Arc::new(sources),
            pattern: String::new(),
            token: 0,
            cancel: Default::default(),
            pending: false,
            preview: None,
            error: None,
            offset: 0,
            page: 1,
            scroll_total: 0,
            advanced: false,
            numbered: true,
            simple_text: String::new(),
            advanced_text: None,
        });
        self.begin_edit(EditKind::BatchRename, String::new(), 0..0);
        self.update_batch_preview(true);
    }
    pub(super) fn update_batch_preview(&mut self, force: bool) {
        let Some(Dialog::Edit {
            kind: EditKind::BatchRename,
            text,
            ..
        }) = &self.dialog
        else {
            return;
        };
        let Some(view) = &self.batch else {
            return;
        };
        let pattern = if view.advanced {
            text.clone()
        } else {
            batch_rename::simple_pattern(text, view.numbered)
        };
        if !force && view.pattern == pattern {
            return;
        }
        let token = self.token();
        let view = self.batch.as_mut().unwrap();
        view.cancel.cancel();
        view.cancel = Default::default();
        view.pattern = pattern.clone();
        view.token = token;
        view.pending = true;
        view.error = None;
        view.preview = None;
        view.offset = 0;
        view.scroll_total = 0;
        self.worker.rename_preview(
            token,
            Arc::clone(&view.sources),
            pattern,
            view.cancel.clone(),
        );
    }
    pub(super) fn accept_rename_preview(&mut self, token: u64, result: Result<Preview, String>) {
        if !self.batch_editing() {
            return;
        }
        let Some(view) = self.batch.as_mut().filter(|v| v.token == token) else {
            return;
        };
        view.pending = false;
        match result {
            Ok(preview) => view.preview = Some(preview),
            Err(error) => view.error = Some(error),
        }
    }
    pub(super) fn number_batch(&mut self) {
        if !self.batch_editing() {
            return;
        }
        let Some(view) = &mut self.batch else {
            return;
        };
        if view.advanced {
            return;
        }
        view.numbered = !view.numbered;
        self.update_batch_preview(true);
    }
    pub(super) fn toggle_batch_mode(&mut self) {
        let Some(Dialog::Edit {
            kind: EditKind::BatchRename,
            text,
            ..
        }) = &self.dialog
        else {
            return;
        };
        let Some(view) = &mut self.batch else {
            return;
        };
        let next = if view.advanced {
            view.advanced_text = Some(text.clone());
            view.simple_text.clone()
        } else {
            view.simple_text = text.clone();
            view.advanced_text
                .clone()
                .unwrap_or_else(|| batch_rename::advanced_pattern(text, view.numbered))
        };
        view.advanced = !view.advanced;
        view.offset = 0;
        // Switching fields must invalidate pending native paste and cursor geometry.
        self.begin_edit(EditKind::BatchRename, next.clone(), 0..next.len());
        self.update_batch_preview(true);
    }
    pub(super) fn scroll_batch(&mut self, delta: isize) {
        if let Some(view) = &mut self.batch {
            view.offset = view
                .offset
                .saturating_add_signed(delta)
                .min(view.scroll_total.saturating_sub(view.page));
        }
    }
    pub(super) fn confirm_batch(&mut self) {
        self.update_batch_preview(false);
        let Some(view) = self.batch.as_ref().filter(|v| !v.pending) else {
            return;
        };
        let Some(plan) = view.preview.as_ref().and_then(Preview::plan) else {
            return;
        };
        self.dialog = None;
        self.batch = None;
        self.start_operation(Operation::BatchRename(plan));
    }
}

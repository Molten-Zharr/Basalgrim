use super::*;

impl App {
    pub fn attach_journal(&mut self, path: PathBuf) {
        self.configure_journal(Some(path));
    }
    pub fn configure_journal(&mut self, path: Option<PathBuf>) {
        self.journal_enabled = true;
        self.journal_path = path.clone();
        self.request_journal(path, true);
    }
    fn request_journal(&mut self, path: Option<PathBuf>, configure: bool) {
        let token = self.token();
        self.journal_request = token;
        self.journal_loading = true;
        if !self.worker.journal(token, path, configure) {
            self.journal_loading = false;
            self.journal_error = Some("Исполнитель журнала недоступен".into());
        }
    }
    pub fn refresh_journal(&mut self) {
        if self.journal_enabled && !self.journal_loading {
            self.request_journal(None, false);
        }
    }
    pub(super) fn accept_journal(
        &mut self,
        token: u64,
        result: Result<Vec<crate::journal::Stored>, String>,
    ) {
        if token != self.journal_request {
            return;
        }
        self.journal_loading = false;
        let records = match result {
            Ok(records) => records,
            Err(error) => {
                let message = format!("Журнал недоступен: {error}");
                self.journal_error = Some(message.clone());
                self.message(message, true);
                return;
            }
        };
        if let Some(error) = self.journal_error.take()
            && self.notice == fs::clean(&error)
        {
            self.message("Журнал загружен", false);
        }
        let selected = self.operations.get(self.operations_selected).map(|r| r.id);
        let ids: BTreeMap<_, _> = self
            .operations
            .iter()
            .filter(|r| r.from_journal)
            .filter_map(|r| r.journal.as_ref().map(|j| (j.id, r.id)))
            .collect();
        self.operations.retain(|r| !r.from_journal);
        for stored in records {
            if self
                .operations
                .iter()
                .any(|r| r.journal.as_ref().is_some_and(|j| j.id == stored.id))
            {
                continue;
            }
            let token = ids.get(&stored.id).copied().unwrap_or_else(|| self.token());
            let record = stored.into_record(token);
            self.operations.push_back(record);
        }
        let running = self.current_operation().map(|r| r.id);
        self.operations.make_contiguous().sort_by_key(|r| {
            std::cmp::Reverse((
                Some(r.id) == running,
                r.recorded_at.unwrap_or(0),
                r.journal.as_ref().map_or(0, |j| j.id),
                r.id,
            ))
        });
        self.operations.truncate(20);
        self.operations_selected = selected
            .and_then(|id| self.operations.iter().position(|r| r.id == id))
            .unwrap_or(0);
    }
}

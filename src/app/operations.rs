use super::*;
use crate::{
    fs::Report,
    operations::{Cancellation, OperationRecord, Progress},
};

pub(super) struct RunningOperation {
    id: u64,
    cancellation: Cancellation,
    cut: bool,
    clipboard_generation: u64,
    rename: Vec<RenameView>,
}
struct RenameView {
    source: PathBuf,
    name: String,
    requests: Vec<u64>,
}
impl Drop for RunningOperation {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl App {
    pub(crate) fn entry_name<'a>(&'a self, pane: usize, entry: &'a fs::Entry) -> &'a str {
        if let Some(rename) = self.running_operation.as_ref().and_then(|r| {
            r.rename.iter().find(|rename| {
                entry.path == rename.source
                    && rename.requests.contains(&self.file_tab(pane).request)
            })
        }) {
            &rename.name
        } else {
            &entry.name
        }
    }

    pub fn replay_candidate(&self, redo: bool) -> Option<(i64, usize)> {
        if self.busy
            || self.clipboard_reads_pending()
            || !self.journal_enabled
            || self.journal_error.is_some()
        {
            return None;
        }
        let available = |record: &OperationRecord| {
            let receipt = record.journal.as_ref()?;
            let count = if redo {
                receipt.history.redo
            } else {
                receipt.history.undo
            };
            (count > 0
                && receipt.history.blocked.is_none()
                && !record.interrupted
                && !record.external)
                .then_some((receipt.history.replay_of.unwrap_or(receipt.id), count))
        };
        if matches!(self.dialog, Some(Dialog::Operations)) {
            self.operations
                .get(self.operations_selected)
                .and_then(available)
        } else {
            self.operations.iter().find_map(available)
        }
    }
    pub fn replay_operation(&mut self, redo: bool) {
        let Some((original, count)) = self.replay_candidate(redo) else {
            self.message(
                if redo {
                    "Нет доступной операции для повтора"
                } else {
                    "Нет доступной операции для отмены"
                },
                true,
            );
            return;
        };
        self.start_operation(Operation::Replay {
            original,
            redo,
            count,
        });
    }
    pub(super) fn other_destination(&self) -> Option<&Path> {
        if !self.settings.dual || self.trash_in(1 - self.active) {
            return None;
        }
        let tab = self.panes[1 - self.active].tab();
        (tab.archive.is_none() && tab.error.is_none() && (!tab.loading || tab.refreshing))
            .then_some(tab.cwd.as_path())
    }

    pub fn current_operation(&self) -> Option<&OperationRecord> {
        let id = self.running_operation.as_ref()?.id;
        self.operations.iter().find(|record| record.id == id)
    }
    pub fn start_operation(&mut self, operation: Operation) {
        if self.busy
            || self.extensions.working.is_some()
            || self.clipboard_reads_pending()
            || self.devices.working.is_some()
            || self.network.working.is_some()
        {
            self.message("Дождитесь завершения или отмените текущую операцию", true);
            return;
        }
        self.folder_sizes.invalidate(Path::new("/"));
        let id = self.token();
        let cancellation = Cancellation::default();
        let cut = matches!(operation, Operation::Transfer { cut: true, .. });
        let renames = match &operation {
            Operation::Rename { source, name } => vec![(source.clone(), name.clone())],
            Operation::BatchRename(plan) => plan
                .items
                .iter()
                .map(|i| {
                    (
                        i.source.clone(),
                        i.destination
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        let rename = renames
            .into_iter()
            .map(|(source, name)| RenameView {
                requests: self
                    .panes
                    .iter()
                    .flat_map(|p| &p.tabs)
                    .filter(|t| source.parent() == Some(t.cwd.as_path()))
                    .map(|t| t.request)
                    .collect(),
                source,
                name,
            })
            .collect();
        let mut record = OperationRecord::new(id, operation.title().into(), operation.total());
        record.destination = operation.destination();
        self.operations.push_front(record);
        self.operations.truncate(20);
        self.operations_selected = 0;
        self.operations_offset = 0;
        self.operations_detail_offset = 0;
        self.running_operation = Some(RunningOperation {
            id,
            cancellation: cancellation.clone(),
            cut,
            clipboard_generation: self.file_exchange.generation,
            rename,
        });
        self.busy = true;
        self.conflict_sequence = 0;
        self.message("Файловая операция выполняется…", false);
        if !self.worker.operate(id, operation, cancellation) {
            self.finish_operation(
                id,
                Report {
                    total: self.operations[0].progress.items_total,
                    error: Some("Исполнитель файловых операций недоступен".into()),
                    ..Default::default()
                },
            );
        }
    }
    pub fn cancel_operation(&mut self) {
        if self.external_drop_reading() {
            self.cancel_external_drop();
            self.message("Чтение перетаскивания отменено", false);
            return;
        }
        if self.cancel_clipboard_read() {
            return;
        }
        let Some(running) = &self.running_operation else {
            return;
        };
        running.cancellation.cancel();
        if let Some(record) = self
            .operations
            .iter_mut()
            .find(|record| record.id == running.id)
        {
            record.cancel_requested = true;
        }
        self.message(
            "Запрошена отмена; выполняется безопасное завершение текущего шага",
            false,
        );
    }
    pub(super) fn accept_operation_progress(&mut self, token: u64, progress: Progress) {
        if self
            .running_operation
            .as_ref()
            .is_some_and(|running| running.id == token)
            && let Some(record) = self.operations.iter_mut().find(|record| record.id == token)
        {
            record.progress = progress;
        }
    }
    pub(super) fn finish_operation(&mut self, token: u64, report: Report) {
        self.folder_sizes.invalidate(Path::new("/"));
        if !self
            .running_operation
            .as_ref()
            .is_some_and(|running| running.id == token)
        {
            return;
        }
        let running = self.running_operation.take().unwrap();
        self.finish_conflict(token);
        self.busy = false;
        self.operation_count += 1;
        self.extension_event(
            "operation_finished",
            Some(format!(
                "completed={} total={} cancelled={} failed={}",
                report.completed.len(),
                report.total,
                report.cancelled,
                report.error.is_some()
            )),
        );
        // Publish the confirmed name into the existing snapshot before removing
        // its pending label. A later scan can sort once while retaining focus.
        // Only touch snapshots captured at dispatch, never a newly navigated tab.
        let requested: BTreeMap<_, _> = running.rename.iter().map(|r| (&r.source, r)).collect();
        let published: BTreeMap<_, _> = report
            .renamed
            .iter()
            .chain(&report.batch_renamed)
            .filter_map(|(source, destination)| {
                requested.get(source).map(|r| (source, (destination, *r)))
            })
            .collect();
        for tab in self.panes.iter_mut().flat_map(|p| &mut p.tabs) {
            let mut rebound = Vec::new();
            for entry in &mut tab.entries {
                if let Some((destination, rename)) = published.get(&entry.path)
                    && rename.requests.contains(&tab.request)
                {
                    let source = std::mem::replace(&mut entry.path, (*destination).clone());
                    entry.name = fs::clean(&destination.file_name().unwrap().to_string_lossy());
                    entry.hidden = destination
                        .file_name()
                        .unwrap()
                        .as_encoded_bytes()
                        .starts_with(b".");
                    rebound.push((source, (*destination).clone()));
                }
            }
            for (source, destination) in rebound {
                if tab.marked.remove(&source) {
                    tab.marked.insert(destination.clone());
                }
                tab.rename_range(&source, &destination);
            }
        }
        if let Some((destination, _)) = self.preview_path.as_ref().and_then(|p| published.get(p))
            && self.tab().entry().is_some_and(|e| e.path == **destination)
        {
            self.preview_cancel.cancel();
            self.preview_token = self.token();
            self.preview_loading = false;
            self.preview_path = Some((*destination).clone());
        }
        if running.cut
            && running.clipboard_generation == self.file_exchange.generation
            && let Some(clip) = &mut self.clipboard
            && clip.cut
        {
            clip.paths.retain(|path| !report.completed.contains(path));
            for path in &mut clip.paths {
                if let Some(recovery) = report
                    .retained_copies
                    .iter()
                    .find(|copy| copy.source == *path)
                    .and_then(|copy| copy.recovery.as_ref())
                {
                    // The original name may now belong to another program's
                    // new file. A retry must use the retained source instead.
                    *path = recovery.clone();
                }
            }
            self.clipboard_pruned();
        }
        let mut summary = format!(
            "{} · готово {}/{}",
            if report.cancelled {
                "Отменено"
            } else if report.error.is_some() {
                "Ошибка"
            } else {
                "Завершено"
            },
            report.completed.len(),
            report.total
        );
        if !report.retained_copies.is_empty() {
            summary.push_str(&format!(
                " · копий без завершения переноса: {} (подробности в операциях)",
                report.retained_copies.len()
            ));
        }
        if !report.skipped.is_empty() {
            summary.push_str(&format!(" · пропущено: {}", report.skipped.len()));
        }
        if !report.destination_recoveries.is_empty() {
            summary.push_str(" · остаток прежней цели сохранён (подробности в операциях)");
        }
        self.message(
            if let Some(error) = &report.error {
                format!(
                    "{} · {summary} · {error}",
                    crate::operations::error_summary(error)
                )
            } else {
                summary
            },
            report.error.is_some(),
        );
        if let Some(record) = self.operations.iter_mut().find(|record| record.id == token) {
            record.finish(report);
        }
        for pane in 0..2 {
            for tab in 0..self.panes[pane].tabs.len() {
                self.refresh_tab(pane, tab);
            }
        }
        if self.trash.is_some() {
            self.refresh_trash();
        }
        self.refresh_journal();
    }
    pub(super) fn operations_key(&mut self, press: crate::input::KeyPress) {
        use crate::input::Key;
        match self.settings.action_for(press) {
            Some(Action::HistoryStorage) => {
                self.open_history_storage();
                return;
            }
            Some(Action::Undo) => {
                self.replay_operation(false);
                return;
            }
            Some(Action::Redo) => {
                self.replay_operation(true);
                return;
            }
            Some(Action::CancelOperation) => {
                self.cancel_operation();
                return;
            }
            Some(Action::Operations) => {
                self.cancel_dialog();
                return;
            }
            Some(Action::Refresh) => {
                self.refresh_journal();
                return;
            }
            Some(Action::Quit) => {
                self.action(Action::Quit);
                return;
            }
            _ => {}
        }
        if press.ctrl || press.alt || press.shift {
            return;
        }
        let last = self.operations.len().saturating_sub(1);
        if matches!(press.key, Key::PageUp | Key::PageDown) {
            self.operations_detail_offset = if press.key == Key::PageDown {
                self.operations_detail_offset
                    .saturating_add(10)
                    .min(self.operations_detail_max)
            } else {
                self.operations_detail_offset.saturating_sub(10)
            };
            return;
        }
        let previous = self.operations_selected;
        self.operations_selected = match press.key {
            Key::Up => self.operations_selected.saturating_sub(1),
            Key::Down => self.operations_selected.saturating_add(1).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::Escape => {
                self.cancel_dialog();
                return;
            }
            _ => self.operations_selected,
        };
        if self.operations_selected != previous {
            self.operations_detail_offset = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        input::{Input, Key, KeyPress},
        operations::Phase,
    };

    fn running(root: &std::path::Path) -> (App, Cancellation) {
        let mut app = App::new(root.to_path_buf(), Settings::default());
        let cancellation = Cancellation::default();
        app.running_operation = Some(RunningOperation {
            id: 42,
            cancellation: cancellation.clone(),
            cut: true,
            clipboard_generation: app.file_exchange.generation,
            rename: Vec::new(),
        });
        app.operations
            .push_front(OperationRecord::new(42, "Перемещение".into(), 2));
        app.busy = true;
        (app, cancellation)
    }

    #[test]
    fn rename_completion_keeps_file_panes_visible_while_the_new_snapshot_loads() {
        use ratatui::{Terminal, backend::TestBackend};
        use std::time::{Duration, Instant};

        for grid in [false, true] {
            for (width, height) in [(34, 14), (160, 44)] {
                let root = tempfile::tempdir().unwrap();
                for index in 0..80 {
                    std::fs::create_dir(root.path().join(format!("dir-{index:03}"))).unwrap();
                }
                let (mut app, _) = running(root.path());
                app.settings.grid = grid;
                app.settings.dual = true;
                app.settings.preview = false;
                let deadline = Instant::now() + Duration::from_secs(5);
                while app.panes.iter().flat_map(|p| &p.tabs).any(|t| t.loading) {
                    app.tick();
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                for pane in &mut app.panes {
                    let tab = pane.tab_mut();
                    tab.selected = 12;
                    tab.offset = 10;
                    tab.follow_selection = false;
                    tab.marked.insert(root.path().join("dir-015"));
                }
                let source = root.path().join("dir-012");
                let destination = root.path().join("dir-012-renamed");
                app.running_operation.as_mut().unwrap().rename = vec![RenameView {
                    source: source.clone(),
                    name: "dir-012-renamed".into(),
                    requests: app
                        .panes
                        .iter()
                        .flat_map(|p| &p.tabs)
                        .map(|t| t.request)
                        .collect(),
                }];
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| crate::ui::render(&mut app, f)).unwrap();
                let before = terminal.backend().buffer().clone();
                let areas = app.file_areas;
                let offsets = app.panes.each_ref().map(|p| p.tab().offset);
                assert_eq!(app.tab().entry().unwrap().path, source);
                std::fs::rename(&source, &destination).unwrap();
                // Stop exactly between completion and directory result handling,
                // even when the worker can scan this small fixture immediately.
                app.finish_operation(
                    42,
                    Report {
                        total: 1,
                        renamed: Some((source.clone(), destination.clone())),
                        completed: vec![source],
                        ..Default::default()
                    },
                );
                assert_eq!(app.tab().entry().unwrap().path, destination);
                terminal.draw(|f| crate::ui::render(&mut app, f)).unwrap();
                assert_eq!(app.file_areas, areas, "refresh collapsed file viewports");
                for area in areas {
                    for y in area.y..area.bottom() {
                        for x in area.x..area.right() {
                            assert_eq!(
                                terminal.backend().buffer()[(x, y)],
                                before[(x, y)],
                                "refresh blanked or moved file content at {x},{y}"
                            );
                        }
                    }
                }
                while app.loading() {
                    app.tick();
                    terminal.draw(|f| crate::ui::render(&mut app, f)).unwrap();
                    assert_eq!(app.file_areas, areas);
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(app.panes.each_ref().map(|p| p.tab().offset), offsets);
                for pane in &app.panes {
                    assert!(pane.tab().entries.iter().any(|e| e.path == destination));
                    assert!(pane.tab().marked.contains(&root.path().join("dir-015")));
                    assert!(!pane.tab().follow_selection);
                }
            }
        }
    }

    #[test]
    fn operation_refresh_keeps_preview_but_does_not_reuse_a_previous_directory() {
        use std::time::{Duration, Instant};

        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("read.txt"), "preview content\n".repeat(80)).unwrap();
        let (mut app, _) = running(root.path());
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.panes.iter().flat_map(|p| &p.tabs).any(|t| t.loading) || app.preview_loading {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.preview.is_some());
        app.preview_offset = 7;
        app.dialog = Some(Dialog::Help);
        let arrival = root.path().join("zzz-new");
        std::fs::write(&arrival, "external change").unwrap();
        app.finish_operation(
            42,
            Report {
                total: 1,
                error: Some("Destination exists".into()),
                ..Default::default()
            },
        );
        app.update_preview();
        assert!(app.preview.is_some(), "refresh blanked the current preview");
        assert_eq!(app.preview_offset, 7);
        while app.loading() {
            app.tick();
            assert!(Instant::now() < deadline, "finished scans must become idle");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(app.dialog, Some(Dialog::Help)));
        assert!(!app.tab().entries.iter().any(|e| e.path == arrival));
        app.cancel_dialog();
        app.tick();
        assert!(app.tab().entries.iter().any(|e| e.path == arrival));
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        app.navigate(other, true);
        app.refresh_tab(0, 0);
        assert!(app.tab().loading && !app.tab().refreshing);
        assert!(app.tab().entries.is_empty());
        assert!(app.preview.is_none());
        while app.tab().loading {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        // A loaded empty directory is also a real snapshot: do not flash a
        // loading screen when an operation refreshes it.
        app.refresh_tab(0, 0);
        assert!(app.tab().refreshing);
    }

    #[test]
    fn incomplete_move_clipboard_uses_recovery_data_instead_of_a_new_source_path_occupant() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = running(root.path());
        let source = root.path().join("source");
        let recovery = root.path().join("recovery/source");
        std::fs::create_dir(recovery.parent().unwrap()).unwrap();
        std::fs::write(&source, "new occupant").unwrap();
        std::fs::write(&recovery, "remaining original").unwrap();
        app.clipboard = Some(Clipboard {
            paths: vec![source.clone()],
            cut: true,
        });
        app.finish_operation(
            42,
            Report {
                total: 1,
                retained_copies: vec![crate::fs::RetainedCopy {
                    source: source.clone(),
                    destination: root.path().join("destination/source"),
                    recovery: Some(recovery.clone()),
                }],
                error: Some("Source restore was blocked".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths.as_slice(),
            std::slice::from_ref(&recovery)
        );
        assert_eq!(std::fs::read_to_string(source).unwrap(), "new occupant");
        assert_eq!(
            std::fs::read_to_string(recovery).unwrap(),
            "remaining original"
        );
        assert!(app.operations[0].completed_paths.is_empty());
    }

    #[test]
    fn stale_updates_cannot_finish_or_rewrite_the_active_operation() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, cancellation) = running(root.path());
        let first = root.path().join("first");
        let second = root.path().join("second");
        app.clipboard = Some(Clipboard {
            paths: vec![first.clone(), second.clone()],
            cut: true,
        });
        app.accept_operation_progress(
            41,
            Progress {
                items_done: 99,
                ..Default::default()
            },
        );
        app.finish_operation(
            41,
            Report {
                completed: vec![second.clone()],
                total: 2,
                ..Default::default()
            },
        );
        assert_eq!(app.operations[0].progress.items_done, 0);
        assert!(app.busy);
        assert_eq!(app.operation_count, 0);
        app.cancel_operation();
        assert!(cancellation.is_cancelled());
        assert!(app.operations[0].cancel_requested);
        app.finish_operation(
            42,
            Report {
                completed: vec![first.clone()],
                total: 2,
                cancelled: true,
                progress: Progress {
                    phase: Phase::Finished,
                    items_done: 1,
                    items_total: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert!(!app.busy);
        assert_eq!(app.operation_count, 1);
        assert_eq!(app.clipboard.as_ref().unwrap().paths, [second]);
        assert_eq!(app.operations[0].completed_paths, [first]);
        assert_eq!(app.operations[0].status(), "Отменено");
        app.accept_operation_progress(42, Progress::default());
        app.finish_operation(42, Report::default());
        assert_eq!(app.operations[0].progress.items_done, 1);
        assert_eq!(app.operation_count, 1);
    }

    #[test]
    fn progress_bar_and_cancel_button_use_shared_hit_regions_without_hover_side_effects() {
        use ratatui::{Terminal, backend::TestBackend};
        let root = tempfile::tempdir().unwrap();
        let (mut app, cancellation) = running(root.path());
        app.operations[0].progress = Progress {
            phase: Phase::Running,
            items_done: 1,
            items_total: 2,
            bytes_done: 1_048_576,
            bytes_total: Some(4_194_304),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(150, 40)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        let area = app
            .hits
            .iter()
            .find_map(|(area, target)| {
                matches!(target, Target::Action(Action::CancelOperation)).then_some(*area)
            })
            .unwrap();
        app.handle(Input::Pointer(Some((area.x, area.y).into())));
        terminal
            .draw(|frame| crate::ui::render(&mut app, frame))
            .unwrap();
        assert!(!cancellation.is_cancelled());
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Готово 1/2"));
        assert!(text.contains("1.0 MiB"));
        app.handle(Input::Click {
            x: area.x,
            y: area.y,
            double: false,
            toggle: false,
        });
        assert!(cancellation.is_cancelled());
        assert!(app.busy);
    }

    #[test]
    fn cancellation_shortcut_is_captured_before_dispatch_and_escape_only_closes_the_screen() {
        let root = tempfile::tempdir().unwrap();
        let (mut app, cancellation) = running(root.path());
        app.dialog = Some(Dialog::CaptureBinding {
            index: 0,
            error: None,
        });
        let cancel = app
            .settings
            .bindings
            .iter()
            .find(|binding| binding.action == Action::CancelOperation)
            .unwrap()
            .key;
        app.handle(Input::Key(cancel));
        assert!(!cancellation.is_cancelled());
        assert!(matches!(
            app.dialog,
            Some(Dialog::CaptureBinding { error: Some(_), .. })
        ));
        app.dialog = Some(Dialog::Operations);
        app.handle(Input::Key(KeyPress::plain(Key::Escape)));
        assert!(app.dialog.is_none());
        assert!(!cancellation.is_cancelled());
        assert!(app.busy);
        app.handle(Input::Key(cancel));
        assert!(cancellation.is_cancelled());
        assert!(app.busy, "Cancellation must wait for worker cleanup");
    }
}

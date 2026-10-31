use super::*;
use crate::{
    fs::Report,
    operations::{Cancellation, OperationRecord, Progress},
};

pub(super) struct RunningOperation {
    id: u64,
    cancellation: Cancellation,
    cut: bool,
}
impl Drop for RunningOperation {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl App {
    pub(super) fn other_destination(&self) -> Option<&Path> {
        if !self.settings.dual {
            return None;
        }
        let tab = self.panes[1 - self.active].tab();
        (tab.error.is_none() && (!tab.loading || tab.refreshing)).then_some(tab.cwd.as_path())
    }

    pub fn current_operation(&self) -> Option<&OperationRecord> {
        let id = self.running_operation.as_ref()?.id;
        self.operations.iter().find(|record| record.id == id)
    }
    pub fn start_operation(&mut self, operation: Operation) {
        if self.busy {
            self.message("Дождитесь завершения или отмените текущую операцию", true);
            return;
        }
        let id = self.token();
        let cancellation = Cancellation::default();
        let cut = matches!(operation, Operation::Transfer { cut: true, .. });
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
        if running.cut
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
                format!("{summary} · {error}")
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
                self.reload(pane, tab);
            }
        }
        self.preview_path = None;
    }
    pub(super) fn operations_key(&mut self, press: crate::input::KeyPress) {
        use crate::input::Key;
        match self.settings.action_for(press) {
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
        });
        app.operations
            .push_front(OperationRecord::new(42, "Перемещение".into(), 2));
        app.busy = true;
        (app, cancellation)
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

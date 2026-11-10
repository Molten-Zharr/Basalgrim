use super::*;
use crate::clipboard::service::{Event as ClipboardEvent, Incoming, Offer, Payload, Request};
use std::collections::{HashMap, VecDeque};

#[derive(Default)]
pub(super) struct Exchange {
    connecting: bool,
    enabled: bool,
    offer: Option<Offer>,
    pub generation: u64,
    failed: bool,
    pending: Option<(u64, PathBuf)>,
    imported: Option<(u64, u64)>,
    queue: VecDeque<Request>,
    writes: HashMap<u64, (u64, bool)>,
}
impl App {
    pub fn clipboard_connecting(&mut self) {
        self.file_exchange.connecting = true;
    }
    pub fn clipboard_reads_pending(&self) -> bool {
        self.file_exchange.pending.is_some()
    }
    pub fn native_text_clipboard(&self) -> bool {
        self.file_exchange.enabled || self.file_exchange.connecting
    }
    pub fn native_text_paste(&self) -> bool {
        self.file_exchange.enabled
    }
    pub(super) fn queue_text_read(&mut self, id: u64) {
        self.file_exchange
            .queue
            .retain(|r| !matches!(r, Request::ReadText { .. }));
        self.file_exchange.queue.push_back(Request::ReadText { id });
    }
    pub(super) fn can_paste(&self) -> bool {
        if self.busy || self.clipboard_reads_pending() {
            return false;
        }
        let internal = self.clipboard.as_ref().is_some_and(|c| !c.paths.is_empty());
        let e = &self.file_exchange;
        if !e.enabled || e.failed {
            return internal;
        }
        e.offer.as_ref().is_some_and(|o| {
            if o.local == Some(e.generation) {
                internal
            } else {
                o.files
            }
        })
    }
    pub(super) fn files_copied(&mut self, clip: Clipboard) {
        let generation = self.token();
        self.cancel_clipboard_read();
        let writes = &mut self.file_exchange.writes;
        self.file_exchange.queue.retain(|r| {
            if let Request::Write {
                id,
                payload: Payload::Files(_),
                ..
            } = r
            {
                writes.remove(id);
                false
            } else {
                true
            }
        });
        self.file_exchange.pending = None;
        self.file_exchange.generation = generation;
        self.file_exchange.imported = None;
        self.file_exchange.failed = false;
        self.clipboard = Some(clip);
        if self.file_exchange.enabled {
            self.export_files(None);
        }
    }
    fn export_files(&mut self, expected: Option<u64>) {
        let Some(clip) = self.clipboard.clone() else {
            return;
        };
        let id = self.token();
        let generation = self.file_exchange.generation;
        self.file_exchange.writes.insert(id, (generation, false));
        self.file_exchange.queue.push_back(Request::Write {
            id,
            generation,
            payload: Payload::Files(clip),
            expected,
        });
    }
    pub(super) fn clipboard_pruned(&mut self) {
        if self.file_exchange.enabled && self.file_exchange.imported.is_none() {
            self.export_files(Some(self.file_exchange.generation));
        }
    }
    pub(super) fn paste_files(&mut self) {
        if self.busy {
            return;
        }
        if self.file_exchange.enabled && !self.file_exchange.failed {
            let id = self.token();
            self.file_exchange.pending = Some((id, self.tab().cwd.clone()));
            // A second Paste supersedes an unread request and retains its own destination.
            self.file_exchange
                .queue
                .retain(|r| !matches!(r, Request::Read { .. }));
            self.file_exchange.queue.push_back(Request::Read { id });
            self.message("Чтение файлового буфера…", false);
        } else {
            self.paste_internal(self.tab().cwd.clone());
        }
    }
    fn paste_internal(&mut self, destination: PathBuf) {
        if let Some(clip) = &self.clipboard
            && !clip.paths.is_empty()
        {
            self.start_operation(Operation::Transfer {
                sources: clip.paths.clone(),
                destination,
                cut: clip.cut,
            });
        } else {
            self.message("В буфере нет файлов для вставки", false);
        }
    }
    pub(super) fn cancel_clipboard_read(&mut self) -> bool {
        let pending = self.file_exchange.pending.take().is_some();
        self.file_exchange
            .queue
            .retain(|r| !matches!(r, Request::Read { .. }));
        if pending {
            self.message("Вставка отменена", false);
        }
        pending
    }
    pub fn accept_clipboard_event(&mut self, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Ready(result) => {
                self.file_exchange.connecting = false;
                self.file_exchange.enabled = result.is_ok();
                if result.is_ok() {
                    if self.clipboard.is_some() {
                        self.export_files(None);
                    }
                } else {
                    if let Some((id, _)) = self.text_paste {
                        self.finish_text_paste(
                            id,
                            Err("Системный текстовый буфер недоступен".into()),
                        );
                    }
                    if let Some((id, _)) = self.terminal.paste {
                        self.finish_terminal_paste(
                            id,
                            Err("Системный текстовый буфер недоступен".into()),
                        );
                    }
                    self.cancel_clipboard_read();
                    let text_ids = self
                        .file_exchange
                        .writes
                        .iter()
                        .filter_map(|(id, (_, text))| text.then_some(*id))
                        .collect::<Vec<_>>();
                    for id in text_ids {
                        self.finish_text_copy(id, Err("Системный буфер отключён".into()));
                    }
                    self.file_exchange.queue.clear();
                    self.file_exchange.writes.clear();
                    // Native support is optional; keep the internal clipboard and text fallback.
                }
            }
            ClipboardEvent::Offer(offer) => self.file_exchange.offer = Some(offer),
            ClipboardEvent::Text { id, result } => self.finish_text_paste(id, result),
            ClipboardEvent::Written { id, result } => {
                let Some((generation, text)) = self.file_exchange.writes.remove(&id) else {
                    return;
                };
                if text {
                    self.finish_text_copy(
                        id,
                        result.map(|()| crate::clipboard::Destination::System),
                    );
                } else if generation == self.file_exchange.generation {
                    self.file_exchange.failed = result.is_err();
                    if let Err(error) = result {
                        self.message(
                            format!("Файлы во внутреннем буфере; системный недоступен: {error}"),
                            true,
                        );
                    }
                }
            }
            ClipboardEvent::Read { id, result } => {
                if !self
                    .file_exchange
                    .pending
                    .as_ref()
                    .is_some_and(|(current, _)| *current == id)
                {
                    return;
                }
                let (_, destination) = self.file_exchange.pending.take().unwrap();
                match result {
                    Ok(Incoming::Local(generation))
                        if generation == self.file_exchange.generation =>
                    {
                        self.paste_internal(destination)
                    }
                    Ok(Incoming::External {
                        stamp,
                        files: Some(files),
                    }) => {
                        if self.file_exchange.imported != Some(stamp) {
                            self.file_exchange.generation = self.token();
                            self.file_exchange.imported = Some(stamp);
                            self.clipboard = Some(files);
                        }
                        self.paste_internal(destination);
                    }
                    Ok(_) => self.message("В системном буфере нет файлов для вставки", false),
                    Err(error) => self.message(
                        format!("Не удалось прочитать файловый буфер: {error}"),
                        true,
                    ),
                }
            }
        }
    }
    #[cfg(any(feature = "file-clipboard", test))]
    fn queue_text_copy(&mut self) {
        if let Some(copy) = self.text_copy.take() {
            self.file_exchange
                .writes
                .insert(copy.id, (self.file_exchange.generation, true));
            let position = self
                .file_exchange
                .queue
                .iter()
                .position(|request| {
                    let (Request::Write { id, .. }
                    | Request::Read { id }
                    | Request::ReadText { id }) = request;
                    *id > copy.id
                })
                .unwrap_or(self.file_exchange.queue.len());
            self.file_exchange.queue.insert(
                position,
                Request::Write {
                    id: copy.id,
                    generation: self.file_exchange.generation,
                    payload: Payload::Text(copy.text),
                    expected: None,
                },
            );
        }
    }
    #[cfg(feature = "file-clipboard")]
    pub fn pump_clipboard(&mut self, runtime: &crate::clipboard::native::Runtime) {
        if !self.clipboard_reads_pending() {
            runtime.cancel_reads();
        }
        if !self.text_paste_pending() {
            runtime.cancel_text_reads();
        }
        while let Ok(event) = runtime.events.try_recv() {
            self.accept_clipboard_event(event);
        }
        if !self.file_exchange.enabled {
            return;
        }
        self.queue_text_copy();
        while let Some(request) = self.file_exchange.queue.pop_front() {
            match runtime.submit(request) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(request)) => {
                    self.file_exchange.queue.push_front(request);
                    break;
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    self.accept_clipboard_event(ClipboardEvent::Ready(
                        Err("Буфер отключён".into()),
                    ));
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::files::FileList;
    use ratatui::{Terminal, backend::TestBackend};
    use std::{
        fs,
        time::{Duration, Instant},
    };
    fn settle(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.loading() {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn fixture() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("one"), "one").unwrap();
        let mut app = App::new(
            dest.clone(),
            Settings {
                preview: false,
                ..Default::default()
            },
        );
        settle(&mut app);
        app.accept_clipboard_event(ClipboardEvent::Ready(Ok(())));
        (temp, app, source, dest)
    }
    fn read_id(app: &App) -> u64 {
        app.file_exchange.pending.as_ref().unwrap().0
    }
    fn incoming(source: &Path, stamp: (u64, u64), cut: bool) -> Incoming {
        Incoming::External {
            stamp,
            files: Some(FileList {
                paths: vec![source.join("one")],
                cut,
            }),
        }
    }
    #[test]
    fn external_paste_has_mouse_command_at_minimum_size_and_captures_destination() {
        let (_temp, mut app, source, dest) = fixture();
        app.accept_clipboard_event(ClipboardEvent::Offer(Offer {
            version: 1,
            files: true,
            local: None,
        }));
        app.open_commands(Position::new(2, 3), MenuContext::Directory);
        let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
        terminal.draw(|f| crate::ui::render(&mut app, f)).unwrap();
        let index = app
            .command_items()
            .iter()
            .position(|item| item.action == Action::Paste && item.enabled)
            .unwrap();
        assert!(
            app.hits
                .iter()
                .any(|(_, t)| matches!(t,Target::Command(i) if *i==index))
        );
        app.dialog = None;
        app.action(Action::Paste);
        let id = read_id(&app);
        app.navigate(source.clone(), true);
        app.accept_clipboard_event(ClipboardEvent::Read {
            id,
            result: Ok(incoming(&source, (1, 1), false)),
        });
        settle(&mut app);
        assert_eq!(fs::read_to_string(dest.join("one")).unwrap(), "one");
        assert!(source.join("one").exists());
    }
    #[test]
    fn clipboard_read_is_cancellable_and_stale_responses_cannot_start_operations() {
        let (_temp, mut app, source, dest) = fixture();
        app.action(Action::Paste);
        let id = read_id(&app);
        let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
        terminal.draw(|f| crate::ui::render(&mut app, f)).unwrap();
        let area = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::Action(Action::CancelOperation)).then_some(*r))
            .unwrap();
        app.handle(crate::input::Input::PointerDown {
            x: area.x,
            y: area.y,
            double: false,
            toggle: false,
        });
        app.handle(crate::input::Input::PointerUp {
            x: area.x,
            y: area.y,
        });
        app.accept_clipboard_event(ClipboardEvent::Read {
            id,
            result: Ok(incoming(&source, (1, 1), false)),
        });
        assert!(!app.busy && !dest.join("one").exists());
        app.action(Action::Paste);
        let id = read_id(&app);
        app.files_copied(FileList {
            paths: vec![source.join("one")],
            cut: false,
        });
        app.accept_clipboard_event(ClipboardEvent::Read {
            id,
            result: Ok(incoming(&source, (1, 1), false)),
        });
        assert!(!app.busy);
    }
    #[test]
    fn external_text_never_pastes_stale_internal_files_but_own_path_text_can() {
        let (_temp, mut app, source, dest) = fixture();
        app.files_copied(FileList {
            paths: vec![source.join("one")],
            cut: false,
        });
        app.action(Action::Paste);
        let id = read_id(&app);
        app.accept_clipboard_event(ClipboardEvent::Read {
            id,
            result: Ok(Incoming::External {
                stamp: (2, 2),
                files: None,
            }),
        });
        assert!(!app.busy && !dest.join("one").exists());
        app.action(Action::Paste);
        let id = read_id(&app);
        let generation = app.file_exchange.generation;
        app.accept_clipboard_event(ClipboardEvent::Read {
            id,
            result: Ok(Incoming::Local(generation)),
        });
        settle(&mut app);
        assert!(dest.join("one").exists());
    }
    #[test]
    fn consumed_foreign_cut_is_not_retried_against_a_new_file_with_the_same_name() {
        let (_temp, mut app, source, dest) = fixture();
        for step in 0..2 {
            app.action(Action::Paste);
            let id = read_id(&app);
            app.accept_clipboard_event(ClipboardEvent::Read {
                id,
                result: Ok(incoming(&source, (1, 1), true)),
            });
            settle(&mut app);
            if step == 0 {
                fs::write(source.join("one"), "later arrival").unwrap();
            }
        }
        assert_eq!(
            fs::read_to_string(source.join("one")).unwrap(),
            "later arrival"
        );
        assert_eq!(fs::read_to_string(dest.join("one")).unwrap(), "one");
    }
    #[test]
    fn completed_cut_does_not_prune_or_export_a_new_clipboard_generation() {
        let (_temp, mut app, source, _dest) = fixture();
        app.files_copied(FileList {
            paths: vec![source.join("one")],
            cut: true,
        });
        let old = app.file_exchange.generation;
        app.paste_internal(app.tab().cwd.clone());
        app.files_copied(FileList {
            paths: vec![source.join("one")],
            cut: true,
        });
        assert_ne!(old, app.file_exchange.generation);
        settle(&mut app);
        assert_eq!(app.clipboard.as_ref().unwrap().paths, [source.join("one")]);
        assert!(app.file_exchange.queue.iter().all(|r| !matches!(
            r,
            Request::Write {
                expected: Some(_),
                ..
            }
        )));
    }
    #[test]
    fn rapid_text_then_file_copy_publishes_the_latest_file_command_last() {
        let (_temp, mut app, source, _dest) = fixture();
        app.copy_path(&source);
        app.files_copied(FileList {
            paths: vec![source.join("one")],
            cut: false,
        });
        app.queue_text_copy();
        assert!(matches!(
            app.file_exchange.queue.front(),
            Some(Request::Write {
                payload: Payload::Text(_),
                ..
            })
        ));
        assert!(matches!(
            app.file_exchange.queue.back(),
            Some(Request::Write {
                payload: Payload::Files(_),
                ..
            })
        ));
    }
}

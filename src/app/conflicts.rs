use super::*;
use crate::{
    conflicts::{Choice, Conflict, Decision},
    input::{Key, KeyPress},
};
use std::sync::mpsc::SyncSender;

pub struct ConflictDialog {
    pub operation: u64,
    pub request: u64,
    pub info: Conflict,
    pub selected: usize,
    pub remaining: bool,
    reply: SyncSender<Decision>,
}

impl App {
    pub(super) fn accept_conflict(
        &mut self,
        token: u64,
        request: u64,
        info: Conflict,
        reply: SyncSender<Decision>,
    ) {
        if !self
            .current_operation()
            .is_some_and(|r| r.id == token && !r.cancel_requested)
            || request <= self.conflict_sequence
        {
            let _ = reply.try_send(Decision::CANCEL);
            return;
        }
        self.conflict_sequence = request;
        let previous = self.dialog.take();
        if !matches!(previous, Some(Dialog::Conflict(_) | Dialog::Commands(_))) {
            self.conflict_previous = previous;
        }
        let selected = if info.can_keep_both { 2 } else { 1 };
        self.dialog = Some(Dialog::Conflict(ConflictDialog {
            operation: token,
            request,
            info,
            selected,
            remaining: false,
            reply,
        }));
        self.hits.clear();
        self.finish_marquee(true);
    }

    pub fn choose_conflict(&mut self, choice: Choice) {
        let Some(Dialog::Conflict(current)) = &self.dialog else {
            return;
        };
        if !current.info.allows(choice) {
            return;
        }
        let Some(Dialog::Conflict(current)) = self.dialog.take() else {
            unreachable!()
        };
        if self
            .current_operation()
            .is_some_and(|r| r.id == current.operation)
        {
            if choice == Choice::Cancel {
                self.cancel_operation();
            }
            let _ = current.reply.try_send(Decision {
                choice,
                remaining: current.remaining && current.info.remaining > 0,
            });
        }
        self.dialog = self.conflict_previous.take();
        self.hits.clear();
    }

    pub fn toggle_conflict_remaining(&mut self) {
        if let Some(Dialog::Conflict(current)) = &mut self.dialog
            && current.info.remaining > 0
        {
            current.remaining = !current.remaining;
        }
    }

    pub(super) fn finish_conflict(&mut self, token: u64) {
        if matches!(&self.dialog, Some(Dialog::Conflict(c)) if c.operation == token) {
            self.dialog = self.conflict_previous.take();
            self.hits.clear();
        }
    }

    pub(super) fn conflict_key(&mut self, press: KeyPress) {
        match self.settings.action_for(press) {
            Some(Action::CancelOperation) => {
                self.choose_conflict(Choice::Cancel);
                return;
            }
            Some(Action::Quit) => {
                self.action(Action::Quit);
                return;
            }
            _ => {}
        }
        if press.ctrl || press.alt || (press.shift && press.key != Key::Tab) {
            return;
        }
        if press.key == Key::Escape {
            self.choose_conflict(Choice::Cancel);
            return;
        }
        if press.key == Key::Char(' ') {
            self.toggle_conflict_remaining();
            return;
        }
        let Some(Dialog::Conflict(current)) = &mut self.dialog else {
            return;
        };
        if press.key == Key::Enter {
            if current.selected == 4 {
                self.toggle_conflict_remaining();
            } else {
                let choice = Choice::ALL[current.selected];
                self.choose_conflict(choice);
            }
            return;
        }
        let count = if current.info.remaining > 0 { 5 } else { 4 };
        let delta = match press.key {
            Key::Up | Key::Left => -1,
            Key::Tab if press.shift => -1,
            Key::Down | Key::Right | Key::Tab => 1,
            _ => 0,
        };
        if delta != 0 {
            for _ in 0..count {
                current.selected =
                    (current.selected as isize + delta).rem_euclid(count as isize) as usize;
                if current.selected == 4 || current.info.allows(Choice::ALL[current.selected]) {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            app.tick();
            if predicate(app) {
                break;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn stale_operation_and_request_ids_cannot_replace_a_current_conflict_dialog() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let dest = root.path().join("dest");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(source.join("file"), "incoming").unwrap();
        std::fs::write(dest.join("file"), "existing").unwrap();
        let mut app = App::new(
            source,
            Settings {
                preview: false,
                ..Settings::default()
            },
        );
        wait(&mut app, |a| !a.loading());
        app.action(Action::Copy);
        app.navigate(dest.clone(), true);
        wait(&mut app, |a| !a.loading());
        app.action(Action::Paste);
        wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
        let old_id = if let Some(Dialog::Conflict(c)) = &app.dialog {
            c.operation
        } else {
            unreachable!()
        };
        app.choose_conflict(Choice::Skip);
        wait(&mut app, |a| !a.loading());
        app.action(Action::Paste);
        wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
        let (id, request, info) = if let Some(Dialog::Conflict(c)) = &app.dialog {
            (c.operation, c.request, c.info.clone())
        } else {
            unreachable!()
        };
        let (tx, rx) = mpsc::sync_channel(1);
        app.accept_conflict(old_id, request + 100, info.clone(), tx);
        assert_eq!(rx.try_recv().unwrap().choice, Choice::Cancel);
        let (tx, rx) = mpsc::sync_channel(1);
        app.accept_conflict(id, request, info, tx);
        assert_eq!(rx.try_recv().unwrap().choice, Choice::Cancel);
        assert!(
            matches!(&app.dialog,Some(Dialog::Conflict(c))if c.operation==id&&c.request==request)
        );
        app.choose_conflict(Choice::Cancel);
        wait(&mut app, |a| !a.loading());
        assert_eq!(
            std::fs::read_to_string(dest.join("file")).unwrap(),
            "existing"
        );
    }
}

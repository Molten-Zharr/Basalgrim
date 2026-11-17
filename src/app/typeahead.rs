use super::*;
use crate::input::{Key, KeyPress};
use std::time::{Duration, Instant};

pub struct Typeahead {
    owner: PathOwner,
    pub text: String,
    last_input: Instant,
    pub matched: bool,
}

impl App {
    pub fn file_text_active(&self) -> bool {
        self.dialog.is_none()
            && !self.terminal_focused()
            && !self.busy
            && (!self.tab().loading || self.tab().refreshing)
            && self.tab().error.is_none()
            && self.file_drag.is_none()
            && self.marquee.is_none()
            && !self.resizing_columns()
    }

    /// Adapters pass actual composed text, never the physical shortcut key.
    pub fn file_text_key(&self, press: KeyPress) -> bool {
        self.file_text_active()
            && !press.ctrl
            && !press.alt
            && matches!(press.key, Key::Char(c) if !c.is_control())
            && (self.settings.action_for(press).is_none()
                || (press.key == Key::Char(' ') && self.typeahead().is_some()))
    }

    pub fn typeahead(&self) -> Option<&Typeahead> {
        self.typeahead.as_ref().filter(|s| {
            self.file_text_active() && s.owner.pane == self.active && s.owner.valid(self)
        })
    }

    pub fn typeahead_remaining(&self) -> Option<Duration> {
        self.typeahead().map(|s| {
            Duration::from_millis(self.settings.typeahead_timeout_ms)
                .saturating_sub(s.last_input.elapsed())
        })
    }

    pub(super) fn tick_typeahead(&mut self) {
        self.expire_typeahead(Instant::now());
    }

    fn expire_typeahead(&mut self, now: Instant) {
        if self.typeahead.is_some()
            && (self.typeahead().is_none()
                || self.typeahead.as_ref().is_some_and(|s| {
                    now.saturating_duration_since(s.last_input)
                        >= Duration::from_millis(self.settings.typeahead_timeout_ms)
                }))
        {
            self.typeahead = None;
        }
    }

    pub(super) fn typeahead_text(&mut self, text: &str) {
        self.typeahead_text_at(text, Instant::now());
    }

    fn typeahead_text_at(&mut self, text: &str, now: Instant) {
        self.expire_typeahead(now);
        if !self.file_text_active() || text.is_empty() || text.chars().any(char::is_control) {
            return;
        }
        let owner = PathOwner::current(self, self.active);
        let state = self.typeahead.get_or_insert_with(|| Typeahead {
            owner,
            text: String::new(),
            last_input: now,
            matched: false,
        });
        // Bound input storage, not the number of directory entries searched.
        if state.text.len().saturating_add(text.len()) > 4096 {
            return;
        }
        state.text.push_str(text);
        state.last_input = now;
        self.select_typeahead();
    }

    pub(super) fn typeahead_key(&mut self, press: KeyPress) -> bool {
        if press.ctrl || press.alt || self.typeahead().is_none() {
            return false;
        }
        match press.key {
            Key::Backspace => {
                let state = self.typeahead.as_mut().unwrap();
                let boundaries = crate::text_edit::boundaries(&state.text);
                let end = boundaries
                    .get(boundaries.len().saturating_sub(2))
                    .copied()
                    .unwrap_or(0);
                state.text.truncate(end);
                state.last_input = Instant::now();
                if state.text.is_empty() {
                    self.typeahead = None;
                } else {
                    self.select_typeahead();
                }
                true
            }
            Key::Escape => {
                self.typeahead = None;
                true
            }
            _ => false,
        }
    }

    fn select_typeahead(&mut self) {
        let Some(state) = &self.typeahead else { return };
        let query = state.text.to_lowercase();
        let tab = self.tab();
        let mut substring = None;
        let mut prefix = None;
        for (index, &entry) in tab.visible.iter().enumerate() {
            let name = self
                .entry_name(self.active, &tab.entries[entry])
                .to_lowercase();
            if name.starts_with(&query) {
                prefix = Some(index);
                break;
            }
            if substring.is_none() && name.contains(&query) {
                substring = Some(index);
            }
        }
        let found = prefix.or(substring);
        self.typeahead.as_mut().unwrap().matched = found.is_some();
        if let Some(index) = found {
            let tab = self.tab_mut();
            tab.selected = index;
            tab.follow_selection = true;
            tab.reset_range();
            // Match ordinary unmodified keyboard navigation; do not invent marks.
            if tab.explicit_selection {
                tab.marked.clear();
                tab.explicit_selection = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, App) {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "alpha.txt",
            "alphabet.txt",
            "a photo.txt",
            "beta.txt",
            "Фото.png",
            "a🇷🇺.txt",
        ] {
            std::fs::write(root.path().join(name), "fixture").unwrap();
        }
        let mut app = App::new(
            root.path().into(),
            Settings {
                preview: false,
                typeahead_timeout_ms: 2000,
                ..Default::default()
            },
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.loading() {
            app.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        (root, app)
    }
    #[test]
    fn deadline_is_from_last_input_and_expired_input_starts_fresh() {
        let (_root, mut app) = fixture();
        let now = Instant::now();
        app.typeahead_text_at("a", now);
        app.typeahead_text_at("l", now + Duration::from_secs(1));
        assert_eq!(app.typeahead().unwrap().text, "al");
        app.expire_typeahead(now + Duration::from_millis(2999));
        assert!(app.typeahead().is_some());
        app.expire_typeahead(now + Duration::from_secs(3));
        assert!(app.typeahead().is_none());
        assert_eq!(app.tab().entry().unwrap().name, "alpha.txt");
        app.settings.typeahead_timeout_ms = 200;
        app.typeahead_text_at("a", now);
        app.typeahead_text_at("b", now + Duration::from_millis(200));
        assert_eq!(app.typeahead().unwrap().text, "b");
        assert_eq!(app.tab().entry().unwrap().name, "beta.txt");
        app.expire_typeahead(now + Duration::from_millis(399));
        assert!(app.typeahead().is_some());
        app.expire_typeahead(now + Duration::from_millis(400));
        assert!(app.typeahead().is_none());
    }
    #[test]
    fn backspace_only_erases_and_ctrl_backspace_navigates() {
        use crate::input::Input;
        let (root, mut app) = fixture();
        app.handle(Input::Text("a🇷🇺".into()));
        app.handle(Input::Key(KeyPress::plain(Key::Backspace)));
        assert_eq!(app.typeahead().unwrap().text, "a");
        app.handle(Input::Key(KeyPress::plain(Key::Backspace)));
        assert!(app.typeahead().is_none());
        assert_eq!(app.tab().cwd, root.path());
        app.tab_mut().filter = "txt".into();
        app.handle(Input::Text("missing".into()));
        assert!(!app.typeahead().unwrap().matched);
        app.handle(Input::Key(KeyPress::plain(Key::Escape)));
        assert!(app.typeahead().is_none());
        assert_eq!(app.tab().filter, "txt");
        app.handle(Input::Key(KeyPress::plain(Key::Backspace)));
        assert_eq!(app.tab().cwd, root.path());
        app.handle(Input::Text("a".into()));
        app.expire_typeahead(Instant::now() + Duration::from_secs(3));
        app.handle(Input::Key(KeyPress::plain(Key::Backspace)));
        assert_eq!(app.tab().cwd, root.path());
        app.handle(Input::Text("b".into()));
        app.handle(Input::Key(KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::Backspace)
        }));
        assert!(app.typeahead().is_none());
        assert_eq!(app.tab().cwd, root.path().parent().unwrap());
    }
}

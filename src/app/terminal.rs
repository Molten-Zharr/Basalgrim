use super::*;
use crate::{
    input::{Input, Key, KeyPress},
    terminal::Control,
};

impl App {
    pub fn terminal_focused(&self) -> bool {
        self.dialog.is_none() && self.terminal.visible && self.terminal.focused
    }
    pub fn text_input_active(&self) -> bool {
        self.text_active() || self.terminal_focused()
    }
    pub(super) fn toggle_terminal(&mut self) {
        self.finish_marquee(true);
        self.cancel_file_drag();
        if !self.terminal.exists() {
            if self.in_trash() || self.in_archive() {
                self.message("Откройте обычную папку для запуска терминала", true);
                return;
            }
            self.terminal.start(self.tab().cwd.clone());
        } else if self.terminal.visible && self.terminal.focused {
            self.terminal.visible = false;
            self.terminal.focused = false;
        } else {
            self.terminal.visible = true;
            self.terminal.focused = true;
        }
        self.terminal.release_mouse();
        self.terminal.invalidate_input();
    }
    pub(super) fn terminal_control(&mut self, control: Control) {
        if !self.terminal.snapshot.running
            && matches!(
                control,
                Control::Paths | Control::Paste | Control::Interrupt
            )
        {
            return;
        }
        if matches!(control, Control::Hide | Control::Expand | Control::Close) {
            self.terminal.release_mouse();
        }
        self.terminal.focused = true;
        match control {
            Control::Focus => {}
            Control::Hide => {
                self.terminal.visible = false;
                self.terminal.focused = false;
                self.terminal.invalidate_input();
            }
            Control::Expand => {
                self.terminal.expanded = !self.terminal.expanded;
                self.terminal.invalidate_input();
            }
            Control::Paths => {
                if !self.in_trash() && !self.in_archive() {
                    let mut paths = self.tab().targets();
                    if paths.is_empty() {
                        paths.push(self.tab().cwd.clone());
                    }
                    self.terminal.paths(&paths);
                }
            }
            Control::Paste => self.request_terminal_paste(),
            Control::Copy => {
                let text = self.terminal.snapshot.text(self.terminal.selection);
                if !text.is_empty() {
                    self.request_text_copy(text);
                }
            }
            Control::Interrupt => {
                self.terminal.send(b"\x03");
            }
            Control::Restart => {
                if !self.terminal.snapshot.running && !self.in_trash() && !self.in_archive() {
                    self.terminal.start(self.tab().cwd.clone());
                }
            }
            Control::Close => {
                self.terminal.invalidate_input();
                if self.terminal.snapshot.running {
                    self.dialog = Some(Dialog::CloseTerminal);
                } else {
                    self.terminal.close();
                }
            }
            Control::Up => self
                .terminal
                .scroll((self.terminal.snapshot.rows as isize).max(1)),
            Control::Down => self
                .terminal
                .scroll(-(self.terminal.snapshot.rows as isize).max(1)),
            Control::Bottom => self.terminal.bottom(),
        }
    }
    fn request_terminal_paste(&mut self) {
        let id = self.token();
        self.terminal.paste = Some((id, self.terminal.revision));
        if self.native_text_paste() {
            self.queue_text_read(id);
        } else {
            self.text_paste_fallback = Some(id);
        }
    }
    pub(super) fn finish_terminal_paste(
        &mut self,
        id: u64,
        result: Result<String, String>,
    ) -> bool {
        let Some((current, revision)) = self.terminal.paste else {
            return false;
        };
        if current != id {
            return false;
        }
        self.terminal.paste = None;
        self.text_paste_fallback = None;
        if self.terminal_focused() && self.terminal.revision == revision {
            match result {
                Ok(text) => self.terminal.paste_text(&text),
                Err(error) => self.terminal.error = Some(error),
            }
        }
        true
    }
    pub(super) fn terminal_input(&mut self, input: &Input) -> bool {
        if self.dialog.is_some() || !self.terminal.visible {
            self.terminal.release_mouse();
            return false;
        }
        if self.terminal_mouse(input) {
            return true;
        }
        if let Input::Key(key) = input {
            if self.settings.action_for(*key) == Some(Action::Terminal) {
                self.toggle_terminal();
                return true;
            }
            if !self.terminal.focused {
                return false;
            }
            match *key {
                KeyPress {
                    key: Key::PageUp,
                    shift: true,
                    ctrl: false,
                    alt: false,
                } => self.terminal_control(Control::Up),
                KeyPress {
                    key: Key::PageDown,
                    shift: true,
                    ctrl: false,
                    alt: false,
                } => self.terminal_control(Control::Down),
                KeyPress {
                    key: Key::End,
                    shift: true,
                    ctrl: false,
                    alt: false,
                } => self.terminal_control(Control::Bottom),
                KeyPress {
                    key: Key::Insert,
                    shift: true,
                    ctrl: false,
                    alt: false,
                } => self.request_terminal_paste(),
                KeyPress {
                    key: Key::Char(c),
                    ctrl: true,
                    shift: true,
                    alt: false,
                } if c.eq_ignore_ascii_case(&'v') => self.request_terminal_paste(),
                KeyPress {
                    key: Key::Char(c),
                    ctrl: true,
                    shift: true,
                    alt: false,
                } if c.eq_ignore_ascii_case(&'c') => self.terminal_control(Control::Copy),
                _ => self.terminal.key(*key),
            }
            return true;
        }
        match input {
            Input::Text(text) if self.terminal.focused => {
                self.terminal.send(text.as_bytes());
                return true;
            }
            Input::Paste(text) if self.terminal.focused => {
                self.terminal.paste_text(text);
                return true;
            }
            Input::CancelDrag => {
                self.terminal.selecting = false;
                self.terminal.invalidate_input();
            }
            Input::Scroll { x, y, delta } if self.terminal.area.contains(Position::new(*x, *y)) => {
                self.terminal.scroll(-(*delta as isize));
                return true;
            }
            Input::PointerDown { x, y, .. }
            | Input::Click { x, y, .. }
            | Input::SecondaryClick { x, y } => {
                let point = Position::new(*x, *y);
                let target = self
                    .hits
                    .iter()
                    .rev()
                    .find(|(r, _)| r.contains(point))
                    .map(|(_, t)| t.clone());
                if let Some(Target::Terminal { id, control }) = target {
                    if id != self.terminal.id {
                        return true;
                    }
                    if matches!(input, Input::SecondaryClick { .. }) {
                        // Explicit clipboard button avoids an accidental paste on secondary click.
                        self.terminal.focused = true;
                        return true;
                    }
                    if control == Control::Focus {
                        self.terminal.focused = true;
                        self.terminal.invalidate_input();
                        let cell = self.terminal_cell(*x, *y);
                        self.terminal.selection = cell.map(|i| (i, i));
                        self.terminal.selecting = matches!(input, Input::PointerDown { .. });
                    } else {
                        self.terminal_control(control);
                    }
                    return true;
                }
                self.terminal.focused = false;
                self.terminal.selecting = false;
                self.terminal.invalidate_input();
            }
            Input::Drag { x, y } | Input::PointerUp { x, y } if self.terminal.selecting => {
                if let Some(cell) = self.terminal_cell(*x, *y)
                    && let Some((_, end)) = &mut self.terminal.selection
                {
                    *end = cell;
                }
                if matches!(input, Input::PointerUp { .. }) {
                    self.terminal.selecting = false;
                    if self.terminal.selection.is_some_and(|(a, b)| a == b) {
                        self.terminal.selection = None;
                    }
                }
                return true;
            }
            _ => {}
        }
        false
    }
    fn terminal_mouse(&mut self, input: &Input) -> bool {
        use vt100::MouseProtocolMode as Mode;
        let mode = self.terminal.snapshot.mouse_mode;
        if mode == Mode::None || self.selection_extend || self.terminal.snapshot.scrollback > 0 {
            self.terminal.release_mouse();
            return false;
        }
        let area = self.terminal.area;
        let (x, y, code, release, count) = match input {
            Input::PointerDown { x, y, .. } | Input::Click { x, y, .. }
                if area.contains(Position::new(*x, *y)) =>
            {
                self.terminal.focused = true;
                self.terminal.mouse_pressed = true;
                (*x, *y, 0, false, 1)
            }
            Input::SecondaryClick { x, y } if area.contains(Position::new(*x, *y)) => {
                self.terminal.focused = true;
                (*x, *y, 2, false, 1)
            }
            Input::PointerUp { x, y } if self.terminal.mouse_pressed => {
                self.terminal.mouse_pressed = false;
                if mode == Mode::Press {
                    return true;
                }
                (*x, *y, 0, true, 1)
            }
            Input::Drag { x, y } if self.terminal.mouse_pressed => {
                if !matches!(mode, Mode::ButtonMotion | Mode::AnyMotion) {
                    return true;
                }
                (*x, *y, 32, false, 1)
            }
            Input::Pointer(Some(p)) if mode == Mode::AnyMotion && area.contains(*p) => {
                if self.terminal.mouse_position == Some((p.x, p.y)) {
                    return false;
                }
                (
                    p.x,
                    p.y,
                    if self.terminal.mouse_pressed { 32 } else { 35 },
                    false,
                    1,
                )
            }
            Input::Scroll { x, y, delta } if area.contains(Position::new(*x, *y)) => (
                *x,
                *y,
                if *delta < 0 { 64 } else { 65 },
                false,
                delta.unsigned_abs().min(20),
            ),
            Input::CancelDrag => {
                if !self.terminal.mouse_pressed {
                    return false;
                }
                self.terminal.mouse_pressed = false;
                let (x, y) = self.terminal.mouse_position.unwrap_or((area.x, area.y));
                (x, y, 0, true, 1)
            }
            _ => return false,
        };
        if area.is_empty() {
            return false;
        }
        self.terminal.mouse_position = Some((x, y));
        self.pointer = Some(Position::new(x, y));
        let (x, y) = (
            x.clamp(area.x, area.right() - 1) - area.x,
            y.clamp(area.y, area.bottom() - 1) - area.y,
        );
        if let Some(bytes) =
            crate::terminal::mouse_bytes(self.terminal.snapshot.mouse_encoding, code, x, y, release)
        {
            for _ in 0..count {
                self.terminal.send(&bytes);
            }
        }
        if matches!(input, Input::SecondaryClick { .. } | Input::Click { .. })
            && mode != Mode::Press
        {
            if let Some(bytes) = crate::terminal::mouse_bytes(
                self.terminal.snapshot.mouse_encoding,
                code,
                x,
                y,
                true,
            ) {
                self.terminal.send(&bytes);
            }
            self.terminal.mouse_pressed = false;
        }
        true
    }
    fn terminal_cell(&self, x: u16, y: u16) -> Option<usize> {
        let a = self.terminal.area;
        if a.is_empty() {
            return None;
        }
        let x = x.clamp(a.x, a.right() - 1) - a.x;
        let y = y.clamp(a.y, a.bottom() - 1) - a.y;
        let i = usize::from(y) * usize::from(self.terminal.snapshot.cols) + usize::from(x);
        (i < self.terminal.snapshot.cells.len()).then_some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal as Renderer, backend::TestBackend};
    use std::time::{Duration, Instant};
    fn render(app: &mut App, w: u16, h: u16) {
        Renderer::new(TestBackend::new(w, h))
            .unwrap()
            .draw(|f| crate::ui::render(app, f))
            .unwrap();
    }
    fn ready(app: &mut App) {
        let end = Instant::now() + Duration::from_secs(5);
        while app.tab().loading {
            app.tick();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn click(app: &mut App, c: Control) {
        let r = app
            .hits
            .iter()
            .find_map(|(r, t)| {
                matches!(t,Target::Terminal{control,..} if *control==c).then_some(*r)
            })
            .expect("mouse control");
        app.handle(Input::PointerDown {
            x: r.x,
            y: r.y,
            double: false,
            toggle: false,
        });
        app.handle(Input::PointerUp { x: r.x, y: r.y });
    }
    #[test]
    fn shared_terminal_focus_mouse_minimum_modal_and_stale_paste() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), "data").unwrap();
        let mut app = App::new(dir.path().to_path_buf(), Settings::default());
        ready(&mut app);
        app.terminal = crate::terminal::tests::isolated(dir.path());
        for (w, h) in [(170, 53), (34, 14)] {
            render(&mut app, w, h);
            for c in [
                Control::Hide,
                Control::Expand,
                Control::Paths,
                Control::Paste,
                Control::Copy,
                Control::Interrupt,
                Control::Close,
                Control::Up,
                Control::Down,
                Control::Bottom,
            ] {
                assert!(
                    app.hits.iter().any(|(r, t)| !r.is_empty()
                        && matches!(t,Target::Terminal{control,..} if *control==c)),
                    "missing {c:?} at {w}x{h}"
                );
            }
            app.pointer = Some(Position::new(app.terminal.area.x, app.terminal.area.y));
            assert!(app.hovered_area().is_none());
        }
        let selected = app.tab().selected;
        app.handle(Input::Key(KeyPress::ctrl('c')));
        app.handle(Input::Key(KeyPress::ctrl('x')));
        assert!(app.clipboard.is_none());
        assert!(!app.busy);
        assert_eq!(app.tab().selected, selected);
        app.handle(Input::Key(KeyPress::ctrl('q')));
        assert!(!app.quit);
        app.request_terminal_paste();
        let (id, _) = app.terminal.paste.unwrap();
        assert!(app.text_paste_pending());
        app.handle(Input::Text("echo ".into()));
        app.finish_text_paste(id, Ok("STALE".into()));
        assert!(app.terminal.paste.is_none());
        render(&mut app, 34, 14);
        click(&mut app, Control::Close);
        assert!(matches!(app.dialog, Some(Dialog::CloseTerminal)));
        render(&mut app, 34, 14);
        assert!(
            !app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Terminal { .. }))
        );
        app.handle(Input::Key(KeyPress::plain(Key::Escape)));
        render(&mut app, 34, 14);
        click(&mut app, Control::Hide);
        let pid = app.terminal.snapshot.pid;
        assert!(!app.terminal.visible);
        app.handle(Input::Key(KeyPress::plain(Key::F(4))));
        assert!(app.terminal_focused());
        assert_eq!(pid, app.terminal.snapshot.pid);
        render(&mut app, 170, 53);
        let point = app.file_areas[app.active];
        app.handle(Input::Click {
            x: point.x,
            y: point.y,
            double: false,
            toggle: false,
        });
        assert!(!app.terminal_focused());
        app.handle(Input::Key(KeyPress::ctrl('c')));
        assert!(app.clipboard.is_some());
        app.terminal.close();
    }
    #[test]
    fn shell_exit_dismisses_only_its_close_confirmation_and_returns_file_focus() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), "data").unwrap();
        let mut app = App::new(dir.path().to_path_buf(), Settings::default());
        ready(&mut app);
        for close_confirmation in [true, false] {
            app.terminal = crate::terminal::tests::isolated(dir.path());
            app.terminal.send(b"exit\r");
            app.dialog = Some(if close_confirmation {
                Dialog::CloseTerminal
            } else {
                Dialog::Help
            });
            let end = Instant::now() + Duration::from_secs(5);
            while app.terminal.exists() {
                app.tick();
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!app.terminal_focused() && !app.quit);
            assert_eq!(app.tab().cwd, dir.path());
            if close_confirmation {
                assert!(app.dialog.is_none());
            } else {
                assert!(matches!(app.dialog, Some(Dialog::Help)));
                app.dialog = None;
            }
            render(&mut app, 34, 14);
            assert!(
                !app.hits
                    .iter()
                    .any(|(_, target)| matches!(target, Target::Terminal { .. }))
            );
            app.handle(Input::Key(KeyPress::ctrl('c')));
            assert!(app.clipboard.is_some());
        }
    }

    #[test]
    fn terminal_binding_migration_preserves_existing_f4_assignment() {
        let mut s = Settings::default();
        s.bindings.retain(|b| b.action != Action::Terminal);
        s.bindings.push(crate::input::Binding {
            action: Action::Copy,
            key: KeyPress::plain(Key::F(4)),
        });
        s.normalize().unwrap();
        assert_eq!(s.action_for(KeyPress::plain(Key::F(4))), Some(Action::Copy));
        assert!(s.bindings.iter().any(|b| b.action == Action::Terminal));
    }
}

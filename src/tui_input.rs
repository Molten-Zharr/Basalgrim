//! One input adapter for terminal protocols and Linux-console GPM events.
pub mod gpm;

use crate::{
    app::App,
    input::{Input, Key, KeyPress},
};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use std::time::{Duration, Instant};

pub fn key(app: &mut App, key: KeyEvent) {
    if key.kind == KeyEventKind::Release {
        return;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if app.text_input_active()
        && !ctrl
        && !alt
        && let KeyCode::Char(c) = key.code
    {
        app.handle(Input::Text(c.to_string()));
    } else if let Some(code) = convert_key(key.code) {
        app.handle(Input::Key(KeyPress {
            key: code,
            ctrl,
            alt,
            shift: key.modifiers.contains(KeyModifiers::SHIFT) || key.code == KeyCode::BackTab,
        }));
    }
}
pub fn convert_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        // Normalize bindings in App, never the text sent to an embedded shell.
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    })
}

#[derive(Default)]
pub struct Mouse {
    previous_click: Option<((u16, u16), Instant)>,
}
impl Mouse {
    pub fn reset(&mut self, app: &mut App) {
        self.previous_click = None;
        if app.outgoing_drag_id().is_none() {
            app.handle(Input::CancelDrag);
        }
        app.handle(Input::SelectionModifier(false));
        app.handle(Input::DragModifiers { copy: false });
        app.handle(Input::Pointer(None));
    }
    pub fn handle(&mut self, app: &mut App, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);
        app.handle(Input::SelectionModifier(
            mouse.modifiers.contains(KeyModifiers::SHIFT),
        ));
        app.handle(Input::Pointer(Some(Position::new(x, y))));
        app.handle(Input::DragModifiers {
            copy: mouse.modifiers.contains(KeyModifiers::CONTROL),
        });
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let selecting = mouse
                    .modifiers
                    .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
                let double = !selecting
                    && self.previous_click.is_some_and(|(p, t)| {
                        p == (x, y) && t.elapsed() < Duration::from_millis(400)
                    });
                self.previous_click = if double || selecting {
                    None
                } else {
                    Some(((x, y), Instant::now()))
                };
                app.handle(Input::PointerDown {
                    x,
                    y,
                    double,
                    toggle: mouse.modifiers.contains(KeyModifiers::CONTROL),
                });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                app.handle(Input::Drag { x, y });
                if app.selection_area().is_some() || app.dragging_files() {
                    self.previous_click = None;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                app.handle(Input::Drag { x, y });
                if app.selection_area().is_some() || app.dragging_files() {
                    self.previous_click = None;
                }
                app.handle(Input::PointerUp { x, y });
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.previous_click = None;
                app.handle(Input::SecondaryClick { x, y });
            }
            MouseEventKind::ScrollUp => app.handle(Input::Scroll { x, y, delta: -1 }),
            MouseEventKind::ScrollDown => app.handle(Input::Scroll { x, y, delta: 1 }),
            _ => {}
        }
    }
}

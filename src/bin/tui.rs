use anyhow::Result;
use basalgrim::{
    app::App,
    icons::{IconBackend, terminal::TerminalIcons},
    input::{Input, Key, KeyPress},
    options::Options,
    settings::Settings,
    ui,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::stdout,
    time::{Duration, Instant},
};

struct Session;
fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
}
impl Drop for Session {
    fn drop(&mut self) {
        restore();
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("basalgrim: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let Some(options) = Options::parse()? else {
        return Ok(());
    };
    let (settings, settings_path, warning) = Settings::startup();
    let mut app = App::new(options.directory, settings);
    if let Some(warning) = warning {
        app.message(warning, true);
    }
    enable_raw_mode()?;
    let _session = Session;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut icons = TerminalIcons::detect();
    app.icon_backend = icons
        .as_ref()
        .map_or(IconBackend::Symbols, TerminalIcons::backend);
    execute!(
        stdout(),
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut previous_click: Option<((u16, u16), Instant)> = None;
    while !app.quit {
        app.tick();
        app.persist_preferences(settings_path.as_deref());
        terminal.draw(|frame| {
            ui::render(&mut app, frame);
            if let Some(icons) = &mut icons
                && !icons.render(frame, &app.icon_slots)
            {
                app.icon_backend = IconBackend::Symbols;
            }
        })?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                if app.text_active()
                    && !ctrl
                    && !alt
                    && let KeyCode::Char(c) = key.code
                {
                    app.handle(Input::Text(c.to_string()));
                    continue;
                }
                if let Some(code) = convert_key(key.code) {
                    app.handle(Input::Key(KeyPress {
                        key: code,
                        ctrl,
                        alt,
                        shift: key.modifiers.contains(KeyModifiers::SHIFT)
                            || key.code == KeyCode::BackTab,
                    }));
                }
            }
            Event::Paste(text) => app.handle(Input::Text(text)),
            Event::Mouse(mouse) => {
                let (x, y) = (mouse.column, mouse.row);
                app.handle(Input::Pointer(Some(ratatui::layout::Position::new(x, y))));
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let double = previous_click.is_some_and(|(point, time)| {
                            point == (x, y) && time.elapsed() < Duration::from_millis(400)
                        });
                        previous_click = if double {
                            None
                        } else {
                            Some(((x, y), Instant::now()))
                        };
                        app.handle(Input::Click {
                            x,
                            y,
                            double,
                            toggle: mouse.modifiers.contains(KeyModifiers::CONTROL),
                        });
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        previous_click = None;
                        app.handle(Input::SecondaryClick { x, y });
                    }
                    MouseEventKind::ScrollUp => app.handle(Input::Scroll { x, y, delta: -1 }),
                    MouseEventKind::ScrollDown => app.handle(Input::Scroll { x, y, delta: 1 }),
                    _ => {}
                }
            }
            Event::FocusLost => app.handle(Input::Pointer(None)),
            Event::Resize(_, _) => {
                if let Some(icons) = &mut icons
                    && let Ok(size) = crossterm::terminal::window_size()
                    && size.columns > 0
                    && size.rows > 0
                {
                    icons.resize_cells(size.width / size.columns, size.height / size.rows);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn convert_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(c) => Key::Char(c.to_ascii_lowercase()),
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

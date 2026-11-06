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
    io::{Write, stdout},
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
    let mut app = App::startup(options, settings, warning);
    enable_raw_mode()?;
    let _session = Session;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut icons = TerminalIcons::detect();
    let mut preview = icons
        .as_ref()
        .map(|icons| basalgrim::preview::terminal::TerminalPreview::new(icons.graphics()));
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
    let clipboard = basalgrim::clipboard::terminal::TerminalClipboard::new();
    let files = basalgrim::clipboard::native::Runtime::terminal();
    let mut dnd = basalgrim::dnd::terminal::TerminalDnd::new();
    let mut native_source = None;
    app.clipboard_connecting();
    while !app.quit {
        app.pump_clipboard(&files);
        if let Some(id) = app.take_text_paste_fallback() {
            app.finish_text_paste(
                id,
                Err("Вставьте текст сочетанием терминала, например Ctrl+Shift+V".into()),
            );
        }
        app.tick();
        dnd.tick();
        pump_dnd(&mut dnd, &mut app, &mut native_source);
        dnd.flush(&mut stdout().lock())?;
        if !app.native_text_clipboard()
            && let Some(copy) = app.take_text_copy()
        {
            let id = copy.id;
            match clipboard.submit(copy) {
                Ok(()) => {}
                Err(std::sync::mpsc::TrySendError::Full(copy)) => app.retry_text_copy(copy),
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    app.finish_text_copy(id, Err("Исполнитель буфера недоступен".into()))
                }
            }
        }
        while let Ok(result) = clipboard.results.try_recv() {
            let id = result.id;
            if !app.text_copy_current(result.id) {
                continue;
            }
            let result = result.result.and_then(|delivery| {
                if let basalgrim::clipboard::terminal::Delivery::Osc52(sequence) = &delivery {
                    let mut output = stdout().lock();
                    output
                        .write_all(sequence.as_bytes())
                        .and_then(|()| output.flush())
                        .map_err(|e| e.to_string())?;
                }
                Ok(delivery.destination())
            });
            app.finish_text_copy(id, result);
        }
        app.persist_preferences(settings_path.as_deref());
        terminal.draw(|frame| {
            ui::render(&mut app, frame);
            if let Some(icons) = &mut icons
                && !icons.render(frame, &app.icon_slots)
            {
                app.icon_backend = IconBackend::Symbols;
            }
            if let Some(preview) = &mut preview
                && !preview.render(frame, app.preview_slot.as_ref())
            {
                app.icon_backend = IconBackend::Symbols;
            }
        })?;
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        match event::read()? {
            Event::Osc(bytes) => dnd.input(&bytes),
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
                app.handle(Input::TextSelectionModifier(
                    mouse.modifiers.contains(KeyModifiers::SHIFT),
                ));
                app.handle(Input::Pointer(Some(ratatui::layout::Position::new(x, y))));
                app.handle(Input::DragModifiers {
                    copy: mouse.modifiers.contains(KeyModifiers::CONTROL),
                });
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
                            previous_click = None;
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        app.handle(Input::Drag { x, y });
                        if app.selection_area().is_some() || app.dragging_files() {
                            previous_click = None;
                        }
                        app.handle(Input::PointerUp { x, y });
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
            Event::FocusLost => {
                if app.outgoing_drag_id().is_none() {
                    app.handle(Input::CancelDrag);
                }
                app.handle(Input::Pointer(None));
            }
            Event::Resize(_, _) => {
                if let Some(icons) = &mut icons
                    && let Ok(size) = crossterm::terminal::window_size()
                    && size.columns > 0
                    && size.rows > 0
                {
                    icons.resize_cells(size.width / size.columns, size.height / size.rows);
                    if let Some(preview) = &mut preview {
                        preview.resize_cells(size.width / size.columns, size.height / size.rows);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn pump_dnd(
    dnd: &mut basalgrim::dnd::terminal::TerminalDnd,
    app: &mut App,
    source: &mut Option<u64>,
) {
    use basalgrim::dnd::{Event, Request};
    for event in dnd.events() {
        match event {
            Event::Ready(_) => {}
            Event::Hover {
                id,
                x,
                y,
                allowed,
                preferred,
                ..
            } => {
                let point = Some(ratatui::layout::Position::new(x as u16, y as u16));
                let action = app.hover_external_drop(id, point, allowed, preferred);
                dnd.request(Request::Accept { id, action });
            }
            Event::Leave { id } => app.leave_external_drop(id),
            Event::Drop { id } => dnd.request(if app.capture_external_drop(id) {
                Request::Read { id }
            } else {
                Request::Finish { id, action: None }
            }),
            Event::Data { id, result } => {
                if app.receive_external_drop(id, result) {
                    *source = None;
                } else {
                    dnd.request(Request::Finish { id, action: None });
                }
            }
            Event::Sent { id, result } => {
                app.finish_export(id, result);
                if *source == Some(id) {
                    *source = None;
                }
            }
        }
    }
    if let Some(point) = dnd.take_gesture()
        && let Some(offer) = app.export_file_drag_at(point)
    {
        *source = Some(offer.id);
        dnd.request(Request::Start {
            id: offer.id,
            paths: offer.paths,
            copy: offer.copy,
        });
    }
    if let Some(id) = *source
        && app.outgoing_drag_id() != Some(id)
    {
        dnd.request(Request::Cancel { id });
        *source = None;
    }
    for id in app.take_drop_cancellations() {
        dnd.request(Request::Cancel { id });
    }
    if let Some((id, action)) = app.external_drop_result() {
        dnd.request(Request::Finish { id, action });
    }
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

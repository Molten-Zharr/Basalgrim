use anyhow::Result;
use basalgrim::{
    app::App,
    icons::{IconBackend, terminal::TerminalIcons},
    input::Input,
    options::Options,
    settings::Settings,
    tui_input::{self, Mouse, gpm},
    ui,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{IsTerminal, Write, stdin, stdout},
    time::Duration,
};

struct Session(bool);
fn restore(console: bool) {
    let _ = disable_raw_mode();
    if !console {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange
        );
    }
    let _ = execute!(stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}
impl Drop for Session {
    fn drop(&mut self) {
        restore(self.0);
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("basalgrim: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    if let Some(result) = basalgrim::privileged::helper_entry() {
        return result;
    }
    let Some(mut options) = Options::parse()? else {
        return Ok(());
    };
    let Some(desktop) = basalgrim::desktop::Runtime::start(
        false,
        options.desktop,
        options.file_manager,
        options.request.take(),
    )?
    else {
        return Ok(());
    };
    anyhow::ensure!(
        stdin().is_terminal() && stdout().is_terminal(),
        "Для basalgrim-tui нужны терминальные stdin и stdout. Запустите его в терминале или Linux-консоли"
    );
    let console = gpm::Console::detect(&stdin(), &stdout());
    let is_console = console.is_some();
    let mut gpm = console.map(gpm::Runtime::new);
    let (settings, settings_path, warning) = Settings::startup();
    let mut app = App::startup(options, settings, warning);
    app.attach_desktop(desktop);
    enable_raw_mode()?;
    let _session = Session(is_console);
    execute!(stdout(), EnterAlternateScreen)?;
    let mut icons = if is_console {
        None
    } else {
        TerminalIcons::detect()
    };
    let mut preview = icons
        .as_ref()
        .map(|icons| basalgrim::preview::terminal::TerminalPreview::new(icons.graphics()));
    app.icon_backend = icons
        .as_ref()
        .map_or(IconBackend::Symbols, TerminalIcons::backend);
    if !is_console {
        execute!(
            stdout(),
            EnableMouseCapture,
            EnableBracketedPaste,
            EnableFocusChange
        )?;
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(is_console);
        hook(info);
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut mouse = Mouse::default();
    let clipboard = basalgrim::clipboard::terminal::TerminalClipboard::new();
    let files = basalgrim::clipboard::native::Runtime::terminal();
    let mut dnd = if is_console {
        basalgrim::dnd::terminal::TerminalDnd::disabled()
    } else {
        basalgrim::dnd::terminal::TerminalDnd::new()
    };
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
        if let Some(gpm) = &mut gpm {
            let size = terminal.size()?;
            let update = gpm.poll(size.width, size.height);
            if update.reset {
                mouse.reset(&mut app);
            }
            for event in update.events {
                mouse.handle(&mut app, event);
            }
            if let Some(notice) = update.notice {
                app.message(notice, false);
            }
        }
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
            if gpm.as_ref().is_some_and(gpm::Runtime::connected)
                && let Some(point) = app.pointer.filter(|p| frame.area().contains(*p))
            {
                frame.buffer_mut()[point]
                    .modifier
                    .toggle(ratatui::style::Modifier::REVERSED);
            }
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
            Event::Osc(bytes) if !is_console => dnd.input(&bytes),
            Event::Key(key) => tui_input::key(&mut app, key),
            Event::Paste(text) => app.handle(Input::Paste(text)),
            Event::Mouse(event) if !is_console => mouse.handle(&mut app, event),
            Event::FocusLost => mouse.reset(&mut app),
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

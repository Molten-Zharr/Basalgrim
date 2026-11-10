use super::*;
use crate::terminal::Control;
use ratatui::{style::Color, widgets::Clear};

fn button(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    text: &str,
    control: Control,
    palette: Palette,
) {
    let enabled = match control {
        Control::Paths => !app.in_trash() && app.terminal.snapshot.running,
        Control::Paste | Control::Interrupt => app.terminal.snapshot.running,
        Control::Copy => app.terminal.selection.is_some(),
        Control::Restart => !app.in_trash(),
        _ => true,
    };
    frame.render_widget(
        Paragraph::new(text).style(if enabled {
            palette.accent()
        } else {
            palette.muted()
        }),
        area,
    );
    hit(
        app,
        area,
        Target::Terminal {
            id: app.terminal.id,
            control,
        },
    );
}
pub fn render(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    if area.width < 4 || area.height < 6 {
        return;
    }
    let focused = app.terminal_focused();
    frame.render_widget(Clear, area);
    let block = panel(palette, "", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let hide = format!(" {} Скрыть ", app.settings.shortcut(Action::Terminal));
    let expand = if app.terminal.expanded {
        " Панель "
    } else {
        " Экран "
    };
    let close = " × ";
    let width = (hide.width() + expand.width() + close.width()) as u16;
    let start = inner.right().saturating_sub(width).max(inner.x);
    frame.render_widget(
        Paragraph::new(" Терминал").style(palette.accent()),
        Rect::new(inner.x, inner.y, start - inner.x, 1),
    );
    let mut x = start;
    for (label, control) in [
        (hide.as_str(), Control::Hide),
        (expand, Control::Expand),
        (close, Control::Close),
    ] {
        let width = (label.width() as u16).min(inner.right().saturating_sub(x));
        button(
            app,
            frame,
            Rect::new(x, inner.y, width, 1),
            label,
            control,
            palette,
        );
        x += width;
    }
    let mut buttons = vec![
        (" Вставить пути ", Control::Paths),
        (" Вставить ", Control::Paste),
        (" Копировать ", Control::Copy),
    ];
    buttons.push(if app.terminal.snapshot.running {
        (" Ctrl+C ", Control::Interrupt)
    } else {
        (" Запустить ", Control::Restart)
    });
    buttons.extend([
        (" ↑ ", Control::Up),
        (" ↓ ", Control::Down),
        (" Вниз ", Control::Bottom),
    ]);
    let (mut x, mut y) = (inner.x, inner.y + 1);
    for (label, control) in buttons {
        let width = label.width() as u16;
        if x + width > inner.right() && x > inner.x {
            x = inner.x;
            y += 1;
        }
        if y >= inner.bottom().saturating_sub(3) {
            break;
        }
        button(
            app,
            frame,
            Rect::new(x, y, width.min(inner.width), 1),
            label,
            control,
            palette,
        );
        x += width;
    }
    y += 1;
    let path = format!(
        "Старт: {}",
        fs::clean(&app.terminal.directory.to_string_lossy())
    );
    frame.render_widget(
        Paragraph::new(files::shorten_name(&path, usize::from(inner.width))).style(palette.muted()),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    let content = Rect::new(
        inner.x,
        y,
        inner.width.min(512),
        inner.bottom().saturating_sub(y + 1).min(256),
    );
    app.terminal.area = content;
    if content.is_empty() {
        return;
    }
    app.terminal.resize(content.height, content.width);
    hit(
        app,
        content,
        Target::Terminal {
            id: app.terminal.id,
            control: Control::Focus,
        },
    );
    let snapshot = &app.terminal.snapshot;
    let base = palette.text();
    for row in 0..content.height.min(snapshot.rows) {
        for col in 0..content.width.min(snapshot.cols) {
            let index = usize::from(row) * usize::from(snapshot.cols) + usize::from(col);
            let Some(cell) = snapshot.cells.get(index) else {
                continue;
            };
            let target = &mut frame.buffer_mut()[(content.x + col, content.y + row)];
            let mut style = base
                .fg(color(cell.fgcolor(), base.fg.unwrap_or(Color::Reset)))
                .bg(color(cell.bgcolor(), base.bg.unwrap_or(Color::Reset)));
            for (enabled, modifier) in [
                (cell.bold(), Modifier::BOLD),
                (cell.dim(), Modifier::DIM),
                (cell.italic(), Modifier::ITALIC),
                (cell.underline(), Modifier::UNDERLINED),
                (cell.inverse(), Modifier::REVERSED),
            ] {
                if enabled {
                    style = style.add_modifier(modifier);
                }
            }
            if app
                .terminal
                .selection
                .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&index))
            {
                style = style
                    .remove_modifier(Modifier::REVERSED)
                    .patch(palette.selection());
            }
            target.set_style(style).set_symbol(if cell.has_contents() {
                cell.contents()
            } else {
                " "
            });
            // The leading wide glyph owns both cells; Ratatui handles the continuation.
        }
    }
    if focused
        && app.terminal.selection.is_none()
        && let Some((row, col)) = snapshot.cursor
        && row < content.height
        && col < content.width
    {
        frame.set_cursor_position((content.x + col, content.y + row));
    }
    let help = app
        .pointer
        .and_then(|point| app.hits.iter().rev().find(|(rect, _)| rect.contains(point)))
        .and_then(|(_, target)| match target {
            Target::Terminal { control, .. } => match control {
                Control::Paths => Some("Вставить пути выбранных файлов"),
                Control::Paste => Some("Текст из буфера · Ctrl+Shift+V"),
                Control::Copy => Some("Выделите текст ЛКМ · Ctrl+Shift+C"),
                Control::Interrupt => Some("Прервать активную команду: Ctrl+C"),
                Control::Restart => Some("Новая оболочка в текущей папке"),
                Control::Close => Some("Завершить оболочку и команду"),
                Control::Hide => Some("Скрыть, сохранив работающий сеанс"),
                _ => None,
            },
            _ => None,
        });
    let status = if let Some(error) = &app.terminal.error {
        error.clone()
    } else if !snapshot.status.is_empty() {
        snapshot.status.clone()
    } else if let Some(help) = help {
        help.into()
    } else if app.terminal.selection.is_some() {
        "Выделение · Копир. / Ctrl+Shift+C".into()
    } else if snapshot.scrollback > 0 {
        format!(
            "История: {} / {} · Вниз — к вводу",
            snapshot.scrollback, snapshot.history
        )
    } else if focused {
        "Tab: дополнить · Ctrl+C: прервать".into()
    } else {
        "Щёлкните в терминале для ввода".into()
    };
    frame.render_widget(
        Paragraph::new(status).style(palette.muted()),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
}
fn color(color: vt100::Color, default: Color) -> Color {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => {
            // soft_ratatui treats Indexed as grayscale. Resolve xterm's palette
            // here so GUI and TUI display the same ANSI/256-color screen.
            let rgb = match i {
                0..=15 => [
                    [0, 0, 0],
                    [205, 0, 0],
                    [0, 205, 0],
                    [205, 205, 0],
                    [0, 0, 238],
                    [205, 0, 205],
                    [0, 205, 205],
                    [229, 229, 229],
                    [127, 127, 127],
                    [255, 0, 0],
                    [0, 255, 0],
                    [255, 255, 0],
                    [92, 92, 255],
                    [255, 0, 255],
                    [0, 255, 255],
                    [255, 255, 255],
                ][usize::from(i)],
                16..=231 => {
                    let value = i - 16;
                    let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
                    [level(value / 36), level((value / 6) % 6), level(value % 6)]
                }
                _ => [8 + (i - 232) * 10; 3],
            };
            Color::Rgb(rgb[0], rgb[1], rgb[2])
        }
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

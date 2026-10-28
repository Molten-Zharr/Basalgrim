mod commands;
mod dialogs;
mod files;
mod operations;
mod selection;
mod settings;
pub mod theme;

use crate::{
    app::{App, Target},
    fs,
    input::Action,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use theme::Palette;
use unicode_width::UnicodeWidthStr;

pub fn panel(palette: Palette, title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    use crate::preferences::{BorderWeight, Corners};
    Block::default()
        .borders(if palette.borders == BorderWeight::None {
            Borders::NONE
        } else {
            Borders::ALL
        })
        .border_type(match palette.borders {
            BorderWeight::Double => BorderType::Double,
            BorderWeight::Thick => BorderType::Thick,
            _ if palette.corners == Corners::Rounded => BorderType::Rounded,
            _ => BorderType::Plain,
        })
        .border_style(palette.text().fg(palette.color(if focused {
            "active"
        } else {
            "inactive_panel_border"
        })))
        .title(title)
        .title_style(palette.accent())
        .style(palette.text())
}
pub fn hit(app: &mut App, rect: Rect, target: Target) {
    if !rect.is_empty() {
        app.hits.push((rect, target));
    }
}
pub fn render(app: &mut App, frame: &mut Frame) {
    app.icon_slots.clear();
    let palette = Palette::new(&app.settings);
    render_content(app, frame, palette);
    app.validate_marquee();
    // Use the same topmost hit region as clicking. Modal rendering has already
    // replaced background hits, so a covered control can never receive hover.
    if let Some(area) = app.hovered_area() {
        let area = area.intersection(frame.area());
        let buffer = frame.buffer_mut();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &mut buffer[(x, y)];
                cell.bg = palette.hover_background(cell.bg);
            }
        }
    }
    selection::render(app, frame, palette);
    for slot in &mut app.icon_slots {
        if let ratatui::style::Color::Rgb(r, g, b) =
            frame.buffer_mut()[(slot.area.x, slot.area.y)].bg
        {
            slot.background = [r, g, b];
        }
    }
}
fn render_content(app: &mut App, frame: &mut Frame, palette: Palette) {
    app.hits.clear();
    app.preview_area = Rect::default();
    app.pane_areas = [Rect::default(); 2];
    app.file_areas = [Rect::default(); 2];
    if matches!(app.dialog, Some(crate::app::Dialog::Operations)) {
        operations::render(app, frame, palette);
        return;
    }
    if matches!(
        app.dialog,
        Some(crate::app::Dialog::Settings)
            | Some(crate::app::Dialog::CaptureBinding { .. })
            | Some(crate::app::Dialog::Edit {
                kind: crate::app::EditKind::Setting(_),
                ..
            })
    ) {
        settings::render(app, frame, palette);
        dialogs::render(app, frame, palette);
        return;
    }
    let area = frame.area();
    frame.render_widget(Block::default().style(palette.text()), area);
    if area.width < 34 || area.height < 14 {
        frame.render_widget(
            Paragraph::new(format!(
                "Basalgrim\nУвеличьте окно до 34 × 14\n{} — выход",
                app.settings.shortcut(Action::Quit)
            ))
            .style(palette.accent()),
            area,
        );
        if area.width >= 12 && area.height >= 4 {
            let menu = Rect::new(area.x + 1, area.y + 3, 10, 1);
            frame.render_widget(Paragraph::new(" Действия ").style(palette.accent()), menu);
            hit(app, menu, Target::Action(Action::Commands));
        }
        dialogs::render(app, frame, palette);
        return;
    }
    let compact = area.height < 20;
    let commands = toolbar_commands(app, area.width);
    let toolbar_height = toolbar_rows(area.width, &commands) + 2;
    let zones = Layout::vertical([
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Length(toolbar_height),
        Constraint::Min(4),
        Constraint::Length(if compact { 1 } else { 3 }),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  BASALGRIM", palette.accent().add_modifier(Modifier::BOLD)),
            Span::styled("  /  MOLTEN-ZHARR", palette.muted()),
            Span::styled(
                "     ФАЙЛЫ ПОД КОНТРОЛЕМ",
                palette.text().fg(palette.color("separator")),
            ),
        ]))
        .style(palette.text()),
        zones[0],
    );
    toolbar(app, frame, zones[1], &commands, palette);
    let sidebar = zones[2].width >= 95 && !compact;
    let body = Layout::horizontal([
        Constraint::Length(if sidebar { 24 } else { 0 }),
        Constraint::Min(30),
    ])
    .split(zones[2]);
    if sidebar {
        places(app, frame, body[0], palette);
    }
    let show_preview =
        app.settings.preview && body[1].width >= if app.settings.dual { 125 } else { 86 };
    let content = Layout::horizontal([
        Constraint::Min(28),
        Constraint::Length(if show_preview { 36 } else { 0 }),
    ])
    .split(body[1]);
    if app.settings.dual && content[0].width >= 64 {
        let panes = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(content[0]);
        files::render(app, frame, panes[0], 0, palette);
        files::render(app, frame, panes[1], 1, palette);
    } else {
        files::render(app, frame, content[0], app.active, palette);
    }
    if show_preview {
        preview(app, frame, content[1], palette);
    }
    let shortcuts = [
        ("Имя", Action::Rename),
        ("Превью", Action::Preview),
        ("Обновить", Action::Refresh),
        ("Папка", Action::NewFolder),
        ("Корзина", Action::Trash),
        ("Сортировка", Action::Sort),
    ];
    let inner = if compact {
        zones[3]
    } else {
        frame.render_widget(panel(palette, " ДЕЙСТВИЯ ", false), zones[3]);
        panel(palette, "", false).inner(zones[3])
    };
    let mut x = inner.x;
    for (label, action) in shortcuts {
        let key = app.settings.shortcut(action);
        let value = format!(" {key} {label} ");
        let width = value.width() as u16;
        if x + width > inner.right() {
            break;
        }
        let rect = Rect::new(x, inner.y, width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {key} "), palette.accent()),
                Span::styled(format!("{label} "), palette.text()),
            ])),
            rect,
        );
        hit(app, rect, Target::Action(action));
        x += width;
    }
    let prefix = if app.busy {
        " ◌ "
    } else if app.notice_error {
        " ! "
    } else {
        " · "
    };
    if app.busy && app.current_operation().is_some() {
        operations::status(app, frame, zones[4], palette);
    } else {
        frame.render_widget(
            Paragraph::new(format!("{prefix}{}", app.notice)).style(if app.notice_error {
                palette.text().fg(palette.color("help_heading"))
            } else {
                palette.muted()
            }),
            zones[4],
        );
    }
    dialogs::render(app, frame, palette);
}

fn toolbar_commands(app: &App, width: u16) -> Vec<(String, Action)> {
    if width < 80 {
        return vec![
            (" Действия ".into(), Action::Commands),
            (" ← ".into(), Action::Back),
            (" ↑ ".into(), Action::Parent),
            (" ⌂ ".into(), Action::Home),
            (
                format!(" {} ", app.settings.shortcut(Action::Search)),
                Action::Search,
            ),
            (" Вид ".into(), Action::Grid),
            (" Настр. ".into(), Action::Settings),
            (
                format!(" {} ", app.settings.shortcut(Action::Help)),
                Action::Help,
            ),
        ];
    }
    vec![
        (" Действия ".into(), Action::Commands),
        (" ← ".into(), Action::Back),
        (" → ".into(), Action::Forward),
        (" ↑ ".into(), Action::Parent),
        (" ⌂ ".into(), Action::Home),
        (" + Папка ".into(), Action::NewFolder),
        (" Копия ".into(), Action::Copy),
        (" Вырезать ".into(), Action::Cut),
        (" Вставить ".into(), Action::Paste),
        (
            format!(" {} Поиск ", app.settings.shortcut(Action::Search)),
            Action::Search,
        ),
        (
            format!(
                " {} ",
                if app.settings.grid {
                    "☷ Список"
                } else {
                    "▦ Сетка"
                }
            ),
            Action::Grid,
        ),
        (
            format!(" {}Панели ", if app.settings.dual { "● " } else { "" }),
            Action::Dual,
        ),
        (
            format!(" {} Операции ", app.settings.shortcut(Action::Operations)),
            Action::Operations,
        ),
        (
            format!(" {} Настройки ", app.settings.shortcut(Action::Settings)),
            Action::Settings,
        ),
        (
            format!(" {} Помощь ", app.settings.shortcut(Action::Help)),
            Action::Help,
        ),
    ]
}
fn toolbar_rows(width: u16, commands: &[(String, Action)]) -> u16 {
    let limit = width.saturating_sub(2);
    let mut rows = 1;
    let mut x = 0;
    for (label, _) in commands {
        let w = label.width() as u16 + 1;
        if x + w > limit && x > 0 {
            rows += 1;
            x = 0;
        }
        x += w;
    }
    rows
}
fn toolbar(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    commands: &[(String, Action)],
    palette: Palette,
) {
    let block = panel(palette, "", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (mut x, mut y) = (inner.x, inner.y);
    for (label, action) in commands {
        let width = label.width() as u16;
        if x + width > inner.right() && x > inner.x {
            x = inner.x;
            y += 1;
        }
        if y >= inner.bottom() {
            break;
        }
        let rect = Rect::new(x, y, width.min(inner.width), 1);
        frame.render_widget(Paragraph::new(label.clone()).style(palette.accent()), rect);
        hit(app, rect, Target::Action(*action));
        x += width + 1;
    }
}
fn places(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    let block = panel(palette, " МЕСТА ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut y = inner.y + 1;
    frame.render_widget(
        Paragraph::new(" БЫСТРЫЙ ДОСТУП").style(palette.text().fg(palette.color("sidebar_title"))),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 2;
    for i in 0..app.places.len() {
        if y >= inner.bottom() {
            break;
        }
        let place = &app.places[i];
        let selected = app.tab().cwd == place.path;
        let style = if selected {
            palette
                .text()
                .fg(palette.color(if palette.light() {
                    "text"
                } else {
                    "background"
                }))
                .bg(palette.color("selection_accent"))
        } else {
            palette.text()
        };
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(" {}", place.label)).style(style),
            rect,
        );
        hit(app, rect, Target::Place(place.path.clone()));
        y += 2;
    }
    if y + 5 < inner.bottom() {
        y += 1;
        frame.render_widget(
            Paragraph::new(" ────────────────────")
                .style(palette.text().fg(palette.color("separator"))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 2;
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                " {} Скрытые  {}",
                if app.settings.hidden { "●" } else { "○" },
                app.settings.shortcut(Action::Hidden)
            ))
            .style(palette.muted()),
            rect,
        );
        hit(app, rect, Target::Action(Action::Hidden));
    }
    if inner.height >= 23 {
        let label = app
            .clipboard
            .as_ref()
            .map(|c| {
                format!(
                    " Буфер: {} · {}",
                    c.paths.len(),
                    if c.cut {
                        "вырезать"
                    } else {
                        "копия"
                    }
                )
            })
            .unwrap_or_else(|| " Буфер пуст".into());
        frame.render_widget(
            Paragraph::new(format!(
                "{label}\n\n {} — перейти",
                app.settings.shortcut(Action::Path)
            ))
            .style(palette.text().fg(palette.color("separator"))),
            Rect::new(inner.x, inner.bottom() - 4, inner.width, 3),
        );
    }
}
fn preview(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    let block = panel(
        palette,
        format!(
            " ПРЕДПРОСМОТР · {} ",
            app.settings.shortcut(Action::Preview)
        ),
        false,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.preview_area = inner;
    if app.preview_loading {
        frame.render_widget(Paragraph::new(" Чтение…").style(palette.muted()), inner);
        return;
    }
    if let Some(error) = &app.preview_error {
        frame.render_widget(
            Paragraph::new(error.clone())
                .style(palette.muted())
                .wrap(ratatui::widgets::Wrap { trim: false }),
            inner,
        );
        return;
    }
    let Some(preview) = &app.preview else {
        frame.render_widget(
            Paragraph::new(" Выберите файл").style(palette.muted()),
            inner,
        );
        return;
    };
    let header_height = (preview.details.len() as u16 + 4).min(inner.height);
    let parts =
        Layout::vertical([Constraint::Length(header_height), Constraint::Min(0)]).split(inner);
    let mut header = vec![
        Line::styled(
            format!(" {}", preview.title),
            palette.accent().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    header.extend(
        preview
            .details
            .iter()
            .map(|s| Line::styled(format!(" {s}"), palette.muted())),
    );
    header.push(Line::styled(
        " ───────────────────────────────",
        palette.text().fg(palette.color("separator")),
    ));
    frame.render_widget(Paragraph::new(header), parts[0]);
    app.preview_page = usize::from(parts[1].height).max(1);
    app.preview_offset = app
        .preview_offset
        .min(preview.lines.len().saturating_sub(app.preview_page));
    let lines = preview
        .lines
        .iter()
        .skip(app.preview_offset)
        .take(app.preview_page)
        .map(|line| Line::styled(line.clone(), palette.text()))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), parts[1]);
}

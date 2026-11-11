mod batch;
mod commands;
mod conflicts;
mod devices;
mod dialogs;
mod files;
mod folder_size;
mod network;
mod operations;
mod path;
mod places;
mod selection;
mod settings;
mod storage;
mod terminal;
mod text_edit;
pub mod theme;
mod trash;

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
    app.validate_editor();
    app.rename_area = None;
    app.terminal.area = Rect::default();
    app.icon_slots.clear();
    app.preview_slot = None;
    let palette = Palette::new(&app.settings);
    render_content(app, frame, palette);
    app.validate_marquee();
    app.validate_file_drag();
    app.refresh_external_hover();
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
    if let Some(feedback) = app.drag_feedback()
        && let Some(area) = feedback.area
    {
        let area = area.intersection(frame.area());
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &mut frame.buffer_mut()[(x, y)];
                cell.bg = palette.hover_background(cell.bg);
            }
        }
        if app.icon_backend != crate::icons::IconBackend::Gui && area.height >= 3 && area.width >= 3
        {
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(palette.accent()),
                area,
            );
        }
    }
    app.thumbnails.update(
        &mut app.icon_slots,
        app.settings.appearance.thumbnails
            && app.icon_backend.supports_images()
            && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images,
    );
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
    app.drop_regions.clear();
    if matches!(app.dialog, Some(crate::app::Dialog::Conflict(_))) {
        conflicts::render(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(crate::app::Dialog::Operations)) {
        operations::render(app, frame, palette);
        return;
    }
    if matches!(
        app.dialog,
        Some(crate::app::Dialog::HistoryStorage | crate::app::Dialog::CleanupHistory(_))
    ) {
        storage::render(app, frame, palette);
        dialogs::render(app, frame, palette);
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
    if app.terminal.visible && (app.terminal.expanded || area.height < 30) {
        terminal::render(app, frame, area, palette);
        dialogs::render(app, frame, palette);
        return;
    }
    let compact = area.height < 20;
    let commands = toolbar_commands(app, area.width);
    let toolbar_height = toolbar_rows(area.width, &commands) + 2;
    let shortcuts = footer_commands(app, compact);
    let footer_labels = shortcuts
        .iter()
        .map(|(label, action)| {
            let key = app.settings.shortcut(*action);
            (
                if compact {
                    if matches!(action, Action::RestoreTrash | Action::DeleteTrash) {
                        label.to_string()
                    } else {
                        key
                    }
                } else {
                    format!(" {key} {label} ")
                },
                *action,
            )
        })
        .collect::<Vec<_>>();
    let footer_height = if compact {
        1
    } else {
        toolbar_rows(area.width, &footer_labels) + 2
    };
    let zones = Layout::vertical([
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Length(toolbar_height),
        Constraint::Min(4),
        Constraint::Length(footer_height),
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
    let file_area = if app.terminal.visible {
        let split = Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(zones[2]);
        terminal::render(app, frame, split[1], palette);
        split[0]
    } else {
        zones[2]
    };
    let sidebar = file_area.width >= 95 && !compact;
    let body = Layout::horizontal([
        Constraint::Length(if sidebar { 24 } else { 0 }),
        Constraint::Min(30),
    ])
    .split(file_area);
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
        files::render(app, frame, panes[0], 0, true, palette);
        files::render(app, frame, panes[1], 1, true, palette);
    } else {
        files::render(app, frame, content[0], app.active, false, palette);
    }
    if show_preview {
        preview(app, frame, content[1], palette);
    }
    let inner = if compact {
        zones[3]
    } else {
        frame.render_widget(panel(palette, " ДЕЙСТВИЯ ", false), zones[3]);
        panel(palette, "", false).inner(zones[3])
    };
    let (mut x, mut y) = (inner.x, inner.y);
    for ((label, action), (value, _)) in shortcuts.into_iter().zip(footer_labels) {
        let key = app.settings.shortcut(action);
        let width = value.width() as u16;
        if x + width > inner.right() && x > inner.x {
            x = inner.x;
            y += 1;
        }
        if y >= inner.bottom() {
            break;
        }
        let rect = Rect::new(x, y, width.min(inner.width), 1);
        frame.render_widget(
            Paragraph::new(if compact {
                Line::from(Span::styled(value, palette.accent()))
            } else {
                Line::from(vec![
                    Span::styled(format!(" {key} "), palette.accent()),
                    Span::styled(format!("{label} "), palette.text()),
                ])
            }),
            rect,
        );
        hit(app, rect, Target::Action(action));
        x += width + 1;
    }
    let prefix = if app.busy {
        " ◌ "
    } else if app.notice_error {
        " ! "
    } else {
        " · "
    };
    if let Some(feedback) = app.drag_feedback() {
        let text = if feedback.count == 0 {
            format!(
                "{} файлы → {}",
                if feedback.copy {
                    "Копировать"
                } else {
                    "Переместить"
                },
                fs::clean(&feedback.destination.as_ref().unwrap().to_string_lossy())
            )
        } else if let Some(destination) = feedback.destination {
            format!(
                "{} {} → {} · Ctrl: копия · Esc: отмена",
                if feedback.copy {
                    "Копировать"
                } else {
                    "Переместить"
                },
                feedback.count,
                fs::clean(&destination.to_string_lossy())
            )
        } else {
            format!(
                "{} {} · выберите другую папку · Esc: отмена",
                if feedback.copy {
                    "Копировать"
                } else {
                    "Переместить"
                },
                feedback.count
            )
        };
        frame.render_widget(Paragraph::new(text).style(palette.accent()), zones[4]);
    } else if app.clipboard_reads_pending() || app.external_drop_reading() {
        let area = zones[4];
        let width = 8.min(area.width);
        frame.render_widget(
            Paragraph::new(if app.external_drop_reading() {
                "Чтение перетаскивания…"
            } else {
                "Чтение буфера…"
            })
            .style(palette.muted()),
            area,
        );
        let cancel = Rect::new(area.right() - width, area.y, width, area.height);
        frame.render_widget(Paragraph::new(" Стоп ").style(palette.accent()), cancel);
        hit(app, cancel, Target::Action(Action::CancelOperation));
    } else if app.busy && app.current_operation().is_some() {
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
        ];
    }
    let mut commands = vec![
        (" Действия ".into(), Action::Commands),
        (" ← ".into(), Action::Back),
        (" → ".into(), Action::Forward),
        (" ↑ ".into(), Action::Parent),
        (" ⌂ ".into(), Action::Home),
        (" + Папка ".into(), Action::NewFolder),
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
    ];
    if app.in_trash() {
        commands.retain(|(_, action)| !matches!(action, Action::NewFolder));
        commands.insert(5, (" Очистить корзину ".into(), Action::EmptyTrash));
    }
    commands
}
fn toolbar_rows(width: u16, commands: &[(String, Action)]) -> u16 {
    let limit = width.saturating_sub(2);
    let mut rows = 1;
    let mut x = 0;
    for (label, _) in commands {
        let w = label.width() as u16;
        if x + w > limit && x > 0 {
            rows += 1;
            x = 0;
        }
        x += w + 1;
    }
    rows
}
fn footer_commands(app: &App, compact: bool) -> Vec<(&'static str, Action)> {
    use Action::*;
    let mut commands = if app.in_trash() {
        if compact {
            vec![
                ("Восстановить", RestoreTrash),
                ("Удалить", DeleteTrash),
                ("Операции", Operations),
            ]
        } else {
            vec![
                ("Восстановить", RestoreTrash),
                ("Удалить", DeleteTrash),
                ("Превью", Preview),
                ("Обновить", Refresh),
                ("Операции", Operations),
                ("Поиск", Search),
                ("Сортировка", Sort),
            ]
        }
    } else {
        vec![
            ("Переименовать", Rename),
            ("Превью", Preview),
            ("Терминал", Terminal),
            ("Обновить", Refresh),
            ("Операции", Operations),
            ("Папка", NewFolder),
            ("Корзина", Trash),
            ("Сортировка", Sort),
        ]
    };
    commands.insert(0, ("Помощь", Help));
    commands.push(("Настройки", Settings));
    commands
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
    let bookmarks: Vec<_> = app
        .library
        .snapshot
        .bookmarks
        .iter()
        .filter(|item| !app.in_quick_access(&item.path))
        .take(usize::from(inner.height))
        .cloned()
        .collect();
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
        let selected = !app.in_trash() && app.tab().cwd == place.path;
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
        y += if bookmarks.is_empty() { 2 } else { 1 };
    }
    if y < inner.bottom() {
        let rect = Rect::new(inner.x, y, inner.width, 1);
        let images = app.icon_backend.supports_images()
            && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images;
        let style = if app.in_trash() {
            palette.selection()
        } else {
            palette.text()
        };
        frame.render_widget(
            Paragraph::new(if images {
                "    Корзина"
            } else {
                " [] Корзина"
            })
            .style(style),
            rect,
        );
        if images && rect.width >= 3 {
            let rgb = |role| match palette.color(role) {
                ratatui::style::Color::Rgb(r, g, b) => [r, g, b],
                _ => unreachable!("Palette colors are RGB"),
            };
            app.icon_slots.push(crate::icons::IconSlot {
                source: None,
                content: None,
                kind: crate::icons::IconKind::Trash,
                area: Rect::new(rect.x + 1, rect.y, 2, 1),
                foreground: rgb("text"),
                accent: rgb("selection_accent"),
                background: rgb("background"),
            });
        }
        hit(app, rect, Target::Action(Action::TrashView));
        y += 2;
    }
    for section in crate::places::Section::ALL {
        if y >= inner.bottom().saturating_sub(5) {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!("    {}", section.label())).style(palette.accent()),
            rect,
        );
        hit(app, rect, Target::PlacesSection(section));
        y += 1;
    }
    if y < inner.bottom().saturating_sub(5) {
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(if app.devices.working.is_some() {
                "    Накопители…"
            } else {
                "    Накопители"
            })
            .style(palette.accent()),
            rect,
        );
        hit(app, rect, Target::Action(Action::Devices));
        y += 1;
    }
    if y < inner.bottom().saturating_sub(5) {
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(Paragraph::new("    Сеть").style(palette.accent()), rect);
        hit(app, rect, Target::Action(Action::Network));
        y += 1;
    }
    let devices: Vec<_> = app
        .devices
        .snapshot
        .devices()
        .into_iter()
        .take(usize::from(inner.height))
        .collect();
    for device in devices {
        if y >= inner.bottom().saturating_sub(6) {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(files::shorten_name(
                &format!("  {}", device.label),
                inner.width as usize,
            ))
            .style(palette.text()),
            rect,
        );
        hit(
            app,
            rect,
            Target::DeviceOpen {
                revision: app.devices.revision,
                key: device.key,
            },
        );
        y += 1;
    }
    if y + 2 < inner.bottom().saturating_sub(5) && !bookmarks.is_empty() {
        y += 1;
        for item in bookmarks {
            if y >= inner.bottom().saturating_sub(5) {
                break;
            }
            let rect = Rect::new(inner.x, y, inner.width, 1);
            frame.render_widget(
                Paragraph::new(format!("  {}", item.label)).style(palette.text()),
                rect,
            );
            hit(app, rect, Target::SavedPlace(item.path));
            y += 1;
        }
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
    if app.preview_loading && app.preview.is_none() {
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
    let directory = app
        .tab()
        .entry()
        .filter(|e| e.directory && !e.symlink && !app.in_trash());
    let header_height =
        (preview.details.len() as u16 + 4 + if directory.is_some() { 2 } else { 0 })
            .min(inner.height);
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
    if let Some(entry) = directory {
        let label = app
            .folder_sizes
            .cached(&entry.path)
            .map(|r| format!(" Содержимое ~{} · кэш", fs::size(r.bytes)))
            .unwrap_or_else(|| " Размер не подсчитан".into());
        header.push(Line::styled(label, palette.muted()));
        let row = Rect::new(
            parts[0].x,
            parts[0].y + header.len() as u16,
            parts[0].width,
            1,
        );
        header.push(Line::styled(" Размер папки…", palette.accent()));
        if row.bottom() <= parts[0].bottom() {
            app.hits.push((
                row,
                Target::FolderMeasure {
                    owner: crate::app::PathOwner::current(app, app.active),
                    request: app.tab().request,
                    path: entry.path.clone(),
                },
            ));
        }
    }
    header.push(Line::styled(
        " ───────────────────────────────",
        palette.text().fg(palette.color("separator")),
    ));
    frame.render_widget(Paragraph::new(header), parts[0]);
    if let Some(image) = &preview.image
        && app.icon_backend.supports_images()
        && !parts[1].is_empty()
    {
        app.preview_slot = Some(crate::preview::PreviewSlot {
            image: image.clone(),
            area: parts[1],
            background: app.settings.appearance.palette()["background"],
        });
        return;
    }
    app.preview_page = usize::from(parts[1].height).max(1);
    app.preview_offset = app
        .preview_offset
        .min(preview.lines.len().saturating_sub(app.preview_page));
    let lines = preview
        .lines
        .iter()
        .enumerate()
        .skip(app.preview_offset)
        .take(app.preview_page)
        .map(|(index, line)| {
            let Some(tokens) = preview.code.as_ref().and_then(|code| code.lines.get(index)) else {
                return Line::styled(line.as_str(), palette.text());
            };
            let mut spans = vec![Span::styled(&line[..5], palette.muted())];
            spans.extend(tokens.iter().map(|token| {
                use crate::preview::syntax::Kind;
                let style = match token.kind {
                    Kind::Plain => palette.text(),
                    Kind::Comment => palette.muted(),
                    Kind::Keyword => palette
                        .text()
                        .fg(palette.color("selection_accent"))
                        .add_modifier(Modifier::BOLD),
                    Kind::String => palette.text().fg(palette.color("help_heading")),
                    Kind::Constant => palette.text().fg(palette.color("active")),
                    Kind::Name => palette.text().fg(palette.color("folder_icon")),
                };
                Span::styled(&line[token.range.clone()], style)
            }));
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), parts[1]);
}

mod search;

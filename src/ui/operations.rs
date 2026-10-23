use super::*;
use crate::operations::{OperationRecord, Phase};
use ratatui::widgets::{Gauge, Wrap};

fn progress_bar(frame: &mut Frame, area: Rect, fraction: f64, label: String, palette: Palette) {
    frame.render_widget(
        Gauge::default()
            .ratio(fraction.clamp(0.0, 1.0))
            .label(label)
            .style(palette.text())
            .gauge_style(palette.accent()),
        area,
    );
}

fn summary(record: &OperationRecord) -> String {
    let progress = &record.progress;
    let bytes = match progress.bytes_total {
        Some(total) => format!(" · {} / {}", fs::size(progress.bytes_done), fs::size(total)),
        None if progress.bytes_done > 0 => format!(" · {}", fs::size(progress.bytes_done)),
        _ => String::new(),
    };
    format!(
        "Готово {}/{}{bytes}",
        progress.items_done, progress.items_total
    )
}

fn button(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    label: String,
    target: Target,
    palette: Palette,
) {
    if area.is_empty() {
        return;
    }
    frame.render_widget(Paragraph::new(label).style(palette.accent()), area);
    hit(app, area, target);
}

pub fn status(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    let Some(record) = app.current_operation() else {
        return;
    };
    let requested = record.cancel_requested;
    let cancel = if requested {
        " Отмена… ".into()
    } else {
        format!(" {} Стоп ", app.settings.shortcut(Action::CancelOperation))
    };
    let width = (cancel.width() as u16).min(area.width / 2);
    let bar = Rect::new(
        area.x,
        area.y,
        area.width.saturating_sub(width + 1),
        area.height,
    );
    let text = if record.progress.phase == Phase::Preparing {
        "Подготовка…".into()
    } else {
        summary(record)
    };
    progress_bar(frame, bar, record.progress.fraction(), text, palette);
    hit(app, bar, Target::Action(Action::Operations));
    if !requested {
        button(
            app,
            frame,
            Rect::new(area.right() - width, area.y, width, area.height),
            cancel,
            Target::Action(Action::CancelOperation),
            palette,
        );
    } else {
        frame.render_widget(
            Paragraph::new(cancel).style(palette.muted()),
            Rect::new(area.right() - width, area.y, width, area.height),
        );
    }
}

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let block = panel(palette, " Basalgrim / Операции ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.operations_details_area = Rect::default();
    if inner.width < 28 || inner.height < 10 {
        frame.render_widget(
            Paragraph::new("Увеличьте окно. Esc — назад.")
                .style(palette.text())
                .wrap(Wrap { trim: false }),
            inner,
        );
        if inner.height > 0 {
            button(
                app,
                frame,
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                " Esc Назад ".into(),
                Target::Cancel,
                palette,
            );
        }
        return;
    }
    let zones = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length((inner.height / 3).clamp(2, 8)),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(" Последние 20 операций за этот запуск").style(palette.muted()),
        zones[0],
    );
    if app.operations.is_empty() {
        frame.render_widget(
            Paragraph::new("\n Операций пока нет.").style(palette.text()),
            zones[2],
        );
    } else {
        app.operations_selected = app.operations_selected.min(app.operations.len() - 1);
        let capacity = usize::from(zones[1].height / 2).max(1);
        app.operations_capacity = capacity;
        if app.operations_selected < app.operations_offset {
            app.operations_offset = app.operations_selected;
        }
        if app.operations_selected >= app.operations_offset + capacity {
            app.operations_offset = app.operations_selected + 1 - capacity;
        }
        app.operations_offset = app
            .operations_offset
            .min(app.operations.len().saturating_sub(capacity));
        for index in
            app.operations_offset..(app.operations_offset + capacity).min(app.operations.len())
        {
            let record = &app.operations[index];
            let rect = Rect::new(
                zones[1].x,
                zones[1].y + (index - app.operations_offset) as u16 * 2,
                zones[1].width,
                2.min(zones[1].height),
            );
            let style = if index == app.operations_selected {
                palette.selection()
            } else {
                palette.text()
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(format!(" {} · {}", record.title, record.status())),
                    Line::raw(format!(" {}", summary(record))),
                ])
                .style(style),
                rect,
            );
            hit(app, rect, Target::Operation(index));
        }
        let record = &app.operations[app.operations_selected];
        let details = zones[2];
        let gauge = Rect::new(details.x, details.y, details.width, 1.min(details.height));
        progress_bar(
            frame,
            gauge,
            record.progress.fraction(),
            format!(" {} · {} ", record.status(), summary(record)),
            palette,
        );
        let text_area = Rect::new(
            details.x,
            details.y + 1,
            details.width,
            details.height.saturating_sub(1),
        );
        let progress = &record.progress;
        let mut lines = vec![Line::styled(format!(" {}", record.title), palette.accent())];
        if let Some(destination) = &record.destination {
            lines.push(Line::raw(format!(
                " Назначение: {}",
                fs::clean(&destination.to_string_lossy())
            )));
        }
        if let Some(path) = &progress.current {
            lines.push(Line::raw(format!(
                " {}",
                fs::clean(&path.to_string_lossy())
            )));
        }
        lines.push(Line::raw(match progress.entries_total {
            Some(total) => format!(" Обработано элементов: {} / {total}", progress.entries_done),
            None => format!(" Обработано элементов: {}", progress.entries_done),
        }));
        if progress.bytes_total.is_some() || progress.bytes_done > 0 {
            let duration = record.duration().as_secs_f64().max(0.001);
            lines.push(Line::raw(format!(
                " Обработано: {} · в среднем {}/с · {:.1} с",
                fs::size(progress.bytes_done),
                fs::size((progress.bytes_done as f64 / duration) as u64),
                duration
            )));
        }
        if record.cancelled {
            lines.push(Line::raw(" Уже завершённые элементы сохранены."));
            if record.error.is_none() {
                lines.push(Line::raw(" Временные данные очищены."));
            }
        }
        if let Some(error) = &record.error {
            lines.push(Line::styled(
                format!(" {}", fs::clean(error)),
                palette.text().fg(palette.color("error")),
            ));
        }
        if !record.completed_paths.is_empty() {
            lines.push(Line::styled(" Завершённые элементы:", palette.accent()));
            for path in &record.completed_paths {
                lines.push(Line::raw(format!(
                    "  {}",
                    fs::clean(&path.to_string_lossy())
                )));
            }
        }
        let paragraph = Paragraph::new(lines)
            .style(palette.text())
            .wrap(Wrap { trim: false });
        app.operations_detail_max = paragraph
            .line_count(text_area.width)
            .saturating_sub(usize::from(text_area.height));
        app.operations_detail_offset = app
            .operations_detail_offset
            .min(app.operations_detail_max)
            .min(u16::MAX as usize);
        app.operations_details_area = text_area;
        frame.render_widget(
            paragraph.scroll((app.operations_detail_offset as u16, 0)),
            text_area,
        );
    }
    let footer = zones[3];
    frame.render_widget(
        Paragraph::new(" ↑↓ — операции · PgUp/PgDn — детали").style(palette.muted()),
        Rect::new(footer.x, footer.y, footer.width, 1),
    );
    let y = footer.bottom() - 1;
    button(
        app,
        frame,
        Rect::new(footer.x, y, 12.min(footer.width), 1),
        " Esc Назад ".into(),
        Target::Cancel,
        palette,
    );
    if app.busy && footer.width > 15 {
        button(
            app,
            frame,
            Rect::new(footer.x + 14, y, footer.width - 14, 1),
            format!(" {} Стоп ", app.settings.shortcut(Action::CancelOperation)),
            Target::Action(Action::CancelOperation),
            palette,
        );
    }
}

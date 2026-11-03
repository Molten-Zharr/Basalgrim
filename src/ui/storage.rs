use super::*;
use ratatui::widgets::Wrap;

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let block = panel(palette, " Basalgrim / Хранилище undo/redo ", true);
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());
    if inner.width < 28 || inner.height < 10 {
        return;
    }
    let zones = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(2),
    ])
    .split(inner);
    let selection = app.storage_selection();
    let summary = if app.history_storage.loading {
        "Подсчёт сохранённых данных…".into()
    } else if let Some(catalog) = &app.history_storage.catalog {
        format!(
            "Проверенные данные: {}\nЗаписей: {} · защищено: {}",
            fs::size(catalog.bytes),
            catalog.candidates.len(),
            catalog.protected
        )
    } else {
        app.history_storage
            .error
            .as_deref()
            .map(fs::clean)
            .unwrap_or_default()
    };
    frame.render_widget(
        Paragraph::new(summary)
            .style(palette.text())
            .wrap(Wrap { trim: false }),
        zones[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            " К очистке: {} ↔",
            app.settings.history_retention.label()
        ))
        .style(palette.accent()),
        zones[1],
    );
    hit(app, zones[1], Target::StoragePolicy);
    frame.render_widget(
        Paragraph::new(format!(
            "Выбрано: {} записей · {}",
            selection.entries.len(),
            fs::size(selection.bytes)
        ))
        .style(palette.text()),
        zones[2],
    );
    let mut lines = vec![Line::styled(
        "Удаление только после подтверждения.",
        palette.muted(),
    )];
    if let Some(error) = &app.history_storage.error {
        lines.push(Line::raw(fs::clean(error)));
    }
    for candidate in &selection.entries {
        lines.push(Line::raw(format!(
            " #{} {} · {}",
            candidate.operation,
            fs::clean(&candidate.title),
            fs::size(candidate.bytes)
        )));
    }
    if let Some(catalog) = &app.history_storage.catalog {
        for issue in &catalog.issues {
            lines.push(Line::styled(fs::clean(issue), palette.muted()));
        }
    }
    let paragraph = Paragraph::new(lines)
        .style(palette.text())
        .wrap(Wrap { trim: false });
    app.history_storage.maximum = paragraph
        .line_count(zones[3].width)
        .saturating_sub(zones[3].height as usize);
    app.history_storage.offset = app
        .history_storage
        .offset
        .min(app.history_storage.maximum)
        .min(u16::MAX as usize);
    frame.render_widget(
        paragraph.scroll((app.history_storage.offset as u16, 0)),
        zones[3],
    );
    let footer = zones[4];
    let enabled = !selection.entries.is_empty() && !app.busy && !app.history_storage.loading;
    let clear = Rect::new(footer.x, footer.y, 16.min(footer.width), 1);
    frame.render_widget(
        Paragraph::new(" Enter Очистить ").style(if enabled {
            palette.accent()
        } else {
            palette.muted()
        }),
        clear,
    );
    if enabled {
        hit(app, clear, Target::StorageClear);
    }
    let refresh = Rect::new(footer.x + 16, footer.y, footer.width.saturating_sub(16), 1);
    frame.render_widget(
        Paragraph::new(" Обновить ").style(palette.accent()),
        refresh,
    );
    hit(app, refresh, Target::StorageRefresh);
    let back = Rect::new(footer.x, footer.y + 1, footer.width, 1);
    frame.render_widget(Paragraph::new(" Esc Назад ").style(palette.accent()), back);
    hit(app, back, Target::Cancel);
}

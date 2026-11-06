use super::*;
use crate::app::{Dialog, TextCommand};

pub fn field(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    if area.is_empty() {
        return;
    }
    let Some(Dialog::Edit { text, .. }) = &app.dialog else {
        return;
    };
    let start = app.editor.visible_start(text, area.width);
    let selected = app.editor.selection(text);
    let mut spans = Vec::new();
    for pair in crate::text_edit::boundaries(text)
        .windows(2)
        .filter(|p| p[0] >= start)
    {
        spans.push(Span::styled(
            fs::clean(&text[pair[0]..pair[1]]),
            if pair[0] >= selected.start && pair[0] < selected.end {
                palette.selection()
            } else {
                palette.text()
            },
        ));
    }
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Block::default().style(palette.text()), area);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(palette.text().add_modifier(Modifier::UNDERLINED)),
        area,
    );
    let cursor = app.editor.cursor.min(text.len());
    if app.editor_menu.is_none() {
        frame.set_cursor_position((
            area.x + (crate::text_edit::width(&text[start..cursor]) as u16).min(area.width - 1),
            area.y,
        ));
    }
    hit(app, area, Target::TextSelect { area, start });
}

pub fn rename_controls(app: &mut App, frame: &mut Frame, row: Rect, palette: Palette) {
    frame.render_widget(ratatui::widgets::Clear, row);
    frame.render_widget(Block::default().style(palette.text()), row);
    let confirm = Rect::new(row.x, row.y, row.width.min(8), 1);
    frame.render_widget(Paragraph::new(" ✓ Ввод ").style(palette.accent()), confirm);
    hit(app, confirm, Target::Confirm);
    let cancel = Rect::new(
        confirm.right(),
        row.y,
        row.width.saturating_sub(confirm.width).min(11),
        1,
    );
    frame.render_widget(Paragraph::new(" × Отмена ").style(palette.accent()), cancel);
    hit(app, cancel, Target::Cancel);
    if let Some(Dialog::Edit {
        error: Some(error), ..
    }) = &app.dialog
    {
        frame.render_widget(
            Paragraph::new(error.clone()).style(palette.accent()),
            Rect::new(
                cancel.right(),
                row.y,
                row.right().saturating_sub(cancel.right()),
                1,
            ),
        );
    }
}

pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some((anchor, selected)) = app.editor_menu else {
        return;
    };
    let area = frame.area();
    let rect = Rect::new(
        anchor.x.min(area.right().saturating_sub(24)),
        anchor.y.min(area.bottom().saturating_sub(6)),
        area.width.min(24),
        area.height.min(6),
    );
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|s| !s.area.intersection(rect).is_empty())
    {
        app.preview_slot = None;
    }
    frame.render_widget(ratatui::widgets::Clear, rect);
    let block = panel(palette, "", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::TextMenuDismiss);
    hit(app, rect, Target::TextMenuBackground);
    let selection = match &app.dialog {
        Some(Dialog::Edit { text, .. }) => !app.editor.selection(text).is_empty(),
        _ => false,
    };
    for (i, command) in TextCommand::ALL
        .into_iter()
        .enumerate()
        .take(usize::from(inner.height))
    {
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let enabled = selection || matches!(command, TextCommand::Paste | TextCommand::SelectAll);
        frame.render_widget(
            Paragraph::new(format!(" {}", command.label())).style(if !enabled {
                palette.muted()
            } else if i == selected {
                palette.selection()
            } else {
                palette.text()
            }),
            row,
        );
        hit(app, row, Target::TextCommand(command));
    }
}

use super::*;
use crate::app::Dialog;
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    if matches!(app.dialog, Some(Dialog::Commands(_))) {
        super::commands::render(app, frame, palette);
        return;
    }
    if let Some(Dialog::CaptureBinding { index, error }) = &app.dialog {
        super::settings::capture(app, frame, palette, *index, error.clone());
        return;
    }
    if matches!(app.dialog, Some(Dialog::Settings | Dialog::Operations)) {
        return;
    }
    let Some(dialog) = &app.dialog else {
        return;
    };
    let area = frame.area();
    let height = if matches!(dialog, Dialog::Help) {
        28
    } else if matches!(dialog, Dialog::Trash(_)) {
        12
    } else {
        10
    };
    let width = if matches!(dialog, Dialog::Help) {
        78
    } else {
        68
    };
    let rect = Rect::new(
        area.x + (area.width - area.width.min(width)) / 2,
        area.y + (area.height - area.height.min(height)) / 2,
        area.width.min(width),
        area.height.min(height),
    );
    app.icon_slots
        .retain(|slot| slot.area.intersection(rect).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|slot| !slot.area.intersection(rect).is_empty())
    {
        app.preview_slot = None;
    }
    frame.render_widget(Clear, rect);
    let title = match dialog {
        Dialog::Edit { kind, .. } => kind.title(),
        Dialog::Trash(_) => "Отправить в корзину?",
        Dialog::Help => "Управление · basalgrim",
        Dialog::Settings
        | Dialog::CaptureBinding { .. }
        | Dialog::Operations
        | Dialog::Conflict(_)
        | Dialog::Commands(_) => unreachable!(),
    };
    let block = panel(palette, format!(" {title} "), true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut lines = vec![Line::raw("")];
    match dialog {
        Dialog::Settings
        | Dialog::CaptureBinding { .. }
        | Dialog::Operations
        | Dialog::Conflict(_)
        | Dialog::Commands(_) => unreachable!(),
        Dialog::Edit {
            text: value, error, ..
        } => {
            lines.push(Line::styled(
                format!(
                    " {}▏",
                    tail(value, usize::from(inner.width.saturating_sub(3)))
                ),
                palette.selection(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                error
                    .clone()
                    .unwrap_or_else(|| " Ctrl+U — очистить · Backspace — удалить символ".into()),
                palette.muted(),
            ));
        }
        Dialog::Trash(paths) => {
            lines.push(Line::styled(
                format!(" {} элемент(ов) → системная корзина", paths.len()),
                palette.accent(),
            ));
            for path in paths.iter().take(3) {
                lines.push(Line::styled(
                    format!(
                        " {}",
                        fs::clean(&path.file_name().unwrap_or_default().to_string_lossy())
                    ),
                    palette.text(),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                " Восстановление доступно в корзине рабочего стола.",
                palette.muted(),
            ));
        }
        Dialog::Help => {
            for (keys, description) in [
                ("↑ ↓ ← → / Home End / PgUp PgDn", "Навигация по файлам"),
                ("Esc", "Сбросить выделение и фильтр"),
            ] {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {keys:<30}"), palette.accent()),
                    Span::styled(description, palette.text()),
                ]));
            }
            for &action in Action::ALL {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {:<30}", app.settings.shortcuts(action)),
                        palette.accent(),
                    ),
                    Span::styled(action.name(), palette.text()),
                ]));
            }
            lines.push(Line::styled(
                " Колесо над предпросмотром — прокрутить содержимое",
                palette.muted(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                " Перенос между дисками: проверка копии перед удалением.",
                palette.muted(),
            ));
        }
    }
    let content = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    if matches!(app.dialog, Some(Dialog::Help)) {
        app.help_max = paragraph
            .line_count(content.width)
            .saturating_sub(usize::from(content.height));
        app.help_offset = app.help_offset.min(app.help_max);
        frame.render_widget(paragraph.scroll((app.help_offset as u16, 0)), content);
    } else {
        frame.render_widget(paragraph, content);
    }
    // Only modal hit regions remain interactive while a dialog is open.
    app.hits.clear();
    if rect.width >= 4 && rect.height > 0 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
        hit(app, close, Target::Cancel);
    }
    let bottom = Rect::new(
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        inner.width.saturating_sub(2),
        1,
    );
    let confirm = Rect::new(bottom.x, bottom.y, 22.min(bottom.width), 1);
    frame.render_widget(
        Paragraph::new(if matches!(app.dialog, Some(Dialog::Trash(_))) {
            " Enter  В корзину "
        } else if matches!(app.dialog, Some(Dialog::Help)) {
            " Enter  Закрыть "
        } else {
            " Enter  Применить "
        })
        .style(
            palette
                .text()
                .fg(palette.color("background"))
                .bg(palette.color("confirm_background")),
        ),
        confirm,
    );
    hit(app, confirm, Target::Confirm);
    if bottom.width > 25 {
        let cancel = Rect::new(bottom.x + 24, bottom.y, 16.min(bottom.width - 24), 1);
        frame.render_widget(
            Paragraph::new(" Esc  Закрыть ").style(palette.accent()),
            cancel,
        );
        hit(app, cancel, Target::Cancel);
    }
}
fn tail(value: &str, limit: usize) -> String {
    let mut width = 0;
    let mut result = Vec::new();
    for c in value.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > limit {
            break;
        }
        result.push(c);
        width += w;
    }
    result.into_iter().rev().collect()
}

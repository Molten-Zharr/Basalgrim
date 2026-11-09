use super::*;
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let height =
        (10 + app.batch.as_ref().map_or(0, |v| v.count().min(8) as u16) * 2).min(area.height);
    let width = area.width.min(84);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.icon_slots
        .retain(|slot| slot.area.intersection(rect).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|s| !s.area.intersection(rect).is_empty())
    {
        app.preview_slot = None;
    }
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Массовое переименование ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.height < 10 || inner.width < 20 {
        return;
    }
    let advanced = app.batch.as_ref().is_some_and(|v| v.advanced);
    let numbered = app.batch.as_ref().is_some_and(|v| v.numbered);
    frame.render_widget(
        Paragraph::new(if advanced {
            if inner.width >= 60 {
                " Шаблон имени · расширения по умолчанию сохраняются"
            } else {
                " Шаблон имени"
            }
        } else {
            " Общее имя без расширения"
        })
        .style(palette.accent()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let field = Rect::new(inner.x + 1, inner.y + 1, inner.width - 2, 1);
    super::text_edit::field(app, frame, field, palette);
    if !advanced
        && matches!(&app.dialog, Some(crate::app::Dialog::Edit { text, .. }) if text.is_empty())
    {
        frame.render_widget(
            Paragraph::new("Например: Фото").style(palette.muted()),
            field,
        );
    }
    frame.render_widget(
        Paragraph::new(if advanced {
            if inner.width >= 48 {
                " Сохранить расширения: Photo_{n:03}"
            } else {
                " Сохранить: Photo_{n:03}"
            }
        } else {
            " Пусто — оставить прежние имена"
        })
        .style(palette.muted()),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
    let options = Rect::new(
        inner.x,
        inner.y + 3,
        if advanced {
            inner.width
        } else {
            inner.width.min(31)
        },
        1,
    );
    if advanced {
        frame.render_widget(
            Paragraph::new(if inner.width >= 48 {
                " Задать .jpg: Photo_{n:03}{ext:jpg}"
            } else {
                " .jpg: Photo_{n:03}{ext:jpg}"
            })
            .style(palette.muted()),
            options,
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!(
                " [{}] Номера: 001, 002, 003 · F3",
                if numbered { "x" } else { " " }
            ))
            .style(palette.accent()),
            options,
        );
        hit(app, options, Target::BatchNumber);
    }
    let mode = Rect::new(inner.x, inner.y + 4, 18, 1);
    frame.render_widget(
        Paragraph::new(if advanced {
            " Обычное имя F4 "
        } else {
            " Свой шаблон F4 "
        })
        .style(palette.accent()),
        mode,
    );
    hit(app, mode, Target::BatchMode);
    let refresh = Rect::new(
        mode.right(),
        mode.y,
        inner.width.saturating_sub(18).min(13),
        1,
    );
    frame.render_widget(
        Paragraph::new(" Обновить F5 ").style(palette.accent()),
        refresh,
    );
    hit(app, refresh, Target::BatchRefresh);
    let list = Rect::new(inner.x, inner.y + 6, inner.width, inner.height - 8);
    let Some(view) = &mut app.batch else {
        return;
    };
    let diagnostic = view.error.as_ref().map(|error| {
        let (title, details) = error
            .split_once('\n')
            .unwrap_or(("Не удалось проверить имена", error));
        // Keep deliberate line breaks, but sanitize controls within each line.
        // Long titles must remain readable through the scrollable details too.
        let details = if crate::text_edit::width(title) > usize::from(inner.width.saturating_sub(1))
        {
            error.as_str()
        } else {
            details
        };
        let lines = details
            .lines()
            .map(|line| Line::raw(fs::clean(line)))
            .collect::<Vec<_>>();
        (
            fs::clean(title),
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .style(palette.accent()),
        )
    });
    view.page = if diagnostic.is_some() {
        usize::from(list.height)
    } else {
        usize::from(list.height) / 2
    }
    .max(1);
    let count = diagnostic.as_ref().map_or_else(
        || view.preview.as_ref().map_or(0, |p| p.rows.len()),
        |(_, paragraph)| paragraph.line_count(list.width),
    );
    view.scroll_total = count;
    view.offset = view.offset.min(count.saturating_sub(view.page));
    let ready = !view.pending && view.preview.as_ref().is_some_and(|p| p.ready());
    let summary = if view.pending {
        " Проверка имён…".into()
    } else if let Some((title, _)) = &diagnostic {
        format!(" {title}")
    } else if let Some(preview) = &view.preview {
        if !advanced
            && !numbered
            && preview
                .rows
                .iter()
                .any(|row| row.error.as_deref() == Some("Повторяющееся имя в выбранной группе"))
        {
            " Имена совпали: включите номера".into()
        } else if preview.conflicts > 0 {
            format!(" Исправьте ошибки: {}", preview.conflicts)
        } else if preview.changed == 0 {
            " Имена не изменятся".into()
        } else if advanced {
            if inner.width >= 64 {
                format!(
                    " Готово: {} · меняется имя, формат файла не меняется",
                    preview.changed
                )
            } else {
                format!(" Готово: {} · формат прежний", preview.changed)
            }
        } else {
            format!(" Готово к переименованию: {}", preview.changed)
        }
    } else {
        if advanced {
            " Исправьте шаблон"
        } else {
            " Исправьте имя"
        }
        .into()
    };
    frame.render_widget(
        Paragraph::new(summary).style(palette.text()),
        Rect::new(inner.x, inner.y + 5, inner.width, 1),
    );
    if let Some((_, paragraph)) = diagnostic {
        frame.render_widget(paragraph.scroll((view.offset as u16, 0)), list);
    } else if let Some(preview) = &view.preview {
        for (i, row) in preview
            .rows
            .iter()
            .skip(view.offset)
            .take(view.page)
            .enumerate()
        {
            let y = list.y + i as u16 * 2;
            frame.render_widget(
                Paragraph::new(format!(
                    " Было: {}",
                    super::files::shorten_name(
                        &row.original,
                        usize::from(list.width.saturating_sub(7))
                    )
                ))
                .style(palette.muted()),
                Rect::new(list.x, y, list.width, 1),
            );
            let proposed = match &row.error {
                Some(error) => format!(" ! {} · {}", fs::clean(error), row.proposed),
                None => format!(
                    " Будет: {}",
                    super::files::shorten_name(
                        &row.proposed,
                        usize::from(list.width.saturating_sub(8))
                    )
                ),
            };
            frame.render_widget(
                Paragraph::new(proposed).style(if row.error.is_some() {
                    palette.accent()
                } else {
                    palette.text()
                }),
                Rect::new(list.x, y + 1, list.width, 1),
            );
        }
    }
    let offset = view.offset;
    let page = view.page;
    let scroll = Rect::new(inner.x, inner.bottom() - 2, inner.width, 1);
    let scroll_label = if view.pending {
        format!(" Выбрано: {}", view.count())
    } else if view.error.is_some() {
        format!(
            " ↑  ↓  Подсказка {}–{} / {count}",
            offset + 1,
            (offset + page).min(count)
        )
    } else {
        format!(
            " ↑  ↓  {}–{} / {count}",
            if count == 0 { 0 } else { offset + 1 },
            (offset + page).min(count)
        )
    };
    frame.render_widget(Paragraph::new(scroll_label).style(palette.muted()), scroll);
    hit(
        app,
        Rect::new(scroll.x, scroll.y, 3, 1),
        Target::BatchScroll(-(page as isize)),
    );
    hit(
        app,
        Rect::new(scroll.x + 3, scroll.y, 3, 1),
        Target::BatchScroll(page as isize),
    );
    let confirm = Rect::new(inner.x, inner.bottom() - 1, 18, 1);
    frame.render_widget(
        Paragraph::new(" ✓ Переименовать ").style(if ready {
            palette.accent()
        } else {
            palette.muted()
        }),
        confirm,
    );
    hit(
        app,
        confirm,
        if ready {
            Target::Confirm
        } else {
            Target::MenuBackground
        },
    );
    let cancel = Rect::new(confirm.right(), confirm.y, (inner.width - 18).min(11), 1);
    frame.render_widget(Paragraph::new(" × Отмена ").style(palette.accent()), cancel);
    hit(app, cancel, Target::Cancel);
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
    super::text_edit::menu(app, frame, palette);
}

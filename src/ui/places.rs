use super::*;
use crate::{app::PlaceControl, places::Section};
use ratatui::widgets::Clear;

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let current_in_quick_access = app.in_quick_access(&app.tab().cwd);
    let area = frame.area();
    let rect = Rect::new(
        area.x + area.width.saturating_sub(84) / 2,
        area.y + area.height.saturating_sub(24) / 2,
        area.width.min(84),
        area.height.min(24),
    );
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    app.preview_slot = None;
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Места ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.width < 28 || inner.height < 9 {
        return;
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
    let mut x = inner.x;
    for section in Section::ALL {
        let label = format!(" {} ", section.label());
        let r = Rect::new(x, inner.y, label.width() as u16, 1);
        frame.render_widget(
            Paragraph::new(label).style(if section == app.places_view.section {
                palette.selection()
            } else {
                palette.accent()
            }),
            r,
        );
        hit(app, r, Target::PlacesSection(section));
        x = r.right();
    }
    let view = &mut app.places_view;
    view.page = usize::from(inner.height - 7);
    if view.selected < view.offset {
        view.offset = view.selected;
    }
    if view.selected >= view.offset + view.page {
        view.offset = view.selected + 1 - view.page;
    }
    let revision = view.revision;
    let mut hits = Vec::new();
    for (index, item) in view
        .rows
        .iter()
        .enumerate()
        .skip(view.offset)
        .take(view.page)
    {
        let r = Rect::new(
            inner.x,
            inner.y + 1 + (index - view.offset) as u16,
            inner.width,
            1,
        );
        let label = format!(" {}", item.label);
        frame.render_widget(
            Paragraph::new(super::files::shorten_name(&label, usize::from(r.width))).style(
                if index == view.selected {
                    palette.selection()
                } else {
                    palette.text()
                },
            ),
            r,
        );
        hits.push((r, Target::PlacesRow { revision, index }));
    }
    let selected = view.rows.get(view.selected);
    let detail = selected
        .map(|i| fs::clean(&i.path.to_string_lossy()))
        .unwrap_or_else(|| {
            if view.filter.is_empty() {
                if view.section == Section::Bookmarks {
                    "Дополнительных закладок нет".into()
                } else {
                    "Здесь пока пусто".into()
                }
            } else {
                "Нет совпадений".into()
            }
        });
    let detail_row = inner.bottom() - 6;
    frame.render_widget(
        Paragraph::new(super::files::shorten_name(&detail, inner.width as usize))
            .style(palette.muted()),
        Rect::new(inner.x, detail_row, inner.width, 1),
    );
    let status = app
        .library
        .error
        .as_deref()
        .or_else(|| app.library.snapshot.warnings.first().map(String::as_str))
        .unwrap_or(if app.library.pending > 0 {
            "Чтение / сохранение…"
        } else if selected.is_some_and(|i| i.desktop) && view.section == Section::Bookmarks {
            "Убрать — скрыть из закладок"
        } else if view.section == Section::Bookmarks
            && view.rows.is_empty()
            && view.filter.is_empty()
        {
            if current_in_quick_access {
                "Стандартные папки — слева"
            } else {
                "Добавьте нужную папку в закладки"
            }
        } else {
            "Удаление из списка сохраняет файл"
        });
    frame.render_widget(
        Paragraph::new(status).style(palette.muted()),
        Rect::new(inner.x, detail_row + 1, inner.width, 1),
    );
    let page = format!(
        " {} / {}",
        if view.rows.is_empty() {
            0
        } else {
            view.selected + 1
        },
        view.rows.len()
    );
    frame.render_widget(
        Paragraph::new(page).style(palette.muted()),
        Rect::new(
            inner.x + 7,
            detail_row + 2,
            inner.width.saturating_sub(7),
            1,
        ),
    );
    let has_item = selected.is_some();
    let remove = has_item && app.library.writable();
    let rename = has_item
        && app.library.writable()
        && view.section != Section::Recent
        && !selected.is_some_and(|i| i.desktop);
    for (r, target) in hits {
        hit(app, r, target);
    }
    for (x, y, label, control, enabled) in [
        (0, 2, " ↑ ", PlaceControl::Up, has_item),
        (3, 2, " ↓ ", PlaceControl::Down, has_item),
        (0, 3, " Открыть ", PlaceControl::Open, has_item),
        (9, 3, " В папку ", PlaceControl::Reveal, has_item),
        (19, 3, " Убрать ", PlaceControl::Remove, remove),
        (
            0,
            4,
            " Добавить папку ",
            PlaceControl::Add,
            app.places_view.section == Section::Bookmarks && app.can_bookmark(),
        ),
        (16, 4, " Название ", PlaceControl::Rename, rename),
        (0, 5, " Фильтр / ", PlaceControl::Filter, true),
        (
            12,
            5,
            " Обновить ",
            PlaceControl::Refresh,
            app.library.pending == 0,
        ),
    ] {
        let r = Rect::new(
            inner.x + x,
            detail_row + y,
            (label.width() as u16).min(inner.width.saturating_sub(x)),
            1,
        );
        frame.render_widget(
            Paragraph::new(label).style(if enabled {
                palette.accent()
            } else {
                palette.muted()
            }),
            r,
        );
        // Every row owns hover; dispatch rechecks availability and the rendered revision.
        hit(app, r, Target::PlacesControl { revision, control });
    }
}

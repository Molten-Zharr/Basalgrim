use super::*;
use crate::app::{PaletteControl as C, Target};
use ratatui::widgets::{Clear, Wrap};

fn control(app: &mut App, area: Rect, control: C) {
    let view = app.palette_view.as_ref().unwrap();
    hit(
        app,
        area,
        Target::Palette {
            token: view.token,
            revision: view.revision,
            control,
        },
    );
}
fn button(
    app: &mut App,
    frame: &mut Frame,
    p: Palette,
    area: Rect,
    label: &str,
    c: C,
    enabled: bool,
) {
    frame.render_widget(
        Paragraph::new(label).style(if enabled { p.accent() } else { p.muted() }),
        area,
    );
    control(app, area, c);
}
pub fn render(app: &mut App, frame: &mut Frame, p: Palette) {
    app.update_palette();
    if app.palette_view.is_none() {
        return;
    }
    let area = frame.area();
    let width = area.width.min(76);
    let height = area.height.min(21);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = panel(p, " Поиск команды ", true);
    let mut inner = block.inner(rect);
    let borderless = p.borders == crate::preferences::BorderWeight::None;
    if borderless {
        inner.y += 1;
        inner.height = inner.height.saturating_sub(1);
    }
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    app.preview_slot = None;
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if borderless {
        frame.render_widget(
            Paragraph::new(" Поиск команды ").style(p.accent()),
            Rect::new(rect.x, rect.y, rect.width, 1),
        );
    }
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.width < 32 || inner.height < 12 {
        return;
    }
    let view = app.palette_view.as_mut().unwrap();
    let capacity = (inner.height - 7) as usize;
    let old_offset = view.offset;
    if view.selected < view.offset {
        view.offset = view.selected;
    }
    if view.selected >= view.offset + capacity {
        view.offset = view.selected + 1 - capacity;
    }
    view.offset = view.offset.min(view.rows.len().saturating_sub(capacity));
    if view.area != rect || view.capacity != capacity || old_offset != view.offset {
        view.revision += 1;
    }
    view.area = rect;
    view.capacity = capacity;
    let rows = view
        .rows
        .iter()
        .enumerate()
        .skip(view.offset)
        .take(capacity)
        .map(|(i, r)| (i, r.clone()))
        .collect::<Vec<_>>();
    let selected = view.selected;
    let count = view.rows.len();
    let current = view.rows.get(selected).cloned();
    let error = view.error;
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(p.accent()), close);
    hit(app, close, Target::Cancel);
    frame.render_widget(
        Paragraph::new("Поиск: ").style(p.muted()),
        Rect::new(inner.x, inner.y, 7, 1),
    );
    super::text_edit::field(
        app,
        frame,
        Rect::new(inner.x + 7, inner.y, inner.width - 7, 1),
        p,
    );
    frame.render_widget(
        Paragraph::new(format!("Найдено: {count} · ↑↓ / Enter")).style(p.muted()),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    for (line, (index, item)) in rows.into_iter().enumerate() {
        let r = Rect::new(inner.x, inner.y + 2 + line as u16, inner.width, 1);
        let shortcut = files::shorten_name(&item.shortcut, (r.width as usize / 2).min(24));
        let label = files::shorten_name(
            item.action.name(),
            (r.width as usize).saturating_sub(shortcut.width() + 3),
        );
        let gap = (r.width as usize).saturating_sub(label.width() + shortcut.width() + 1);
        let style = if index == selected {
            p.selection()
        } else if item.reason.is_some() {
            p.muted()
        } else {
            p.text()
        };
        frame.render_widget(
            Paragraph::new(format!(" {label}{}{shortcut}", " ".repeat(gap))).style(style),
            r,
        );
        control(app, r, C::Row(index));
    }
    let name = current.as_ref().map_or("", |row| row.action.name());
    frame.render_widget(
        Paragraph::new(name)
            .style(p.text())
            .wrap(Wrap { trim: true }),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 2),
    );
    let reason = error
        .or_else(|| current.as_ref().and_then(|r| r.reason))
        .unwrap_or(if count == 0 {
            "Ничего не найдено. Измените запрос."
        } else {
            "Enter или двойной щелчок — запуск"
        });
    frame.render_widget(
        Paragraph::new(reason)
            .style(p.muted())
            .wrap(Wrap { trim: true }),
        Rect::new(inner.x, inner.bottom() - 3, inner.width, 2),
    );
    let y = inner.bottom() - 1;
    button(
        app,
        frame,
        p,
        Rect::new(inner.x, y, 11, 1),
        " Запустить ",
        C::Run,
        current.is_some_and(|r| r.reason.is_none()),
    );
    button(
        app,
        frame,
        p,
        Rect::new(inner.x + 11, y, 3, 1),
        " ↑ ",
        C::Move(-1),
        true,
    );
    button(
        app,
        frame,
        p,
        Rect::new(inner.x + 14, y, 3, 1),
        " ↓ ",
        C::Move(1),
        true,
    );
    button(
        app,
        frame,
        p,
        Rect::new(inner.x + 17, y, 7, 1),
        " Сброс ",
        C::Clear,
        true,
    );
    let close = Rect::new(inner.right() - 8, y, 8, 1);
    frame.render_widget(Paragraph::new(" Закрыть").style(p.accent()), close);
    hit(app, close, Target::Cancel);
    super::text_edit::menu(app, frame, p);
}

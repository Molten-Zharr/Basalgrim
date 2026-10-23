use super::*;
use crate::app::Dialog;
use ratatui::widgets::Clear;

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let items = app.command_items();
    let area = frame.area();
    let Some(Dialog::Commands(menu)) = &mut app.dialog else {
        return;
    };
    let width = area.width.min(49);
    let height = area.height.min(items.len() as u16 + 4);
    let rect = Rect::new(
        menu.anchor
            .x
            .min(area.right().saturating_sub(width))
            .max(area.x),
        menu.anchor
            .y
            .min(area.bottom().saturating_sub(height))
            .max(area.y),
        width,
        height,
    );
    let block = panel(palette, " ДЕЙСТВИЯ ", true);
    let inner = block.inner(rect);
    menu.capacity = usize::from(inner.height.saturating_sub(2)).max(1);
    menu.offset = menu.offset.min(items.len().saturating_sub(menu.capacity));
    // Resizing must not leave a keyboard-selected command outside the popup.
    if menu.selected < menu.offset {
        menu.offset = menu.selected;
    } else if menu.selected >= menu.offset + menu.capacity {
        menu.offset = menu.selected + 1 - menu.capacity;
    }
    let offset = menu.offset;
    let capacity = menu.capacity;
    let selected = menu.selected;
    app.icon_slots
        .retain(|slot| slot.area.intersection(rect).is_empty());
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    // The overlay owns the whole input surface. Dismissing it never clicks through.
    hit(app, area, Target::Cancel);
    hit(app, rect, Target::MenuBackground);
    if rect.width >= 4 && rect.height > 0 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
        hit(app, close, Target::Cancel);
    }
    let top = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(
        Paragraph::new(if offset > 0 {
            " ↑ Ещё команды "
        } else {
            " Правый клик · Esc — закрыть "
        })
        .style(palette.muted()),
        top,
    );
    if offset > 0 {
        hit(app, top, Target::CommandScroll(-(capacity as isize)));
    }
    for (index, item) in items.iter().enumerate().skip(offset).take(capacity) {
        let row = Rect::new(
            inner.x,
            inner.y + 1 + (index - offset) as u16,
            inner.width,
            1,
        );
        let style = if !item.enabled {
            palette.muted()
        } else if index == selected {
            palette.selection()
        } else {
            palette.text()
        };
        let shortcut = app.settings.shortcut(item.action);
        let label = item.action.name();
        let label_width = usize::from(row.width).saturating_sub(shortcut.width() + 3);
        let label = files::shorten_name(label, label_width);
        let gap = usize::from(row.width).saturating_sub(label.width() + shortcut.width() + 2);
        frame.render_widget(
            Paragraph::new(format!(" {label}{}{shortcut} ", " ".repeat(gap))).style(style),
            row,
        );
        if item.enabled {
            hit(app, row, Target::Command(index));
        }
    }
    let bottom = Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1);
    if offset + capacity < items.len() {
        frame.render_widget(
            Paragraph::new(" ↓ Ещё команды ").style(palette.accent()),
            bottom,
        );
        hit(app, bottom, Target::CommandScroll(capacity as isize));
    } else {
        frame.render_widget(Paragraph::new(" Закрыть ").style(palette.accent()), bottom);
        hit(app, bottom, Target::Cancel);
    }
}

use super::*;
use crate::{
    app::{Dialog, MenuContext},
    preferences::BorderWeight,
};
use ratatui::widgets::Clear;

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let items = app.command_items();
    let area = frame.area();
    let full =
        matches!(&app.dialog, Some(Dialog::Commands(menu)) if menu.context == MenuContext::All);
    let border_rows = if palette.borders == BorderWeight::None {
        0
    } else {
        2
    };
    let header_rows = u16::from(border_rows == 0);
    let scrolling =
        full || items.len() > usize::from(area.height.saturating_sub(border_rows + header_rows));
    let controls = if scrolling { 2 } else { 0 };
    let title = app.command_title();
    let wanted_width = items
        .iter()
        .map(|item| item.action.name().width() + app.settings.shortcut(item.action).width() + 5)
        .max()
        .unwrap_or(0)
        .max(title.width() + 8)
        .max(if full { 30 } else { 20 });
    let width = area.width.min(wanted_width.min(u16::MAX as usize) as u16);
    let height = area.height.min(
        (items.len().min(u16::MAX as usize) as u16)
            .saturating_add(border_rows + header_rows + controls),
    );
    let Some(Dialog::Commands(menu)) = &mut app.dialog else {
        return;
    };
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
    let block = panel(palette, format!(" {title} "), true);
    let inner = block.inner(rect);
    menu.capacity = usize::from(inner.height.saturating_sub(header_rows + controls)).max(1);
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
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if header_rows > 0 {
        frame.render_widget(
            Paragraph::new(format!(" {title} ")).style(palette.accent()),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    // A hidden border still reserves a title/close row above command items.
    if rect.width >= 4 && rect.height > 0 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
        hit(app, close, Target::Cancel);
    }
    let start_y = inner.y + header_rows;
    if inner.height <= header_rows + controls {
        return;
    }
    if scrolling {
        let top = Rect::new(inner.x, start_y, inner.width, 1);
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
    }
    for (index, item) in items.iter().enumerate().skip(offset).take(capacity) {
        let row = Rect::new(
            inner.x,
            start_y + u16::from(scrolling) + (index - offset) as u16,
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
        // Disabled commands consume clicks and own the same one-row hover area.
        // Activation still checks availability in App::choose_command.
        hit(app, row, Target::Command(index));
    }
    if !scrolling {
        return;
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

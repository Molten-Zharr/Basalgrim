use super::*;
use crate::app::{Dialog, PathOwner};
use ratatui::layout::Position;
use ratatui::widgets::Clear;
use std::path::{Component, Path, PathBuf};

struct Crumb {
    label: String,
    path: PathBuf,
}
fn crumbs(cwd: &Path) -> Vec<Crumb> {
    let mut path = PathBuf::new();
    let mut result = Vec::new();
    for component in cwd.components() {
        path.push(component.as_os_str());
        let label = if component == Component::RootDir {
            "/".into()
        } else {
            fs::clean(&component.as_os_str().to_string_lossy())
        };
        result.push(Crumb {
            label,
            path: path.clone(),
        });
    }
    if result.is_empty() {
        result.push(Crumb {
            label: ".".into(),
            path: cwd.into(),
        });
    }
    result
}
fn label_button(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    label: &str,
    target: Target,
    style: ratatui::style::Style,
) {
    frame.render_widget(Paragraph::new(label.to_owned()).style(style), area);
    hit(app, area, target);
}
fn crumb(
    app: &mut App,
    frame: &mut Frame,
    rect: Rect,
    item: &Crumb,
    owner: &PathOwner,
    current: bool,
    palette: Palette,
) -> u16 {
    if rect.is_empty() {
        return 0;
    }
    let label = files::shorten_name(&item.label, usize::from(rect.width.saturating_sub(2)));
    label_button(
        app,
        frame,
        rect,
        &format!(" {label} "),
        if current {
            Target::PathEdit(owner.clone())
        } else {
            Target::PathGo {
                owner: owner.clone(),
                path: item.path.clone(),
            }
        },
        if current {
            palette.accent().add_modifier(Modifier::BOLD)
        } else {
            palette.muted()
        },
    );
    app.drop_region(rect, owner.pane, owner.tab, Some(item.path.clone()), false);
    rect.width
}
fn separator(frame: &mut Frame, row: Rect, x: &mut u16, palette: Palette) {
    frame.render_widget(
        Paragraph::new("›").style(palette.muted()),
        Rect::new(*x, row.y, 1, 1),
    );
    *x += 1;
}
pub fn bar(app: &mut App, frame: &mut Frame, row: Rect, pane: usize, palette: Palette) {
    if row.width < 16 || row.is_empty() {
        return;
    }
    let owner = PathOwner::current(app, pane);
    if pane == app.active
        && matches!(
            app.dialog,
            Some(Dialog::Edit {
                kind: crate::app::EditKind::Path(_),
                ..
            })
        )
    {
        editor(app, frame, row, palette);
        return;
    }
    let copy = Rect::new(row.right() - 6, row.y, 6, 1);
    label_button(
        app,
        frame,
        copy,
        " Коп. ",
        Target::PathCopy(owner.clone()),
        palette.accent(),
    );
    let available = row.width - 6;
    if app.trash_in(pane) {
        label_button(
            app,
            frame,
            Rect::new(row.x, row.y, available.min(9), 1),
            " Корзина ",
            Target::PathEdit(owner),
            palette.accent().add_modifier(Modifier::BOLD),
        );
        return;
    }
    let items = crumbs(&owner.cwd);
    let total =
        items.iter().map(|p| p.label.width() + 2).sum::<usize>() + items.len().saturating_sub(1);
    let mut x = row.x;
    if total <= usize::from(available) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                separator(frame, row, &mut x, palette);
            }
            x += crumb(
                app,
                frame,
                Rect::new(x, row.y, (item.label.width() + 2) as u16, 1),
                item,
                &owner,
                index + 1 == items.len(),
                palette,
            );
        }
    } else if items.len() <= 2 {
        if items.len() == 2 {
            x += crumb(
                app,
                frame,
                Rect::new(x, row.y, 3, 1),
                &items[0],
                &owner,
                false,
                palette,
            );
            separator(frame, row, &mut x, palette);
        }
        let width = (row.x + available).saturating_sub(x);
        crumb(
            app,
            frame,
            Rect::new(x, row.y, width, 1),
            items.last().unwrap(),
            &owner,
            true,
            palette,
        );
    } else {
        x += crumb(
            app,
            frame,
            Rect::new(x, row.y, 3, 1),
            &items[0],
            &owner,
            false,
            palette,
        );
        separator(frame, row, &mut x, palette);
        let rect = Rect::new(x, row.y, 3, 1);
        x += 3;
        label_button(
            app,
            frame,
            rect,
            " … ",
            Target::PathParents {
                owner: owner.clone(),
                anchor: Position::new(rect.x, row.y + 1),
            },
            palette.accent(),
        );
        separator(frame, row, &mut x, palette);
        let budget = usize::from((row.x + available).saturating_sub(x));
        let mut start = items.len() - 1;
        let mut used = (items[start].label.width() + 2).min(budget);
        while start > 1 && used + 1 + items[start - 1].label.width() + 2 <= budget {
            start -= 1;
            used += 1 + items[start].label.width() + 2;
        }
        for (index, item) in items.iter().enumerate().skip(start) {
            if index > start {
                separator(frame, row, &mut x, palette);
            }
            let width = ((item.label.width() + 2)
                .min(usize::from((row.x + available).saturating_sub(x))))
                as u16;
            x += crumb(
                app,
                frame,
                Rect::new(x, row.y, width, 1),
                item,
                &owner,
                index + 1 == items.len(),
                palette,
            );
        }
    }
}

pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let Some(Dialog::Paths(menu)) = &mut app.dialog else {
        return;
    };
    let width = area.width.min(68);
    let height = area.height.min((menu.parents.len().clamp(1, 8) + 4) as u16);
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
    let block = panel(palette, " Папки пути ", true);
    let inner = block.inner(rect);
    let zones = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    menu.capacity = usize::from(zones[0].height).max(1);
    if menu.selected < menu.offset {
        menu.offset = menu.selected;
    }
    if menu.selected >= menu.offset + menu.capacity {
        menu.offset = menu.selected + 1 - menu.capacity;
    }
    menu.offset = menu
        .offset
        .min(menu.parents.len().saturating_sub(menu.capacity));
    let rows: Vec<_> = menu
        .parents
        .iter()
        .enumerate()
        .skip(menu.offset)
        .take(menu.capacity)
        .map(|(index, p)| (index, fs::clean(&p.to_string_lossy())))
        .collect();
    let selected = menu.selected;
    let capacity = menu.capacity as isize;
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
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    for (row, (index, label)) in rows.iter().enumerate() {
        let row = Rect::new(zones[0].x, zones[0].y + row as u16, zones[0].width, 1);
        let label = files::shorten_name(label, usize::from(row.width.saturating_sub(2)));
        label_button(
            app,
            frame,
            row,
            &format!(" {label}"),
            Target::PathParent(*index),
            if *index == selected {
                palette.selection()
            } else {
                palette.text()
            },
        );
    }
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(" Корневая папка").style(palette.muted()),
            zones[0],
        );
    }
    let up = Rect::new(zones[1].x, zones[1].y, 9.min(zones[1].width), 1);
    label_button(
        app,
        frame,
        up,
        " ↑ Выше ",
        Target::PathScroll(-capacity),
        palette.muted(),
    );
    if zones[1].width > 9 {
        let down = Rect::new(up.right(), up.y, 9.min(zones[1].width - 9), 1);
        label_button(
            app,
            frame,
            down,
            " Ниже ↓ ",
            Target::PathScroll(capacity),
            palette.muted(),
        );
    }
    let copy = Rect::new(zones[2].x, zones[2].y, 12.min(zones[2].width), 1);
    label_button(
        app,
        frame,
        copy,
        " Копировать ",
        Target::PathMenuCopy,
        palette.accent(),
    );
    if zones[2].width > 12 {
        let edit = Rect::new(copy.right(), copy.y, 10.min(zones[2].width - 12), 1);
        label_button(
            app,
            frame,
            edit,
            " Изменить ",
            Target::PathMenuEdit,
            palette.accent(),
        );
    }
    if rect.width >= 4 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        label_button(app, frame, close, " × ", Target::Cancel, palette.accent());
    }
}

fn editor(app: &mut App, frame: &mut Frame, row: Rect, palette: Palette) {
    let Some(Dialog::Edit { text, .. }) = &app.dialog else {
        return;
    };
    let field = Rect::new(row.x, row.y, row.width.saturating_sub(17), 1);
    let cursor = app.path_cursor.min(text.len());
    let available = usize::from(field.width.saturating_sub(1));
    let mut start = 0;
    let span = Span::raw(text.as_str());
    let mut offset = 0;
    for grapheme in span.styled_graphemes(ratatui::style::Style::default()) {
        if offset >= cursor || text[offset..cursor].width() <= available {
            break;
        }
        offset += grapheme.symbol.len();
        start = offset;
    }
    let visible = &text[start..];
    frame.render_widget(
        Paragraph::new(visible.to_owned()).style(if app.path_text_selected {
            palette.selection()
        } else {
            palette.text()
        }),
        field,
    );
    if !app.path_text_selected && field.width > 0 {
        frame.set_cursor_position((
            field.x + (text[start..cursor].width() as u16).min(field.width - 1),
            field.y,
        ));
    }
    hit(app, field, Target::PathTextSelect { area: field, start });
    for (offset, width, label, target) in [
        (0, 7, " Коп. ", Target::PathTextCopy),
        (7, 7, " Ввод ", Target::Confirm),
        (14, 3, " × ", Target::Cancel),
    ] {
        let rect = Rect::new(field.right() + offset, row.y, width, 1).intersection(row);
        label_button(app, frame, rect, label, target, palette.accent());
    }
}

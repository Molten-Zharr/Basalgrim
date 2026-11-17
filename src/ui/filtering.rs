use super::*;
use crate::app::{Dialog, EditKind, FilterControl as C, FilterOwner};
use ratatui::widgets::Clear;
fn button(
    app: &mut App,
    frame: &mut Frame,
    r: Rect,
    label: &str,
    owner: &FilterOwner,
    control: C,
    palette: Palette,
) {
    if r.is_empty() {
        return;
    }
    frame.render_widget(Paragraph::new(label.to_owned()).style(palette.accent()), r);
    hit(
        app,
        r,
        Target::Filter {
            owner: owner.clone(),
            menu: None,
            control,
        },
    );
}
pub fn bar(app: &mut App, frame: &mut Frame, mut row: Rect, pane: usize, palette: Palette) {
    if row.width < 20 || row.is_empty() {
        return;
    }
    if app.settings.view_mode() == crate::view::ViewMode::Columns
        && app.pane_areas[pane].height < 11
    {
        for (x, label, delta) in [(row.x, " ← ", -1), (row.x + 3, " → ", 1)] {
            let r = Rect::new(x, row.y, 3, 1);
            frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
            hit(
                app,
                r,
                Target::FileColumns {
                    owner: crate::app::PathOwner::current(app, pane),
                    request: app.file_tab(pane).request,
                    delta,
                },
            );
        }
        row.x += 6;
        row.width -= 6;
    }
    let owner = FilterOwner::current(app, pane);
    let compact = row.width < 76;
    let options = app.file_tab(pane).filter_options.clone();
    let paging = app.file_tab(pane).filter_run.is_some();
    let editing = pane == app.active && app.inline_filter();
    let extension = editing
        && matches!(
            app.dialog,
            Some(Dialog::Edit {
                kind: EditKind::FilterExtension { .. },
                ..
            })
        );
    frame.render_widget(Clear, row);
    frame.render_widget(Block::default().style(palette.text()), row);
    let mut right = row.right();
    right -= 3;
    button(
        app,
        frame,
        Rect::new(right, row.y, 3, 1),
        " × ",
        &owner,
        C::Clear,
        palette,
    );
    let w = if compact { 3 } else { 10 };
    right -= w;
    button(
        app,
        frame,
        Rect::new(right, row.y, w, 1),
        if compact {
            " … "
        } else {
            " Параметры "
        },
        &owner,
        C::OptionsMenu,
        palette,
    );
    if paging {
        right -= 3;
        button(
            app,
            frame,
            Rect::new(right, row.y, 3, 1),
            " › ",
            &owner,
            C::Page(1),
            palette,
        );
        right -= 3;
        button(
            app,
            frame,
            Rect::new(right, row.y, 3, 1),
            " ‹ ",
            &owner,
            C::Page(-1),
            palette,
        );
    }
    let w = if compact { 7 } else { 20 };
    right -= w;
    let depth = options.depth.map_or_else(
        || {
            if compact {
                "∞".into()
            } else {
                "Максимальная".into()
            }
        },
        |n| n.to_string(),
    );
    button(
        app,
        frame,
        Rect::new(right, row.y, w, 1),
        &format!("{} {depth} ↓", if compact { "Гл." } else { "Глубина" }),
        &owner,
        C::DepthMenu,
        palette,
    );
    if !compact {
        let w = 20;
        right -= w;
        let r = Rect::new(right + 3, row.y, w - 5, 1);
        frame.render_widget(
            Paragraph::new(" .*").style(palette.muted()),
            Rect::new(right, row.y, 3, 1),
        );
        frame.render_widget(
            Paragraph::new("*").style(palette.muted()),
            Rect::new(right + w - 2, row.y, 1, 1),
        );
        if extension {
            super::text_edit::field(app, frame, r, palette);
        } else {
            button(
                app,
                frame,
                r,
                &files::shorten_name(
                    if options.extension.is_empty() {
                        "Расширение"
                    } else {
                        &options.extension
                    },
                    r.width as usize,
                ),
                &owner,
                C::Extension,
                palette,
            );
        }
    }
    let prefix = if compact {
        if extension { ".* " } else { "Ф: " }
    } else {
        "Фильтр: "
    };
    frame.render_widget(
        Paragraph::new(prefix).style(palette.muted()),
        Rect::new(row.x, row.y, prefix.width() as u16, 1),
    );
    let x = row.x + prefix.width() as u16;
    let r = Rect::new(x, row.y, right.saturating_sub(x + 1), 1);
    if editing && (!extension || compact) {
        super::text_edit::field(app, frame, r, palette);
    } else {
        let text = if extension {
            &options.extension
        } else {
            &app.file_tab(pane).filter
        };
        let label = if text.is_empty() {
            "Имя файла".into()
        } else {
            files::shorten_name(text, r.width as usize)
        };
        button(app, frame, r, &label, &owner, C::Name, palette);
    }
}
pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let rows = app.filter_menu_items();
    let Some(Dialog::FilterMenu(m)) = &app.dialog else {
        return;
    };
    let token = m.token;
    let owner = m.owner.clone();
    let selected = m.selected;
    let screen = frame.area();
    let w = 42.min(screen.width);
    let h = (rows.len() as u16 + 3).min(screen.height);
    let area = Rect::new(
        m.anchor.x.min(screen.right() - w),
        m.anchor.y.min(screen.bottom() - h),
        w,
        h,
    );
    let title = if m.depth {
        " Вложенность "
    } else {
        " Параметры фильтра "
    };
    app.hits.clear();
    app.icon_slots
        .retain(|s| s.area.intersection(area).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|s| !s.area.intersection(area).is_empty())
    {
        app.preview_slot = None;
    }
    hit(app, screen, Target::DismissMenu);
    hit(app, area, Target::MenuBackground);
    frame.render_widget(Clear, area);
    let block = panel(palette, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(files::shorten_name(
            &app.filter_status(owner.owner.pane),
            inner.width as usize,
        ))
        .style(palette.muted()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    for (i, (label, control, enabled)) in rows
        .into_iter()
        .enumerate()
        .take(usize::from(inner.height.saturating_sub(1)))
    {
        let r = Rect::new(inner.x, inner.y + 1 + i as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(files::shorten_name(&label, inner.width as usize)).style(if !enabled {
                palette.muted()
            } else if selected == i {
                palette.selection()
            } else {
                palette.text()
            }),
            r,
        );
        // Disabled rows own a one-row inert region, never the menu background.
        hit(
            app,
            r,
            Target::Filter {
                owner: owner.clone(),
                menu: Some(token),
                control,
            },
        );
    }
    let close = Rect::new(area.right() - 4, area.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
}

use super::*;
use ratatui::{
    layout::Alignment,
    style::Style,
    widgets::{Cell, Row, Table},
};

fn image_slot(
    entry: &fs::Entry,
    request: u64,
    area: Rect,
    palette: Palette,
) -> crate::icons::IconSlot {
    let kind = crate::icons::IconKind::for_entry(entry);
    let rgb = |token| match palette.color(token) {
        ratatui::style::Color::Rgb(r, g, b) => [r, g, b],
        _ => unreachable!("Palette colors are RGB"),
    };
    crate::icons::IconSlot {
        source: crate::thumbnails::Source::new(entry, request),
        content: None,
        kind,
        area,
        foreground: rgb("text"),
        accent: rgb(kind.color_role()),
        background: rgb("background"),
    }
}

pub fn render(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    pane: usize,
    split: bool,
    palette: Palette,
) {
    app.pane_areas[pane] = area;
    let focused = app.active == pane;
    let toggle = (!split || pane == 1).then(|| {
        if split {
            " × ".to_owned()
        } else {
            format!(
                " {} {} ",
                if app.settings.dual { "×" } else { "+" },
                if area.width < 52 {
                    "2-я панель"
                } else {
                    "Вторая панель"
                }
            )
        }
    });
    let title = format!(
        " {} · {} ",
        if pane == 0 {
            "ФАЙЛЫ"
        } else {
            "ВТОРАЯ ПАНЕЛЬ"
        },
        if focused {
            "активна".into()
        } else {
            app.settings.shortcut(Action::NextPane)
        }
    );
    let room = area
        .width
        .saturating_sub(toggle.as_ref().map_or(0, |s| s.width() as u16) + 3);
    let title = if title.width() > usize::from(room) {
        format!(
            " {} ",
            if pane == 0 {
                "ФАЙЛЫ"
            } else {
                "ПАНЕЛЬ 2"
            }
        )
    } else {
        title
    };
    let block =
        panel(palette, title, focused).border_style(palette.text().fg(palette.color(if focused {
            "active"
        } else {
            "inactive_file_border"
        })));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(label) = toggle {
        let width = (label.width() as u16).min(area.width.saturating_sub(2));
        let rect = Rect::new(area.right() - width - 1, area.y, width, 1);
        frame.render_widget(Paragraph::new(label).style(palette.accent()), rect);
        hit(app, rect, Target::Action(Action::Dual));
    }
    if inner.height < 4 {
        return;
    }
    let compact = inner.height < 9;
    let parts = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Length(if compact { 0 } else { 1 }),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    tabs(app, frame, parts[0], pane, palette);
    super::path::bar(
        app,
        frame,
        Rect::new(parts[1].x, parts[1].bottom() - 1, parts[1].width, 1),
        pane,
        palette,
    );
    let is_trash = app.trash_in(pane);
    let tab = app.file_tab(pane);
    let filter = if tab.filter.is_empty() {
        String::new()
    } else {
        format!("  / {}", tab.filter)
    };
    let archive_label = if tab.archive.is_some() {
        " · Архив: только чтение"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {}  ·  {} элементов{}{}",
            tab.sort.label(),
            tab.visible.len(),
            filter,
            archive_label
        ))
        .style(palette.text().fg(palette.color("separator"))),
        parts[2],
    );
    if is_trash {
        super::trash::controls(app, frame, parts[2], palette);
    }
    let tab = app.file_tab(pane);
    if tab.loading && !tab.refreshing {
        frame.render_widget(
            Paragraph::new("\n Чтение каталога…").style(palette.muted()),
            parts[3],
        );
        return;
    }
    if let Some(error) = &tab.error {
        frame.render_widget(
            Paragraph::new(format!(
                "\n {error}\n\n {} — назад  ·  {} — повторить",
                app.settings.shortcut(Action::Parent),
                app.settings.shortcut(Action::Refresh)
            ))
            .style(palette.muted())
            .wrap(ratatui::widgets::Wrap { trim: false }),
            parts[3],
        );
        return;
    }
    let icon_start = app.icon_slots.len();
    if app.settings.grid {
        app.set_file_area(pane, parts[3]);
        grid(app, frame, parts[3], pane, palette);
    } else {
        app.set_file_area(
            pane,
            Rect::new(
                parts[3].x,
                parts[3].y + 1,
                parts[3].width,
                parts[3].height.saturating_sub(1),
            ),
        );
        list(app, frame, parts[3], pane, palette);
    }
    if app.file_tab(pane).archive.is_some() {
        for slot in &mut app.icon_slots[icon_start..] {
            slot.source = None;
        }
    }
    let owner = app.panes[pane].active;
    app.drop_region(
        app.file_areas[pane],
        pane,
        owner,
        Some(app.panes[pane].tab().cwd.clone()),
        false,
    );
    let entries = app
        .hits
        .iter()
        .filter_map(|(area, target)| match target {
            Target::Entry { pane: p, index } if *p == pane => {
                let entry = &app.file_tab(pane).entries[app.file_tab(pane).visible[*index]];
                Some((*area, entry.directory.then(|| entry.path.clone())))
            }
            Target::EntryBackground if app.file_areas[pane].contains((area.x, area.y).into()) => {
                Some((*area, Some(app.panes[pane].tab().cwd.clone())))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for (area, destination) in entries {
        app.drop_region(area, pane, owner, destination, false);
    }
    let tab = app.file_tab(pane);
    let total = tab.visible.iter().fold(0u64, |sum, &i| {
        sum.saturating_add(if tab.entries[i].directory {
            0
        } else {
            tab.entries[i].size
        })
    });
    let footer = if is_trash {
        super::trash::details(app)
    } else if tab.marked.is_empty() && !tab.explicit_selection {
        format!(
            " {} / {}   ·   {}",
            if tab.visible.is_empty() {
                0
            } else {
                tab.selected + 1
            },
            tab.visible.len(),
            fs::size(total)
        )
    } else {
        format!(
            " Выбрано: {}   ·   {} — отметить",
            tab.marked.len(),
            app.settings.shortcut(Action::Mark)
        )
    };
    frame.render_widget(Paragraph::new(footer).style(palette.muted()), parts[4]);
    if matches!(
        app.dialog,
        Some(crate::app::Dialog::Edit {
            kind: crate::app::EditKind::Rename(_),
            ..
        })
    ) && app
        .edit_owner
        .as_ref()
        .is_some_and(|o| o.pane == pane && o.valid(app))
    {
        super::text_edit::rename_controls(app, frame, parts[4], palette);
    }
}
fn tabs(app: &mut App, frame: &mut Frame, row: Rect, pane: usize, palette: Palette) {
    if row.is_empty() {
        return;
    }
    let labels = app.panes[pane]
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| {
            let name = if app
                .trash
                .as_ref()
                .is_some_and(|v| v.owner.pane == pane && v.owner.tab == i)
            {
                "Корзина".into()
            } else {
                fs::clean(
                    &tab.cwd
                        .file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new("/"))
                        .to_string_lossy(),
                )
            };
            format!(" {}:{} ", i + 1, name)
        })
        .collect::<Vec<_>>();
    let active = app.panes[pane].active;
    let closable = labels.len() > 1;
    let tab_width = |index: usize| {
        labels[index].width().min(22) as u16 + if closable && index == active { 3 } else { 0 }
    };
    let available = row.width.saturating_sub(4);
    let overflow = (0..labels.len())
        .map(|i| usize::from(tab_width(i)) + 1)
        .sum::<usize>()
        > usize::from(available);
    let budget = available
        .saturating_sub(if overflow { 8 } else { 0 })
        .max(1);
    let mut start = active;
    let mut used = tab_width(active).min(budget) + 1;
    while start > 0 && used + tab_width(start - 1) < budget {
        start -= 1;
        used += tab_width(start) + 1;
    }
    let mut x = row.x;
    if overflow {
        if start > 0 {
            let r = Rect::new(x, row.y, 3, 1);
            frame.render_widget(Paragraph::new(" ‹ ").style(palette.accent()), r);
            hit(
                app,
                r,
                Target::Tab {
                    pane,
                    index: start - 1,
                },
            );
        }
        x += 4;
    }
    let end_x = x + budget;
    let mut end = start;
    while end < labels.len() && x < end_x {
        let width = tab_width(end).min(end_x - x);
        let close_width = if closable && active == end {
            3.min(width.saturating_sub(1))
        } else {
            0
        };
        let rect = Rect::new(x, row.y, width - close_width, 1);
        frame.render_widget(
            Paragraph::new(labels[end].clone()).style(if active == end {
                palette.selection()
            } else {
                palette.muted()
            }),
            rect,
        );
        hit(app, rect, Target::Tab { pane, index: end });
        app.drop_region(
            rect,
            pane,
            end,
            Some(app.panes[pane].tabs[end].cwd.clone()),
            true,
        );
        if close_width > 0 {
            let close = Rect::new(rect.right(), row.y, close_width, 1);
            frame.render_widget(
                Paragraph::new(if close_width == 3 { " × " } else { "×" }).style(palette.accent()),
                close,
            );
            hit(
                app,
                close,
                Target::CloseTab {
                    owner: crate::app::PathOwner::current(app, pane),
                    request: app.panes[pane].tab().request,
                },
            );
        }
        x += width + 1;
        end += 1;
    }
    if overflow {
        if end < labels.len() {
            let r = Rect::new(x.min(row.right() - 7), row.y, 3, 1);
            frame.render_widget(Paragraph::new(" › ").style(palette.accent()), r);
            hit(app, r, Target::Tab { pane, index: end });
        }
        x = x.min(row.right() - 7) + 4;
    }
    let plus = Rect::new(
        x.min(row.right().saturating_sub(3)),
        row.y,
        3.min(row.width),
        1,
    );
    frame.render_widget(Paragraph::new(" + ").style(palette.accent()), plus);
    hit(app, plus, Target::NewTab { pane });
}

pub(super) fn shorten_name(name: &str, width: usize) -> String {
    if name.width() <= width {
        return name.into();
    }
    if width == 0 {
        return String::new();
    }
    let available = width - 1; // Reserve one column for the ellipsis.
    let extension_width = name
        .rsplit_once('.')
        .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
        .map(|(_, extension)| extension.width() + 1)
        .filter(|&width| width < available)
        .unwrap_or(0);
    let mut tail_width = (available / 2).max(extension_width);
    let mut head_width = available - tail_width;
    // Use the renderer's graphemes so wide and combined symbols stay intact.
    let span = Span::raw(name);
    let graphemes: Vec<_> = span
        .styled_graphemes(Style::default())
        .map(|grapheme| grapheme.symbol)
        .collect();
    let mut head = String::new();
    for grapheme in &graphemes {
        let width = grapheme.width();
        if width > head_width {
            break;
        }
        head.push_str(grapheme);
        head_width -= width;
    }
    let mut tail_start = graphemes.len();
    for grapheme in graphemes.iter().rev() {
        let width = grapheme.width();
        if width > tail_width {
            break;
        }
        tail_start -= 1;
        tail_width -= width;
    }
    format!("{head}…{}", graphemes[tail_start..].concat())
}

fn grid(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    let columns = (usize::from(area.width) / 18).max(1);
    let rows = (usize::from(area.height) / 5).max(1);
    let count = columns * rows;
    let tab = app.file_tab_mut(pane);
    tab.columns = columns;
    tab.page_size = count;
    let selected_row = tab.selected / columns;
    let mut top_row = tab.offset / columns;
    if tab.follow_selection {
        if selected_row < top_row {
            top_row = selected_row;
        }
        if selected_row >= top_row + rows {
            top_row = selected_row + 1 - rows;
        }
    }
    top_row = top_row.min(tab.visible.len().div_ceil(columns).saturating_sub(rows));
    tab.offset = top_row * columns;
    let start = tab.offset;
    if tab.visible.is_empty() {
        frame.render_widget(
            Paragraph::new("\n Папка пуста").style(palette.muted()),
            area,
        );
        return;
    }
    for index in start..(start + count).min(tab.visible.len()) {
        let local = index - start;
        let col = local % columns;
        let row = local / columns;
        let left = area.x + (col * usize::from(area.width) / columns) as u16;
        let right = area.x + ((col + 1) * usize::from(area.width) / columns) as u16;
        let rect = Rect::new(
            left,
            area.y + (row * 5) as u16,
            right - left,
            5.min(area.height.saturating_sub((row * 5) as u16)),
        );
        let entry = &app.file_tab(pane).entries[app.file_tab(pane).visible[index]];
        let renaming = app.renaming(pane, &entry.path);
        let selected =
            app.file_tab(pane).selected == index && !app.file_tab(pane).explicit_selection;
        let marked = app.file_tab(pane).marked.contains(&entry.path);
        let base = if selected || marked {
            palette.selection()
        } else {
            palette.text()
        };
        frame.render_widget(Block::default().style(base), rect);
        let icon = if entry.directory {
            [" ▄▄▄     ", " ███████ "]
        } else {
            [" ┌────┐ ", " │ ≡≡ │ "]
        };
        let icon_style = base.fg(palette.color(if entry.directory {
            "folder_icon"
        } else {
            "folder_path"
        }));
        let icon_area = Rect::new(
            rect.x,
            rect.y,
            rect.width,
            2.min(rect.height.saturating_sub(1)),
        );
        let slot = if !icon_area.is_empty()
            && app.icon_backend.supports_images()
            && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images
        {
            let width = icon_area.width.min(6);
            Some(image_slot(
                entry,
                app.file_tab(pane).request,
                Rect::new(
                    icon_area.x + (icon_area.width - width) / 2,
                    icon_area.y,
                    width,
                    icon_area.height,
                ),
                palette,
            ))
        } else {
            frame.render_widget(
                Paragraph::new(icon.join("\n"))
                    .alignment(Alignment::Center)
                    .style(icon_style),
                icon_area,
            );
            None
        };
        let mut rename_area = None;
        if rect.height > 0 && rect.width > 2 {
            let prefix = if marked { "● " } else { "" };
            let suffix = if entry.symlink { " ↗" } else { "" };
            let label_width = rect.width - 2;
            let name_width =
                usize::from(label_width).saturating_sub(prefix.width() + suffix.width());
            let row = Rect::new(rect.x + 1, rect.y + 2.min(rect.height - 1), label_width, 1);
            if renaming && let Some(crate::app::Dialog::Edit { text, .. }) = &app.dialog {
                // Use the same centered origin as the ordinary label. Reserve
                // a cell for the caret, and retain the mark/link decorations.
                let width = crate::text_edit::width(text);
                let padding = name_width.saturating_sub(width) / 2;
                let x = row.x + padding as u16;
                frame.render_widget(
                    Paragraph::new(prefix).style(base),
                    Rect::new(x, row.y, prefix.width() as u16, 1),
                );
                let field = Rect::new(
                    x + prefix.width() as u16,
                    row.y,
                    (width + 1).min(name_width.saturating_sub(padding)) as u16,
                    1,
                );
                let suffix_x = (field.x + width.min(name_width) as u16)
                    .min(row.right().saturating_sub(suffix.width() as u16));
                frame.render_widget(
                    Paragraph::new(suffix).style(base),
                    Rect::new(suffix_x, row.y, suffix.width() as u16, 1),
                );
                rename_area = Some(field);
            } else {
                let label = format!(
                    "{prefix}{}{suffix}",
                    shorten_name(app.entry_name(pane, entry), name_width)
                );
                frame.render_widget(
                    Paragraph::new(label)
                        .alignment(Alignment::Center)
                        .style(base),
                    row,
                );
            }
        }
        if rect.height > 3 {
            let detail = if entry.directory {
                "Папка".into()
            } else {
                fs::size(entry.size)
            };
            frame.render_widget(
                Paragraph::new(detail)
                    .alignment(Alignment::Center)
                    .style(base.fg(palette.color("folder_path"))),
                Rect::new(rect.x, rect.y + 3, rect.width, 1),
            );
        }
        if rename_area.is_some() {
            app.rename_area = rename_area;
        }
        app.icon_slots.extend(slot);
        hit(app, rect, Target::Entry { pane, index });
        if rect.height > 1 && rect.width > 2 {
            hit(
                app,
                Rect::new(rect.x, rect.y + 1, 1, rect.height - 1),
                Target::EntryBackground,
            );
            hit(
                app,
                Rect::new(rect.right() - 1, rect.y + 1, 1, rect.height - 1),
                Target::EntryBackground,
            );
        }
        if rect.height == 5 {
            hit(
                app,
                Rect::new(rect.x, rect.bottom() - 1, rect.width, 1),
                Target::EntryBackground,
            );
        }
        let mark_area = Rect::new(rect.right().saturating_sub(3), rect.y, 3.min(rect.width), 1);
        frame.render_widget(
            Paragraph::new(if marked { "[●]" } else { "[ ]" }).style(base),
            mark_area,
        );
        hit(app, mark_area, Target::EntryMark { pane, index });
    }
}
fn list(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    let rows = usize::from(area.height.saturating_sub(1));
    let images = app.icon_backend.supports_images()
        && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images;
    let is_trash = app.trash_in(pane);
    let mut icon_slots = Vec::new();
    let tab = app.file_tab_mut(pane);
    tab.columns = 1;
    tab.page_size = rows.max(1);
    if rows == 0 {
        return;
    }
    if tab.follow_selection {
        if tab.selected < tab.offset {
            tab.offset = tab.selected;
        }
        if tab.selected >= tab.offset + rows {
            tab.offset = tab.selected + 1 - rows;
        }
    }
    tab.offset = tab.offset.min(tab.visible.len().saturating_sub(rows));
    let start = tab.offset;
    let tab = app.file_tab(pane);
    let mut rendered = vec![];
    for index in start..(start + rows).min(tab.visible.len()) {
        let entry = &tab.entries[tab.visible[index]];
        let base = if (tab.selected == index && !tab.explicit_selection)
            || tab.marked.contains(&entry.path)
        {
            palette.selection()
        } else {
            palette.text()
        };
        let mark = if tab.marked.contains(&entry.path) {
            "●"
        } else {
            "□"
        };
        let name = format!(
            " {mark} {}{}{}",
            if images { "   " } else { "" },
            app.entry_name(pane, entry),
            if entry.symlink { " ↗" } else { "" }
        );
        if images {
            icon_slots.push(image_slot(
                entry,
                app.file_tab(pane).request,
                Rect::new(area.x + 3, area.y + 1 + (index - start) as u16, 2, 1),
                palette,
            ));
        }
        rendered.push(
            Row::new(vec![
                Cell::from(name).style(if entry.directory {
                    base.fg(palette.color("folder_icon"))
                } else {
                    base
                }),
                Cell::from(if entry.directory {
                    "—".into()
                } else {
                    fs::size(entry.size)
                }),
                Cell::from(fs::kind(entry)),
                Cell::from(fs::date(entry.modified)),
            ])
            .style(base),
        );
    }
    let table = Table::new(
        rendered,
        [
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(if area.width > 65 { 12 } else { 0 }),
            Constraint::Length(if area.width > 85 { 15 } else { 0 }),
        ],
    )
    .header(
        Row::new([
            " Имя",
            "Размер",
            "Тип",
            if is_trash {
                "Удалён"
            } else {
                "Изменён"
            },
        ])
        .style(palette.muted()),
    )
    .column_spacing(1);
    frame.render_widget(table, area);
    let len = tab.visible.len();
    app.icon_slots.extend(icon_slots);
    for index in start..(start + rows).min(len) {
        if app.renaming(
            pane,
            &app.file_tab(pane).entries[app.file_tab(pane).visible[index]].path,
        ) {
            let prefix = if images { 6 } else { 3 };
            let name_width = area.width.saturating_sub(
                13 + if area.width > 65 { 12 } else { 0 } + if area.width > 85 { 15 } else { 0 },
            );
            app.rename_area = Some(Rect::new(
                area.x + prefix,
                area.y + 1 + (index - start) as u16,
                name_width.saturating_sub(prefix),
                1,
            ));
        }
        hit(
            app,
            Rect::new(area.x, area.y + 1 + (index - start) as u16, area.width, 1),
            Target::Entry { pane, index },
        );
        if area.width > 3 {
            hit(
                app,
                Rect::new(area.right() - 1, area.y + 1 + (index - start) as u16, 1, 1),
                Target::EntryBackground,
            );
        }
        hit(
            app,
            Rect::new(
                area.x,
                area.y + 1 + (index - start) as u16,
                3.min(area.width),
                1,
            ),
            Target::EntryMark { pane, index },
        );
    }
}

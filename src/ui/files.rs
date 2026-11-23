use super::*;
use ratatui::{
    layout::Alignment,
    style::Style,
    widgets::{Cell, Row, Table},
};

fn image_slot(
    entry: &fs::Entry,
    request: u64,
    decoration: Option<&crate::folder_style::Decoration>,
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
        content: decoration.and_then(|d| d.image.clone()),
        kind,
        area,
        foreground: rgb("text"),
        accent: decoration
            .and_then(|d| d.style.color)
            .unwrap_or_else(|| rgb(kind.color_role())),
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
    let is_trash = app.trash_in(pane);
    let detail_list = app.settings.view_mode() == crate::view::ViewMode::List && !is_trash;
    let tab = app.file_tab(pane);
    let count = tab
        .filter_run
        .as_ref()
        .map_or(tab.visible.len(), |r| r.worker.results.total);
    let counter = if detail_list && (!tab.loading || tab.refreshing) && tab.error.is_none() {
        format!(" ({count})")
    } else {
        String::new()
    };
    let title = format!(
        " {}{counter} · {} ",
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
            " {}{counter} ",
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
        Constraint::Length(u16::from(!compact && !detail_list)),
        Constraint::Min(1),
        Constraint::Length(if compact { 0 } else { 1 }),
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
    super::filtering::bar(app, frame, parts[5], pane, palette);
    let tab = app.file_tab(pane);
    if is_trash {
        super::trash::controls(app, frame, parts[2], palette);
    } else if !compact {
        let mut labels = Vec::new();
        if !detail_list
            || !app
                .settings
                .list_columns
                .columns
                .iter()
                .any(|c| c.column == tab.sort.field().0)
        {
            labels.push(tab.sort.label());
        }
        if !detail_list {
            labels.push(format!("{count} элементов"));
            if !tab.filter.is_empty() {
                labels.push(format!("/ {}", tab.filter));
            }
        }
        if tab.archive.is_some() {
            labels.push("Архив: только чтение".into());
        }
        if !labels.is_empty() {
            let row = if detail_list {
                // The spare line above the breadcrumbs can show archive or hidden-sort details.
                Rect::new(parts[1].x, parts[1].y, parts[1].width, 1)
            } else {
                parts[2]
            };
            frame.render_widget(
                Paragraph::new(format!(" {}", labels.join("  ·  ")))
                    .style(palette.text().fg(palette.color("separator"))),
                row,
            );
        }
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
    if app.file_tab(pane).grouping.enabled && !app.file_tab(pane).visible.is_empty() {
        grouped(app, frame, parts[3], pane, palette);
    } else if app.settings.view_mode() == crate::view::ViewMode::Icons {
        app.file_tab_mut(pane).group_view = None;
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
        if app.settings.view_mode() == crate::view::ViewMode::Columns {
            columns(app, frame, parts[3], pane, palette);
        } else {
            list(app, frame, parts[3], pane, palette);
        }
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
    } else if tab.filter_run.is_some() {
        format!(
            " {} · выбрано {}",
            app.filter_status(pane),
            tab.marked.len()
        )
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
    if app.settings.view_mode() == crate::view::ViewMode::Columns && parts[4].height > 0 {
        let r = parts[4];
        let owner = crate::app::PathOwner::current(app, pane);
        let request = app.panes[pane].tab().request;
        let tab = app.file_tab(pane);
        let stride = tab.column_rows.max(1);
        let count = tab
            .group_view
            .as_ref()
            .map_or_else(|| tab.visible.len().div_ceil(stride), |v| v.total);
        let first = tab
            .group_view
            .as_ref()
            .map_or(tab.offset / stride, |v| v.offset);
        frame.render_widget(
            Paragraph::new(format!(
                "          {}–{} / {} столбцов",
                if count == 0 { 0 } else { first + 1 },
                (first + tab.columns).min(count),
                count
            ))
            .style(palette.muted()),
            r,
        );
        for (x, label, delta) in [(r.x, " ← ", -1), (r.x + 4, " → ", 1)] {
            let rect = Rect::new(x, r.y, 3, 1);
            frame.render_widget(Paragraph::new(label).style(palette.accent()), rect);
            hit(
                app,
                rect,
                Target::FileColumns {
                    owner: owner.clone(),
                    request,
                    delta,
                },
            );
        }
    }

    if pane == app.active
        && let Some(search) = app.typeahead()
    {
        let mut row = if parts[4].height > 0 {
            parts[4]
        } else {
            parts[5]
        };
        if app.settings.view_mode() == crate::view::ViewMode::Columns {
            let gap = row.width.min(8);
            row.x += gap;
            row.width -= gap;
        }
        if row.width > 3 {
            let label = format!(
                "{}: {}",
                if search.matched {
                    "Набор"
                } else {
                    "Нет совпадений"
                },
                search.text
            );
            app.hits.retain(|(r, _)| r.intersection(row).is_empty());
            frame.render_widget(ratatui::widgets::Clear, row);
            frame.render_widget(Block::default().style(palette.text()), row);
            frame.render_widget(
                Paragraph::new(shorten_name(&label, row.width.saturating_sub(3) as usize))
                    .style(palette.accent().add_modifier(Modifier::BOLD)),
                Rect::new(row.x, row.y, row.width - 3, 1),
            );
            let close = Rect::new(row.right() - 3, row.y, 3, 1);
            frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
            hit(app, close, Target::ClearTypeahead);
        }
    }
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
        super::text_edit::rename_controls(
            app,
            frame,
            if parts[4].height > 0 {
                parts[4]
            } else {
                parts[5]
            },
            palette,
        );
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
            } else if let Some(location) =
                app.network.location(&tab.cwd).filter(|l| l.root == tab.cwd)
            {
                location.label.clone()
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
    tab.column_rows = 0;
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
            Paragraph::new(if app.file_tab(pane).filtered() {
                "\n Нет совпадений"
            } else {
                "\n Папка пуста"
            })
            .style(palette.muted()),
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
        draw_tile(app, frame, rect, pane, index, palette);
    }
}
fn name_suffix(app: &App, pane: usize, entry: &fs::Entry, width: u16, show_size: bool) -> String {
    if entry.symlink {
        return " ↗".into();
    }
    if show_size
        && !app.renaming(pane, &entry.path)
        && let Some(size) = app.folder_size_label(pane, entry)
        && size.width() + 4 <= usize::from(width)
    {
        return format!(" {size}");
    }
    String::new()
}

fn size_text(app: &App, pane: usize, entry: &fs::Entry) -> String {
    app.folder_size_label(pane, entry)
        .unwrap_or_else(|| crate::columns::Column::Size.text(entry))
}

fn draw_tile(
    app: &mut App,
    frame: &mut Frame,
    rect: Rect,
    pane: usize,
    index: usize,
    palette: Palette,
) {
    if app.file_tab(pane).filtered() && rect.height < 4 {
        draw_name_row(
            app,
            frame,
            Rect::new(rect.x, rect.y, rect.width, 1),
            pane,
            index,
            palette,
            false,
        );
        if rect.height > 1 {
            draw_location(
                app,
                frame,
                Rect::new(rect.x, rect.y + 1, rect.width, 1),
                pane,
                index,
                palette,
            );
        }
        return;
    }
    let entry = &app.file_tab(pane).entries[app.file_tab(pane).visible[index]];
    let decoration = app.folder_decoration(entry);
    let symbol = decoration.and_then(|d| d.style.symbol());
    let color = folder_color(decoration, palette);
    let renaming = app.renaming(pane, &entry.path);
    let selected = app.file_tab(pane).selected == index && !app.file_tab(pane).explicit_selection;
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
    let icon_style = base.fg(if entry.directory {
        color
    } else {
        palette.color("folder_path")
    });
    let icon_area = Rect::new(
        rect.x,
        rect.y,
        rect.width,
        2.min(rect.height.saturating_sub(1)),
    );
    let slot = if symbol.is_none()
        && !icon_area.is_empty()
        && app.icon_backend.supports_images()
        && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images
    {
        let width = icon_area.width.min(6);
        Some(image_slot(
            entry,
            app.file_tab(pane).request,
            decoration,
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
            Paragraph::new(symbol.map_or_else(|| icon.join("\n"), str::to_owned))
                .alignment(Alignment::Center)
                .style(icon_style),
            icon_area,
        );
        None
    };
    let mut rename_area = None;
    if rect.height > 0 && rect.width > 2 {
        let prefix = if marked { "● " } else { "" };
        let label_width = rect.width - 2;
        let tag = if !renaming
            && (app.settings.auto_folder_sizes || app.file_tab(pane).filtered() || rect.height < 4)
        {
            folder_tag(decoration, label_width)
        } else {
            String::new()
        };
        let suffix = name_suffix(
            app,
            pane,
            entry,
            label_width.saturating_sub((tag.width() + prefix.width()) as u16),
            rect.height < 4 || app.file_tab(pane).filtered(),
        );
        let name_width = usize::from(label_width).saturating_sub(prefix.width() + suffix.width());
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
                Paragraph::new(suffix.as_str()).style(base),
                Rect::new(suffix_x, row.y, suffix.width() as u16, 1),
            );
            rename_area = Some(field);
        } else {
            let label = format!(
                "{prefix}{}{suffix}{tag}",
                shorten_name(
                    app.entry_name(pane, entry),
                    name_width.saturating_sub(tag.width())
                )
            );
            frame.render_widget(
                Paragraph::new(label)
                    .alignment(Alignment::Center)
                    .style(typeahead_style(
                        app,
                        pane,
                        index,
                        if decoration.and_then(|d| d.style.color).is_some() {
                            base.fg(color)
                        } else {
                            base
                        },
                    )),
                row,
            );
        }
    }
    if rect.height > 3 {
        let detail = if app.file_tab(pane).filtered() {
            location_label(app, pane, entry)
        } else if let Some(size) = app.folder_size_label(pane, entry) {
            size
        } else if let Some(tag) = decoration.map(|d| &d.style.tag).filter(|t| !t.is_empty()) {
            format!("#{tag}")
        } else if entry.directory {
            "Папка".into()
        } else {
            fs::size(entry.size)
        };
        frame.render_widget(
            Paragraph::new(shorten_name(
                &detail,
                usize::from(rect.width.saturating_sub(2)),
            ))
            .alignment(Alignment::Center)
            .style(base.fg(palette.color("folder_path"))),
            Rect::new(rect.x + 1, rect.y + 3, rect.width.saturating_sub(2), 1),
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

fn folder_tag(decoration: Option<&crate::folder_style::Decoration>, width: u16) -> String {
    decoration
        .map(|d| d.style.tag.as_str())
        .filter(|tag| !tag.is_empty() && width >= 8)
        .map(|tag| format!(" #{}", shorten_name(tag, usize::from(width / 2) - 2)))
        .unwrap_or_default()
}

fn folder_color(
    decoration: Option<&crate::folder_style::Decoration>,
    palette: Palette,
) -> ratatui::style::Color {
    decoration.and_then(|d| d.style.color).map_or_else(
        || palette.color("folder_icon"),
        |[r, g, b]| ratatui::style::Color::Rgb(r, g, b),
    )
}

fn row_style(app: &App, pane: usize, index: usize, palette: Palette) -> Style {
    let tab = app.file_tab(pane);
    let entry = &tab.entries[tab.visible[index]];
    if (tab.selected == index && !tab.explicit_selection) || tab.marked.contains(&entry.path) {
        palette.selection()
    } else {
        palette.text()
    }
}
fn columns(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    let entry_height = app.file_tab(pane).entry_height();
    let rows = (usize::from(area.height.saturating_sub(1)) / entry_height).max(1);
    let cols = usize::from(app.settings.view_columns)
        .min(usize::from(area.width) / 12)
        .max(1);
    let tab = app.file_tab_mut(pane);
    tab.columns = cols;
    tab.column_rows = rows;
    tab.page_size = rows * cols;
    let total = tab.visible.len().div_ceil(rows);
    let mut first = tab.offset / rows;
    if tab.follow_selection {
        let selected = tab.selected / rows;
        if selected < first {
            first = selected;
        }
        if selected >= first + cols {
            first = selected + 1 - cols;
        }
    }
    first = first.min(total.saturating_sub(cols));
    tab.offset = first * rows;
    let start = tab.offset;
    let len = tab.visible.len();
    frame.render_widget(
        Paragraph::new(" Имя").style(palette.muted()),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.height <= 1 {
        return;
    }
    for index in start..(start + rows * cols).min(len) {
        let local = index - start;
        let col = local / rows;
        let row = local % rows;
        let x = area.x + (col * usize::from(area.width) / cols) as u16;
        let right = area.x + ((col + 1) * usize::from(area.width) / cols) as u16;
        let r = Rect::new(x, area.y + 1 + (row * entry_height) as u16, right - x, 1);
        draw_name_row(app, frame, r, pane, index, palette, false);
        if entry_height > 1 && r.y + 1 < area.bottom() {
            draw_location(
                app,
                frame,
                Rect::new(r.x, r.y + 1, r.width, 1),
                pane,
                index,
                palette,
            );
        }
    }
}
/// One row owns its name, icon, mark and optional expansion control in every backend.
fn draw_name_row(
    app: &mut App,
    frame: &mut Frame,
    rect: Rect,
    pane: usize,
    index: usize,
    palette: Palette,
    tree: bool,
) {
    let tab = app.file_tab(pane);
    let entry = &tab.entries[tab.visible[index]];
    let style = row_style(app, pane, index, palette);
    let marked = tab.marked.contains(&entry.path);
    let decoration = app.folder_decoration(entry);
    let color = folder_color(decoration, palette);
    let symbol = decoration.and_then(|d| d.style.symbol());
    let images = rect.width >= 10
        && symbol.is_none()
        && app.icon_backend.supports_images()
        && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images;
    let depth = if tree {
        entry
            .path
            .strip_prefix(&tab.cwd)
            .map_or(0, |p| p.components().count().saturating_sub(1))
    } else {
        0
    };
    let indent = (depth.saturating_mul(2))
        .min(usize::from(rect.width.saturating_sub(16)))
        .min(32) as u16;
    let mark_width = if rect.width >= 6 { 3 } else { 1 };
    let toggle_x = rect.x + mark_width + indent;
    let has_icon = images || (rect.width >= 8 && symbol.is_some());
    let prefix = mark_width + indent + if tree { 2 } else { 0 } + if has_icon { 3 } else { 0 };
    let field = Rect::new(
        rect.x + prefix.min(rect.width),
        rect.y,
        rect.width.saturating_sub(prefix + 1),
        1,
    );
    let status = tab.tree.as_ref().and_then(|t| t.branches.get(&entry.path));
    let arrow = if !entry.directory || entry.symlink {
        "  "
    } else if status.is_some_and(|b| b.loading) {
        "… "
    } else if status.is_some_and(|b| b.error.is_some()) {
        "! "
    } else if status.is_some() {
        "↓ "
    } else {
        "→ "
    };
    let path = entry.path.clone();
    let toggle = (tree && !app.trash_in(pane) && entry.directory && !entry.symlink).then(|| {
        Target::TreeToggle {
            owner: crate::app::PathOwner::current(app, pane),
            request: tab.request,
            revision: tab.tree.as_ref().map_or(0, |t| t.revision),
            path: path.clone(),
        }
    });
    let name = app.entry_name(pane, entry);
    let name_only = match app.settings.view_mode() {
        crate::view::ViewMode::Tree => false,
        crate::view::ViewMode::List => !app
            .settings
            .list_columns
            .columns
            .iter()
            .any(|s| s.column == crate::columns::Column::Size),
        _ => true,
    };
    frame.render_widget(Block::default().style(style), rect);
    frame.render_widget(
        Paragraph::new(if mark_width == 1 {
            if marked { "●" } else { "□" }
        } else if marked {
            " ● "
        } else {
            " □ "
        })
        .style(style),
        Rect::new(rect.x, rect.y, mark_width.min(rect.width), 1),
    );
    if tree {
        frame.render_widget(
            Paragraph::new(arrow).style(style.fg(palette.color("active"))),
            Rect::new(toggle_x, rect.y, 2, 1),
        );
    }
    if depth.saturating_mul(2) > usize::from(indent) && indent > 0 {
        frame.render_widget(
            Paragraph::new("…").style(palette.muted()),
            Rect::new(rect.x + 3, rect.y, 1, 1),
        );
    }
    let tag = if app.renaming(pane, &path) {
        String::new()
    } else {
        folder_tag(decoration, field.width)
    };
    let suffix = name_suffix(
        app,
        pane,
        entry,
        field.width.saturating_sub(tag.width() as u16),
        name_only,
    );
    if has_icon && let Some(symbol) = symbol {
        frame.render_widget(
            Paragraph::new(symbol).style(style.fg(color)),
            Rect::new(field.x - 3, rect.y, 2, 1),
        );
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{}{suffix}{tag}",
            shorten_name(
                name,
                usize::from(field.width).saturating_sub(suffix.width() + tag.width())
            )
        ))
        .style(typeahead_style(
            app,
            pane,
            index,
            if entry.directory {
                style.fg(color)
            } else {
                style
            },
        )),
        field,
    );
    let slot = images.then(|| {
        image_slot(
            entry,
            tab.request,
            decoration,
            Rect::new(field.x - 3, rect.y, 2, 1),
            palette,
        )
    });
    if app.renaming(pane, &path) {
        app.rename_area = Some(field);
    }
    app.icon_slots.extend(slot);
    hit(app, rect, Target::Entry { pane, index });
    hit(
        app,
        Rect::new(rect.right() - 1, rect.y, 1, 1),
        Target::EntryBackground,
    );
    hit(
        app,
        Rect::new(rect.x, rect.y, mark_width.min(rect.width), 1),
        Target::EntryMark { pane, index },
    );
    if let Some(target) = toggle {
        hit(app, Rect::new(toggle_x, rect.y, 2, 1), target);
    }
}
fn list(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    let entry_height = app.file_tab(pane).entry_height();
    let rows = usize::from(area.height.saturating_sub(1)) / entry_height;
    if rows == 0 {
        return;
    }
    let tree = app.settings.view_mode() == crate::view::ViewMode::Tree;
    let is_trash = app.trash_in(pane);
    let tab = app.file_tab_mut(pane);
    tab.columns = 1;
    tab.column_rows = 0;
    tab.page_size = rows;
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
    let end = (start + rows).min(tab.visible.len());
    if !tree {
        let rows: Vec<_> = (start..end)
            .map(|i| (i, area.y + 1 + ((i - start) * entry_height) as u16))
            .collect();
        detail_list(app, frame, area, pane, palette, &rows);
        if entry_height > 1 {
            for (index, y) in rows {
                draw_location(
                    app,
                    frame,
                    Rect::new(area.x, y + 1, area.width, 1),
                    pane,
                    index,
                    palette,
                );
            }
        }
        return;
    }
    let tab = app.file_tab(pane);
    let details = tree;
    let name_width = if details {
        area.width.saturating_sub(
            13 + if area.width > 65 { 12 } else { 0 } + if area.width > 85 { 15 } else { 0 },
        )
    } else {
        area.width
    };
    if details {
        let rendered = (start..end)
            .map(|index| {
                let entry = &tab.entries[tab.visible[index]];
                Row::new(vec![
                    Cell::from(""),
                    Cell::from(size_text(app, pane, entry)),
                    Cell::from(fs::kind(entry)),
                    Cell::from(fs::date(entry.modified)),
                ])
                .height(entry_height as u16)
                .style(row_style(app, pane, index, palette))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Table::new(
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
            .column_spacing(1),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(" Имя").style(palette.muted()),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }
    for index in start..end {
        let y = area.y + 1 + ((index - start) * entry_height) as u16;
        hit(
            app,
            Rect::new(area.x, y, area.width, 1),
            Target::Entry { pane, index },
        );
        draw_name_row(
            app,
            frame,
            Rect::new(area.x, y, name_width, 1),
            pane,
            index,
            palette,
            tree && !is_trash && !app.file_tab(pane).filtered(),
        );
        if entry_height > 1 {
            draw_location(
                app,
                frame,
                Rect::new(area.x, y + 1, area.width, 1),
                pane,
                index,
                palette,
            );
        }
        hit(
            app,
            Rect::new(area.right() - 1, y, 1, 1),
            Target::EntryBackground,
        );
    }
}

fn detail_list(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    pane: usize,
    palette: Palette,
    rows: &[(usize, u16)],
) {
    use crate::columns::Column;
    let header = Rect::new(area.x, area.y, area.width, 1);
    app.set_column_header(pane, header);
    let context = app.column_owner(pane, header);
    let specs = app.settings.list_columns.columns.clone();
    let widths = app.settings.list_columns.widths(area.width);
    let sort = app.file_tab(pane).sort.field();
    let mut x = area.x;
    for (spec, width) in specs.iter().zip(widths) {
        let r = Rect::new(x, area.y, width, 1);
        x += width;
        if width == 0 {
            continue;
        }
        let label = if app.trash_in(pane) && spec.column == Column::Modified {
            "Дата удаления"
        } else {
            spec.column.label()
        };
        let title = format!(
            "{}{}{}",
            label,
            if sort.0 == spec.column {
                if sort.1 { " ↓" } else { " ↑" }
            } else {
                ""
            },
            if spec.manual { " *" } else { "" }
        );
        frame.render_widget(
            Paragraph::new(shorten_name(&title, usize::from(width.saturating_sub(1))))
                .style(palette.muted()),
            r,
        );
        hit(
            app,
            r,
            Target::ColumnHeader {
                context: context.clone(),
                column: spec.column,
            },
        );
        let edge = Rect::new(r.right() - 1, r.y, 1, 1);
        frame.render_widget(Paragraph::new("│").style(palette.accent()), edge);
        hit(
            app,
            edge,
            Target::ColumnBoundary {
                context: context.clone(),
                column: spec.column,
            },
        );
        for &(index, y) in rows {
            let cell = Rect::new(r.x, y, width, 1);
            if spec.column == Column::Name {
                draw_name_row(app, frame, cell, pane, index, palette, false);
            } else {
                let tab = app.file_tab(pane);
                let entry = &tab.entries[tab.visible[index]];
                let style = row_style(app, pane, index, palette);
                let value = if spec.column == Column::Size {
                    size_text(app, pane, entry)
                } else {
                    spec.column.text(entry)
                };
                frame.render_widget(Block::default().style(style), cell);
                frame.render_widget(
                    Paragraph::new(shorten_name(&value, usize::from(width.saturating_sub(1))))
                        .style(style),
                    cell,
                );
                hit(app, cell, Target::Entry { pane, index });
            }
            frame.render_widget(
                Paragraph::new("│").style(palette.muted()),
                Rect::new(cell.right() - 1, cell.y, 1, 1),
            );
        }
    }
    // Ordinary row hits keep metadata cells usable for selection, opening and drag.
    for &(_, y) in rows {
        hit(
            app,
            Rect::new(area.right() - 1, y, 1, 1),
            Target::EntryBackground,
        );
    }
}

fn typeahead_style(app: &App, pane: usize, index: usize, style: Style) -> Style {
    if pane == app.active
        && app.file_tab(pane).selected == index
        && app.typeahead().is_some_and(|s| s.matched)
    {
        style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        style
    }
}

fn grouped(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    use crate::view::ViewMode;
    let mode = app.settings.view_mode();
    let compact_columns =
        mode == ViewMode::Columns && app.file_tab(pane).filtered() && area.height < 4;
    let content = if mode == ViewMode::Icons || compact_columns {
        area
    } else {
        Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        )
    };
    app.set_file_area(pane, content);
    if content.is_empty() {
        return;
    }
    let columns = match mode {
        ViewMode::Icons => (usize::from(area.width) / 18).max(1),
        ViewMode::Columns => usize::from(app.settings.view_columns)
            .min(usize::from(area.width) / 12)
            .max(1),
        _ => 1,
    };
    let tab = app.file_tab_mut(pane);
    let entry_height = tab.entry_height();
    let changed = tab.group_view.as_ref().is_none_or(|v| {
        v.mode != mode
            || v.width != content.width
            || v.height != content.height
            || v.columns != columns
            || v.entry_height != entry_height
    });
    if changed {
        let mut v = crate::grouping::Viewport::new_with_rows(
            &tab.groups,
            mode,
            content.width,
            content.height,
            columns,
            entry_height,
        );
        if !tab.follow_selection {
            v.anchor(tab.offset);
        }
        tab.group_view = Some(v);
    }
    tab.columns = columns;
    tab.column_rows = if mode == ViewMode::Columns {
        (usize::from(content.height.saturating_sub(1)) / entry_height).max(1)
    } else {
        0
    };
    tab.page_size = match mode {
        ViewMode::Icons => columns * (usize::from(content.height) / 5).max(1),
        ViewMode::Columns => columns * tab.column_rows,
        _ => usize::from(content.height).max(1),
    };
    let view = tab.group_view.as_mut().unwrap();
    if tab.follow_selection {
        view.follow(tab.selected);
    }
    tab.offset = view.first();
    let items = view.items();
    let headings = view
        .headers()
        .into_iter()
        .map(|(g, x, y)| (tab.groups[g].label.clone(), tab.groups[g].count, x, y))
        .collect::<Vec<_>>();
    let rows: Vec<_> = items
        .iter()
        .map(|&(i, _, y)| (i, content.y + y as u16))
        .collect();
    if mode == ViewMode::List {
        detail_list(app, frame, area, pane, palette, &rows);
    } else if mode == ViewMode::Tree {
        // Reuse the ordinary tree controls; expanded descendants stay with roots.
        let name_width = area.width.saturating_sub(
            13 + if area.width > 65 { 12 } else { 0 } + if area.width > 85 { 15 } else { 0 },
        );
        let widths = [
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(if area.width > 65 { 12 } else { 0 }),
            Constraint::Length(if area.width > 85 { 15 } else { 0 }),
        ];
        frame.render_widget(
            Table::new(
                [Row::new([" Имя", "Размер", "Тип", "Изменён"]).style(palette.muted())],
                widths,
            )
            .column_spacing(1),
            Rect::new(area.x, area.y, area.width, 1),
        );
        for &(index, y) in &rows {
            let e = &app.file_tab(pane).entries[app.file_tab(pane).visible[index]];
            let r = Rect::new(area.x, y, area.width, 1);
            frame.render_widget(
                Table::new(
                    [Row::new(vec![
                        String::new(),
                        size_text(app, pane, e),
                        fs::kind(e).into(),
                        fs::date(e.modified),
                    ])
                    .style(row_style(app, pane, index, palette))],
                    widths,
                )
                .column_spacing(1),
                r,
            );
            hit(app, r, Target::Entry { pane, index });
            draw_name_row(
                app,
                frame,
                Rect::new(r.x, r.y, name_width, 1),
                pane,
                index,
                palette,
                !app.trash_in(pane) && !app.file_tab(pane).filtered(),
            );
            hit(
                app,
                Rect::new(r.right() - 1, y, 1, 1),
                Target::EntryBackground,
            );
        }
    } else {
        if mode == ViewMode::Columns && !compact_columns {
            frame.render_widget(
                Paragraph::new(" Имя").style(palette.muted()),
                Rect::new(area.x, area.y, area.width, 1),
            );
        }
        for (index, col, row) in items {
            let left = content.x + (col * usize::from(content.width) / columns) as u16;
            let right = content.x + ((col + 1) * usize::from(content.width) / columns) as u16;
            let r = Rect::new(
                left,
                content.y + row as u16,
                right - left,
                if mode == ViewMode::Icons {
                    5.min(content.height.saturating_sub(row as u16))
                } else {
                    1
                },
            );
            if mode == ViewMode::Icons {
                draw_tile(app, frame, r, pane, index, palette);
            } else {
                draw_name_row(app, frame, r, pane, index, palette, false);
            }
        }
    }
    if mode != ViewMode::Icons && entry_height > 1 {
        let items = app.file_tab(pane).group_view.as_ref().unwrap().items();
        for (index, col, row) in items {
            if row + 1 >= usize::from(content.height) {
                continue;
            }
            let (x, width) = if mode == ViewMode::Columns {
                let left = content.x + (col * usize::from(content.width) / columns) as u16;
                let right = content.x + ((col + 1) * usize::from(content.width) / columns) as u16;
                (left, right - left)
            } else {
                (content.x, content.width)
            };
            draw_location(
                app,
                frame,
                Rect::new(x, content.y + row as u16 + 1, width, 1),
                pane,
                index,
                palette,
            );
        }
    }
    for (label, count, col, row) in headings {
        let r = if mode == ViewMode::Columns {
            let x = content.x + (col * usize::from(content.width) / columns) as u16;
            let right = content.x + ((col + 1) * usize::from(content.width) / columns) as u16;
            Rect::new(x, content.y + row as u16, right - x, 1)
        } else {
            Rect::new(content.x, content.y + row as u16, content.width, 1)
        };
        let counted = format!("{label} · {count}");
        let budget = usize::from(r.width.saturating_sub(3));
        let label = shorten_name(
            if counted.width() <= budget {
                &counted
            } else {
                &label
            },
            budget,
        );
        let line = format!(
            " {label} {}",
            "─".repeat(usize::from(r.width).saturating_sub(label.width() + 2))
        );
        frame.render_widget(
            Paragraph::new(line).style(palette.accent().add_modifier(Modifier::BOLD)),
            r,
        );
        hit(app, r, Target::EntryBackground);
    }
}

fn location_label(app: &App, pane: usize, entry: &fs::Entry) -> String {
    let root = &app.file_tab(pane).cwd;
    let parent = entry.path.parent().unwrap_or(root);
    let path = parent
        .strip_prefix(root)
        .ok()
        .map(|p| {
            root.file_name()
                .map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from)
                .join(p)
        })
        .unwrap_or_else(|| parent.to_path_buf());
    fs::clean(&path.to_string_lossy())
}
fn draw_location(
    app: &mut App,
    frame: &mut Frame,
    r: Rect,
    pane: usize,
    index: usize,
    palette: Palette,
) {
    let e = &app.file_tab(pane).entries[app.file_tab(pane).visible[index]];
    let label = location_label(app, pane, e);
    let base = row_style(app, pane, index, palette).fg(palette.color("folder_path"));
    frame.render_widget(Block::default().style(base), r);
    frame.render_widget(
        Paragraph::new(format!(
            "  {}",
            shorten_name(&label, usize::from(r.width.saturating_sub(2)))
        ))
        .style(base),
        r,
    );
    hit(app, r, Target::Entry { pane, index });
}

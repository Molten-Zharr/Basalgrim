use super::*;
use ratatui::{
    layout::Alignment,
    style::Style,
    widgets::{Cell, Row, Table},
};

fn image_slot(entry: &fs::Entry, area: Rect, palette: Palette) -> crate::icons::IconSlot {
    let kind = crate::icons::IconKind::for_entry(entry);
    let rgb = |token| match palette.color(token) {
        ratatui::style::Color::Rgb(r, g, b) => [r, g, b],
        _ => unreachable!("Palette colors are RGB"),
    };
    crate::icons::IconSlot {
        kind,
        area,
        foreground: rgb("text"),
        accent: rgb(kind.color_role()),
        background: rgb("background"),
    }
}

pub fn render(app: &mut App, frame: &mut Frame, area: Rect, pane: usize, palette: Palette) {
    app.pane_areas[pane] = area;
    let focused = app.active == pane;
    let block = panel(
        palette,
        format!(
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
        ),
        focused,
    )
    .border_style(palette.text().fg(palette.color(if focused {
        "active"
    } else {
        "inactive_file_border"
    })));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 {
        return;
    }
    let compact = inner.height < 9;
    let parts = Layout::vertical([
        Constraint::Length(if compact { 0 } else { 1 }),
        Constraint::Length(if compact { 1 } else { 2 }),
        Constraint::Length(if compact { 0 } else { 1 }),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let mut x = parts[0].x;
    for i in 0..if compact {
        0
    } else {
        app.panes[pane].tabs.len()
    } {
        let tab = &app.panes[pane].tabs[i];
        let label = format!(
            " {}:{} ",
            i + 1,
            fs::clean(
                &tab.cwd
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("/"))
                    .to_string_lossy()
            )
        );
        let width = (label.width() as u16)
            .min(22)
            .min(parts[0].right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let rect = Rect::new(x, parts[0].y, width, 1);
        frame.render_widget(
            Paragraph::new(label).style(if app.panes[pane].active == i {
                palette.selection()
            } else {
                palette.muted()
            }),
            rect,
        );
        hit(app, rect, Target::Tab { pane, index: i });
        x += width + 1;
    }
    if focused {
        hit(app, parts[1], Target::Action(Action::Path));
    }
    let tab = app.panes[pane].tab();
    frame.render_widget(
        Paragraph::new(format!(" {}", fs::clean(&tab.cwd.to_string_lossy())))
            .style(palette.muted()),
        Rect::new(parts[1].x, parts[1].bottom() - 1, parts[1].width, 1),
    );
    let filter = if tab.filter.is_empty() {
        String::new()
    } else {
        format!("  / {}", tab.filter)
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {}  ·  {} элементов{}",
            tab.sort.label(),
            tab.visible.len(),
            filter
        ))
        .style(palette.text().fg(palette.color("separator"))),
        parts[2],
    );
    if tab.loading {
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
    let tab = app.panes[pane].tab();
    let total = tab.visible.iter().fold(0u64, |sum, &i| {
        sum.saturating_add(if tab.entries[i].directory {
            0
        } else {
            tab.entries[i].size
        })
    });
    let footer = if tab.marked.is_empty() && !tab.explicit_selection {
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
    let tab = app.panes[pane].tab_mut();
    tab.columns = columns;
    tab.page_size = count;
    let selected_row = tab.selected / columns;
    let mut top_row = tab.offset / columns;
    if selected_row < top_row {
        top_row = selected_row;
    }
    if selected_row >= top_row + rows {
        top_row = selected_row + 1 - rows;
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
        let entry = &app.panes[pane].tab().entries[app.panes[pane].tab().visible[index]];
        let selected =
            app.panes[pane].tab().selected == index && !app.panes[pane].tab().explicit_selection;
        let marked = app.panes[pane].tab().marked.contains(&entry.path);
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
        let icon_area = Rect::new(rect.x, rect.y, rect.width, 2.min(rect.height));
        if app.icon_backend.supports_images()
            && app.settings.appearance.icon_mode == crate::preferences::IconMode::Images
        {
            let width = icon_area.width.min(6);
            app.icon_slots.push(image_slot(
                entry,
                Rect::new(
                    icon_area.x + (icon_area.width - width) / 2,
                    icon_area.y,
                    width,
                    icon_area.height,
                ),
                palette,
            ));
        } else {
            frame.render_widget(
                Paragraph::new(icon.join("\n"))
                    .alignment(Alignment::Center)
                    .style(icon_style),
                icon_area,
            );
        }
        if rect.height > 2 && rect.width > 2 {
            let prefix = if marked { "● " } else { "" };
            let suffix = if entry.symlink { " ↗" } else { "" };
            let label_width = rect.width - 2;
            let name_width =
                usize::from(label_width).saturating_sub(prefix.width() + suffix.width());
            let label = format!("{prefix}{}{suffix}", shorten_name(&entry.name, name_width));
            frame.render_widget(
                Paragraph::new(label)
                    .alignment(Alignment::Center)
                    .style(base),
                Rect::new(rect.x + 1, rect.y + 2, label_width, 1),
            );
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
        hit(app, rect, Target::Entry { pane, index });
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
    let tab = app.panes[pane].tab_mut();
    tab.columns = 1;
    tab.page_size = rows.max(1);
    if rows == 0 {
        return;
    }
    if tab.selected < tab.offset {
        tab.offset = tab.selected;
    }
    if tab.selected >= tab.offset + rows {
        tab.offset = tab.selected + 1 - rows;
    }
    tab.offset = tab.offset.min(tab.visible.len().saturating_sub(rows));
    let start = tab.offset;
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
            entry.name,
            if entry.symlink { " ↗" } else { "" }
        );
        if images {
            app.icon_slots.push(image_slot(
                entry,
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
    .header(Row::new([" Имя", "Размер", "Тип", "Изменён"]).style(palette.muted()))
    .column_spacing(1);
    frame.render_widget(table, area);
    let len = tab.visible.len();
    for index in start..(start + rows).min(len) {
        hit(
            app,
            Rect::new(area.x, area.y + 1 + (index - start) as u16, area.width, 1),
            Target::Entry { pane, index },
        );
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

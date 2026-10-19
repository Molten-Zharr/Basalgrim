use super::*;
use crate::{
    app::{SettingsFocus, SettingsSection, SettingsTab},
    input::{self, KeyPress},
};
use ratatui::{
    layout::Alignment,
    widgets::{Clear, Wrap},
};

fn button(
    app: &mut App,
    frame: &mut Frame,
    rect: Rect,
    label: &str,
    target: Target,
    palette: Palette,
) {
    if rect.is_empty() {
        return;
    }
    frame.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(palette.accent()),
        rect,
    );
    hit(app, rect, target);
}

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let block = panel(palette, " Basalgrim / Настройки ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.settings_view.parameters_area = Rect::default();
    app.settings_view.tabs_area = Rect::default();
    if inner.width < 32 || inner.height < 12 {
        frame.render_widget(
            Paragraph::new("Увеличьте окно для редактирования настроек. Esc — назад.")
                .wrap(Wrap { trim: false })
                .style(palette.text()),
            inner,
        );
        if inner.height > 0 {
            button(
                app,
                frame,
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                "Esc  Назад",
                Target::Cancel,
                palette,
            );
        }
        return;
    }
    let footer_height = 4;
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - footer_height);
    let footer = Rect::new(inner.x, body.bottom(), inner.width, footer_height);
    let menu_width = if inner.width < 60 { 10 } else { 18 };
    let menu = Rect::new(body.x, body.y, menu_width, body.height);
    let mut content = Rect::new(
        menu.right() + 2,
        body.y,
        body.width - menu_width - 2,
        body.height,
    );
    frame.render_widget(
        Paragraph::new("РАЗДЕЛЫ").style(palette.muted()),
        Rect::new(menu.x, menu.y, menu.width, 1),
    );
    for (i, section) in SettingsSection::ALL.into_iter().enumerate() {
        let rect = Rect::new(menu.x, menu.y + 2 + i as u16 * 2, menu.width, 1);
        if rect.bottom() > menu.bottom() {
            break;
        }
        let style = if app.settings_view.section == section {
            palette
                .selection()
                .add_modifier(if app.settings_view.focus == SettingsFocus::Menu {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                })
        } else {
            palette.text()
        };
        frame.render_widget(
            Paragraph::new(format!(" {}", section.name())).style(style),
            rect,
        );
        hit(app, rect, Target::SettingsSection(section));
    }
    if app.settings_view.section == SettingsSection::Themes {
        let mut x = content.x;
        let mut y = content.y;
        for tab in SettingsTab::ALL {
            let width = (tab.name().width() as u16 + 2).min(content.width);
            if x + width > content.right() && x > content.x {
                x = content.x;
                y += 1;
            }
            if y >= content.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            let style = if app.settings_view.tab == tab {
                palette.accent().add_modifier(Modifier::UNDERLINED)
            } else {
                palette.muted()
            };
            frame.render_widget(
                Paragraph::new(format!(" {}", tab.name())).style(style),
                rect,
            );
            hit(app, rect, Target::SettingsTab(tab));
            x += width + 1;
        }
        let height = (y - content.y + 2).min(content.height);
        app.settings_view.tabs_area = Rect::new(content.x, content.y, content.width, height);
        content.y += height;
        content.height -= height;
    } else {
        frame.render_widget(
            Paragraph::new("ГОРЯЧИЕ КЛАВИШИ").style(palette.accent()),
            Rect::new(content.x, content.y, content.width, 1),
        );
        content.y += 2;
        content.height = content.height.saturating_sub(2);
    }
    let rows = app.settings_rows();
    let stacked = content.width < 48;
    let step = if stacked { 3 } else { 2 };
    let capacity = (content.height / step).max(1) as usize;
    app.settings_view.capacity = capacity;
    app.settings_view.parameters_area = content;
    app.settings_view.selected = app.settings_view.selected.min(rows.len().saturating_sub(1));
    if app.settings_view.selected < app.settings_view.offset {
        app.settings_view.offset = app.settings_view.selected;
    }
    if app.settings_view.selected >= app.settings_view.offset + capacity {
        app.settings_view.offset = app.settings_view.selected + 1 - capacity;
    }
    app.settings_view.offset = app
        .settings_view
        .offset
        .min(rows.len().saturating_sub(capacity));
    for (visible, (index, row)) in rows
        .iter()
        .enumerate()
        .skip(app.settings_view.offset)
        .take(capacity)
        .enumerate()
    {
        let y = content.y + visible as u16 * step;
        let height = if stacked { 2 } else { 1 };
        if y + height > content.bottom() {
            break;
        }
        let rect = Rect::new(content.x, y, content.width, height);
        let selected = app.settings_view.selected == index
            && app.settings_view.focus == SettingsFocus::Parameters;
        let style = if selected {
            palette.selection()
        } else {
            palette.text()
        };
        frame.render_widget(Block::default().style(style), rect);
        let value_width = if stacked {
            content.width
        } else {
            content.width.min(28)
        };
        let label_width = if stacked {
            content.width
        } else {
            content.width.saturating_sub(value_width + 1)
        };
        frame.render_widget(
            Paragraph::new(row.label.clone()).style(style),
            Rect::new(rect.x, rect.y, label_width, 1),
        );
        hit(
            app,
            rect,
            Target::Setting {
                index,
                activate: false,
                direction: 1,
            },
        );
        let mut value = Rect::new(
            if stacked {
                rect.x
            } else {
                rect.right() - value_width
            },
            rect.y + u16::from(stacked),
            value_width,
            1,
        );
        if let Some([r, g, b]) = row.color {
            frame.render_widget(
                Paragraph::new("■").style(style.fg(ratatui::style::Color::Rgb(r, g, b))),
                Rect::new(value.x, value.y, 1, 1),
            );
            value.x += 2;
            value.width = value.width.saturating_sub(2);
        }
        if row.adjustable && value.width >= 12 {
            button(
                app,
                frame,
                Rect::new(value.x, value.y, 2, 1),
                "‹",
                Target::Setting {
                    index,
                    activate: false,
                    direction: -1,
                },
                palette,
            );
            button(
                app,
                frame,
                Rect::new(value.right() - 2, value.y, 2, 1),
                "›",
                Target::Setting {
                    index,
                    activate: false,
                    direction: 1,
                },
                palette,
            );
            value.x += 2;
            value.width -= 4;
        }
        frame.render_widget(
            Paragraph::new(row.value.clone())
                .alignment(if stacked {
                    Alignment::Left
                } else {
                    Alignment::Right
                })
                .style(style.fg(palette.color("active"))),
            value,
        );
        hit(
            app,
            value,
            Target::Setting {
                index,
                activate: true,
                direction: 1,
            },
        );
    }
    let description = rows
        .get(app.settings_view.selected)
        .map_or("", |r| r.description.as_str());
    frame.render_widget(
        Paragraph::new(description)
            .wrap(Wrap { trim: false })
            .style(palette.muted()),
        Rect::new(footer.x, footer.y, footer.width, 2),
    );
    let status = if let Some(error) = &app.settings_save_error {
        format!("{error} · Ctrl+S — повторить")
    } else if !app.settings_save_available {
        "Сохранение недоступно: исходный файл настроек не будет перезаписан".into()
    } else if app.settings_dirty {
        "Ожидает сохранения…".into()
    } else {
        "Сохранено · изменения применяются сразу".into()
    };
    frame.render_widget(
        Paragraph::new(status).style(if app.settings_save_error.is_some() {
            palette.text().fg(palette.color("error"))
        } else {
            palette.muted()
        }),
        Rect::new(footer.x, footer.y + 2, footer.width, 1),
    );
    if app.settings_save_error.is_some() {
        hit(
            app,
            Rect::new(footer.x, footer.y + 2, footer.width, 1),
            Target::RetrySave,
        );
    }
    let close = Rect::new(footer.x, footer.y + 3, 12.min(footer.width), 1);
    button(app, frame, close, "Esc  Назад", Target::Cancel, palette);
    if footer.width > 14 {
        frame.render_widget(
            Paragraph::new("Tab — фокус  ↑↓ — выбор  ←→ — изменить  Enter — открыть")
                .style(palette.muted()),
            Rect::new(footer.x + 14, footer.y + 3, footer.width - 14, 1),
        );
    }
}

pub fn capture(
    app: &mut App,
    frame: &mut Frame,
    palette: Palette,
    index: usize,
    error: Option<String>,
) {
    app.hits.clear();
    let area = frame.area();
    let width = area.width.min(82);
    let height = area.height.min(24);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Назначить сочетание ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.width < 16 || inner.height < 9 {
        frame.render_widget(
            Paragraph::new("Нажмите сочетание. Esc — отмена.")
                .wrap(Wrap { trim: false })
                .style(palette.text()),
            inner,
        );
        return;
    }
    let action = app.settings.bindings[index].action;
    let title = format!(
        "{} · {}",
        action.name(),
        app.settings.bindings[index].key.label()
    );
    frame.render_widget(
        Paragraph::new(title).style(palette.accent()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new("Нажмите сочетание или выберите модификаторы и клавишу мышью.")
            .wrap(Wrap { trim: false })
            .style(palette.text()),
        Rect::new(inner.x, inner.y + 1, inner.width, 2),
    );
    for (i, label) in ["Ctrl", "Alt", "Shift"].iter().enumerate() {
        let width = (inner.width / 3).max(1);
        let rect = Rect::new(inner.x + i as u16 * width, inner.y + 3, width, 1);
        let label = format!(
            "[{}] {label}",
            if app.settings_view.modifiers[i] {
                "x"
            } else {
                " "
            }
        );
        button(
            app,
            frame,
            rect,
            &label,
            Target::BindingModifier(i),
            palette,
        );
    }
    let keys = input::shortcut_choices();
    let columns = (usize::from(inner.width) / 8).max(1);
    let key_height = usize::from(inner.height.saturating_sub(8));
    let capacity = key_height * columns;
    app.settings_view.binding_capacity = capacity;
    app.settings_view.binding_offset = app
        .settings_view
        .binding_offset
        .min(keys.len().saturating_sub(capacity.max(1)));
    for (visible, key) in keys
        .into_iter()
        .skip(app.settings_view.binding_offset)
        .take(capacity)
        .enumerate()
    {
        let rect = Rect::new(
            inner.x + (visible % columns) as u16 * 8,
            inner.y + 5 + (visible / columns) as u16,
            8.min(inner.width - (visible % columns) as u16 * 8),
            1,
        );
        button(
            app,
            frame,
            rect,
            &KeyPress::plain(key).label(),
            Target::BindingKey(key),
            palette,
        );
    }
    if let Some(error) = error {
        frame.render_widget(
            Paragraph::new(error)
                .wrap(Wrap { trim: false })
                .style(palette.text().fg(palette.color("error"))),
            Rect::new(inner.x, inner.bottom() - 3, inner.width, 2),
        );
    } else {
        frame.render_widget(
            Paragraph::new("Колесо — остальные клавиши; Esc — отмена").style(palette.muted()),
            Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
        );
    }
    button(
        app,
        frame,
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        "Esc  Отмена",
        Target::Cancel,
        palette,
    );
}

use super::*;
use crate::app::ExtensionControl as C;
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let width = area.width.min(84);
    let rows = app.extension_view.as_ref().map_or(0, |v| v.rows.len());
    let height = area
        .height
        .min((rows.saturating_add(11) as u16).clamp(12, 23));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    app.preview_slot = None;
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Расширения ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.width < 28 || inner.height < 10 {
        return;
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
    let Some(v) = &mut app.extension_view else {
        return;
    };
    let token = v.token;
    let revision = v.revision;
    let target = |control| Target::Extension {
        token,
        revision,
        control,
    };
    let mut controls = vec![];
    for (x, w, label, active, control) in [
        (0, 10, " Команды ", v.commands, C::Commands),
        (11, 17, " Установленные ", !v.commands, C::Installed),
    ] {
        let r = Rect::new(inner.x + x, inner.y, w, 1);
        frame.render_widget(
            Paragraph::new(label).style(if active {
                palette.selection()
            } else {
                palette.accent()
            }),
            r,
        );
        controls.push((r, target(control)));
    }
    let capacity = inner.height.saturating_sub(9).max(1) as usize;
    v.page = capacity;
    if v.selected < v.offset {
        v.offset = v.selected;
    }
    if v.selected >= v.offset + capacity {
        v.offset = v.selected + 1 - capacity;
    }
    v.offset = v.offset.min(v.rows.len().saturating_sub(capacity));
    for (i, row) in v.rows.iter().enumerate().skip(v.offset).take(capacity) {
        let r = Rect::new(inner.x, inner.y + 1 + (i - v.offset) as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(files::shorten_name(&row.label, inner.width as usize)).style(
                if i == v.selected {
                    palette.selection()
                } else {
                    palette.text()
                },
            ),
            r,
        );
        controls.push((r, target(C::Row(i))));
    }
    if v.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if v.commands {
                "Нет подходящих команд"
            } else {
                "Нет установленных расширений"
            })
            .style(palette.muted()),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    let selected = v
        .rows
        .get(v.selected)
        .and_then(|r| app.extensions.catalog.get(r.plugin).map(|p| (r, p)));
    let description = selected
        .map(|(r, p)| {
            let title = r
                .command
                .map(|i| p.manifest.commands[i].name.as_str())
                .unwrap_or(&p.manifest.name);
            format!(
                "{title} · {}\n{}{}",
                p.manifest.version,
                p.manifest.description,
                p.error
                    .as_ref()
                    .map(|e| format!("\n{e}"))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| {
            if v.commands {
                "Команды появляются после включения подходящего расширения".into()
            } else {
                "Установить: выберите папку с basalgrim-plugin.json".into()
            }
        });
    let text = if app.extensions.status.is_empty() {
        description
    } else {
        format!("{}\n{description}", app.extensions.status)
    };
    let info = Rect::new(inner.x, inner.y + capacity as u16 + 2, inner.width, 3);
    v.detail_area = info;
    let paragraph = Paragraph::new(text)
        .style(palette.muted())
        .wrap(Wrap { trim: false });
    v.detail_offset = v.detail_offset.min(
        paragraph
            .line_count(inner.width)
            .saturating_sub(info.height as usize),
    );
    frame.render_widget(paragraph.scroll((v.detail_offset as u16, 0)), info);
    let last = inner.bottom();
    frame.render_widget(
        Paragraph::new(if v.commands {
            "Результат: PgUp/PgDn или колесо"
        } else {
            "Код работает с вашими правами"
        })
        .style(palette.muted()),
        Rect::new(inner.x, last - 4, inner.width, 1),
    );
    let mut button = |x: u16, y: u16, w: u16, label: &str, control: C, enabled: bool| {
        let r = Rect::new(inner.x + x, y, w.min(inner.width.saturating_sub(x)), 1);
        frame.render_widget(
            Paragraph::new(label.to_owned()).style(if enabled {
                palette.accent()
            } else {
                palette.muted()
            }),
            r,
        );
        // Even disabled controls own hover; their dispatch also checks state.
        controls.push((r, target(control)));
    };
    let idle = app.extensions.working.is_none();
    if app.extensions.working.is_some() {
        button(0, last - 3, 12, " Стоп F4 ", C::Stop, true);
    } else if v.commands {
        button(0, last - 3, 14, " Выполнить ", C::Run, selected.is_some());
    } else {
        button(
            0,
            last - 3,
            14,
            if selected.is_some_and(|(_, p)| p.enabled) {
                " Выключить "
            } else {
                " Включить "
            },
            C::Toggle,
            selected.is_some(),
        );
        button(15, last - 3, 12, " Удалить ", C::Remove, selected.is_some());
    }
    button(0, last - 2, 14, " Установить ", C::Install, idle);
    button(15, last - 2, 14, " Обновить F5 ", C::Refresh, idle);
    button(0, last - 1, 4, " ↑ ", C::Move(-1), true);
    button(4, last - 1, 4, " ↓ ", C::Move(1), true);
    button(10, last - 1, 8, "Текст ↑", C::Detail(-3), true);
    button(19, last - 1, 8, "Текст ↓", C::Detail(3), true);
    for (rect, t) in controls {
        hit(app, rect, t);
    }
}

use super::*;
use crate::{app::DeviceControl, devices::Kind};
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let width = area.width.min(76);
    let height = area.height.min(
        (app.devices_view.rows.len() as u16)
            .saturating_add(9)
            .clamp(12, 22),
    );
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
    let block = panel(palette, " Накопители ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.width < 28 || inner.height < 9 {
        return;
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
    let revision = app.devices_view.revision;
    if let Some(plan) = &app.devices_view.confirm {
        let label = plan
            .device
            .drive
            .as_ref()
            .map(|d| d.label.as_str())
            .unwrap_or(&plan.device.label);
        let mounted = plan.volumes.iter().filter(|v| !v.mounts.is_empty()).count();
        let label = files::shorten_name(label, inner.width.saturating_sub(11) as usize);
        frame.render_widget(
            Paragraph::new(format!("Извлечь «{label}»?\n\nОтключатся все его разделы.\nПодключено разделов: {mounted}\nЗатронуто накопителей: {}", plan.drives.len()))
                .style(palette.text()).wrap(Wrap { trim: true }),
            Rect::new(inner.x, inner.y, inner.width, inner.height - 2),
        );
        button(
            app,
            frame,
            palette,
            Rect::new(inner.x, inner.bottom() - 1, 12, 1),
            " Извлечь ",
            DeviceControl::Confirm,
            !app.busy && app.devices.working.is_none() && app.devices.ready,
        );
        button(
            app,
            frame,
            palette,
            Rect::new(inner.x + 14, inner.bottom() - 1, 10, 1),
            " Отмена ",
            DeviceControl::Back,
            true,
        );
        return;
    }
    let v = &mut app.devices_view;
    v.page = (inner.height - 6) as usize;
    if v.selected < v.offset {
        v.offset = v.selected;
    }
    if v.selected >= v.offset + v.page {
        v.offset = v.selected + 1 - v.page;
    }
    let mut hits = Vec::new();
    for (index, device) in v.rows.iter().enumerate().skip(v.offset).take(v.page) {
        let r = Rect::new(inner.x, inner.y + (index - v.offset) as u16, inner.width, 1);
        let status = device.state();
        let name_width = inner.width.saturating_sub(status.width() as u16 + 2) as usize;
        let label = files::shorten_name(&device.label, name_width);
        let padding = name_width.saturating_sub(label.width());
        frame.render_widget(
            Paragraph::new(format!(" {label}{} {status}", " ".repeat(padding))).style(
                if index == v.selected {
                    palette.selection()
                } else {
                    palette.text()
                },
            ),
            r,
        );
        hits.push((r, Target::DeviceRow { revision, index }));
    }
    let selected = v.rows.get(v.selected);
    let detail = selected
        .map(|d| {
            if let Some(v) = &d.volume {
                format!("{} · {} · {}", v.device.display(), fs::size(v.size), v.fs)
            } else {
                d.label.clone()
            }
        })
        .unwrap_or_else(|| {
            if app.devices.ready {
                "Нет доступных накопителей".into()
            } else {
                "Поиск накопителей…".into()
            }
        });
    let row = inner.bottom() - 6;
    frame.render_widget(
        Paragraph::new(files::shorten_name(
            &fs::clean(&detail),
            inner.width as usize,
        ))
        .style(palette.muted()),
        Rect::new(inner.x, row, inner.width, 1),
    );
    let mount = selected
        .and_then(|d| d.volume.as_ref())
        .and_then(|v| v.mounts.first())
        .map(|p| fs::clean(&p.to_string_lossy()))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(files::shorten_name(&mount, inner.width as usize)).style(palette.muted()),
        Rect::new(inner.x, row + 1, inner.width, 1),
    );
    let status = if app.devices.working.is_some() {
        "Выполняется действие…"
    } else if let Some(e) = &app.devices.error {
        e
    } else if app.notice_error {
        &app.notice
    } else {
        "Открыть — подключить и перейти"
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(if app.notice_error || app.devices.error.is_some() {
                palette.text()
            } else {
                palette.muted()
            })
            .wrap(Wrap { trim: true }),
        Rect::new(inner.x, row + 2, inner.width, 2),
    );
    let free = !app.busy && app.devices.working.is_none() && app.devices.ready;
    let available =
        |kind| free && selected.is_some_and(|d| app.devices.snapshot.plan(&d.key, kind).is_ok());
    let (open, mount, unmount, eject) = (
        available(Kind::Open),
        available(Kind::Mount) && !selected.is_some_and(|d| d.mounted()),
        available(Kind::Unmount),
        available(Kind::Eject),
    );
    let count = format!(
        " {} / {}",
        if v.rows.is_empty() { 0 } else { v.selected + 1 },
        v.rows.len()
    );
    for (r, t) in hits {
        hit(app, r, t);
    }
    for (x, y, label, control, enabled) in [
        (0, 4, " Открыть ", DeviceControl::Open, open),
        (9, 4, " Подключить ", DeviceControl::Mount, mount),
        (21, 4, " Отключить ", DeviceControl::Unmount, unmount),
        (0, 5, " Извлечь ", DeviceControl::Eject, eject),
        (
            9,
            5,
            " Обновить ",
            DeviceControl::Refresh,
            app.devices.working.is_none(),
        ),
        (19, 5, " ↑ ", DeviceControl::Up, true),
        (22, 5, " ↓ ", DeviceControl::Down, true),
    ] {
        button(
            app,
            frame,
            palette,
            Rect::new(inner.x + x, row + y, label.width() as u16, 1),
            label,
            control,
            enabled,
        );
    }
    frame.render_widget(
        Paragraph::new(count).style(palette.muted()),
        Rect::new(inner.x + 25, inner.bottom() - 1, inner.width - 25, 1),
    );
}
fn button(
    app: &mut App,
    frame: &mut Frame,
    palette: Palette,
    rect: Rect,
    label: &str,
    control: DeviceControl,
    enabled: bool,
) {
    frame.render_widget(
        Paragraph::new(label).style(if enabled {
            palette.accent()
        } else {
            palette.muted()
        }),
        rect,
    );
    hit(
        app,
        rect,
        Target::DeviceControl {
            revision: app.devices_view.revision,
            control,
        },
    );
}

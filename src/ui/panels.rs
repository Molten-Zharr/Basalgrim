use super::*;
use crate::app::{Dialog, PanelKind};
use ratatui::{style::Style, widgets::Clear};

pub fn buttons(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    places: bool,
    preview: bool,
    palette: Palette,
) {
    let mut x = area.right();
    for (show, label, kind) in [
        (preview, " Превью ", PanelKind::Preview),
        (places, " Места ", PanelKind::Places),
    ] {
        if !show {
            continue;
        }
        let w = label.width() as u16;
        x = x.saturating_sub(w);
        let r = Rect::new(x, area.y, w, 1);
        frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
        if kind == PanelKind::Places {
            app.bookmark_drop_area = r;
        }
        hit(app, r, Target::PanelOpen(kind));
    }
}
pub(super) fn quick_access_row(
    app: &mut App,
    frame: &mut Frame,
    area: Rect,
    label: &str,
    target: Target,
    style: Style,
    panel: Option<u64>,
) {
    let button = Rect::new(area.right() - 3, area.y, 3, 1);
    let link = Rect {
        width: area.width - button.width,
        ..area
    };
    let hovered = app.pointer.is_some_and(|point| area.contains(point))
        && !app.dragging_files()
        && !app.external_drag_active()
        && app.selection_area().is_none()
        && (panel.is_some() || app.dialog.is_none())
        && app.library.writable();
    let label_area = if hovered { link } else { area };
    frame.render_widget(
        Paragraph::new(files::shorten_name(label, usize::from(label_area.width))).style(style),
        label_area,
    );
    let unpin = if hovered {
        match &target {
            Target::Place(path) | Target::SavedPlace(path) => Some(Target::UnpinPlace {
                path: path.clone(),
                revision: app.library.revision,
                panel,
            }),
            _ => None,
        }
    } else {
        None
    };
    hit(
        app,
        label_area,
        match panel {
            Some(token) => Target::PanelLink {
                token,
                revision: app.library.revision,
                target: Box::new(target),
            },
            None => target,
        },
    );
    if let Some(target) = unpin {
        frame.render_widget(Paragraph::new(" × ").style(style), button);
        hit(app, button, target);
    }
}
pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::Panel(kind)) = app.dialog else {
        return;
    };
    let screen = frame.area();
    if screen.width < 8 || screen.height < 6 {
        return;
    }
    let token = app.design.panel_token;
    let width = screen
        .width
        .saturating_sub(2)
        .min(if kind == PanelKind::Places { 38 } else { 76 });
    let area = Rect::new(
        if kind == PanelKind::Places {
            screen.x + 1
        } else {
            screen.right() - width - 1
        },
        screen.y + 1,
        width,
        screen.height - 2,
    );
    app.icon_slots
        .retain(|slot| slot.area.intersection(area).is_empty());
    if kind == PanelKind::Preview
        || app
            .preview_slot
            .as_ref()
            .is_some_and(|s| !s.area.intersection(area).is_empty())
    {
        app.preview_slot = None;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(palette.text()), area);
    app.hits.clear();
    hit(app, screen, Target::DismissMenu);
    hit(app, area, Target::MenuBackground);
    if kind == PanelKind::Preview {
        super::preview(
            app,
            frame,
            Rect::new(area.x, area.y, area.width, area.height - 2),
            palette,
        );
        // Extra preview actions are ordinary shared commands, still protected by
        // modal dismissal. Wrap their renderer-owned regions like sidebar links.
        for (_, target) in &mut app.hits {
            if matches!(target, Target::FolderMeasure { .. }) {
                *target = Target::PanelLink {
                    token,
                    revision: app.library.revision,
                    target: Box::new(target.clone()),
                };
            }
        }
    } else {
        let block = panel(palette, " МЕСТА ", true);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        use crate::places::{PlaceSource, Section};
        let mut rows = vec![(
            Section::Bookmarks.label().to_owned(),
            Target::PlacesSection(Section::Bookmarks),
        )];
        rows.extend(app.quick_access_items().map(|i| {
            let i = i.into_owned();
            let target = if i.source == PlaceSource::Standard {
                Target::Place(i.path)
            } else {
                Target::SavedPlace(i.path)
            };
            (format!("  {}", i.label), target)
        }));
        rows.push(("Корзина".into(), Target::Action(Action::TrashView)));
        for section in [Section::Favorites, Section::Recent] {
            rows.push((section.label().into(), Target::PlacesSection(section)));
        }
        rows.push(("Накопители".into(), Target::Action(Action::Devices)));
        rows.push(("Сеть".into(), Target::Action(Action::Network)));
        for d in app.devices.snapshot.devices() {
            rows.push((
                d.label.clone(),
                Target::DeviceOpen {
                    revision: app.devices.revision,
                    key: d.key,
                },
            ));
        }
        let body = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(2),
        );
        let page = usize::from(body.height).max(1);
        app.design.panel_max = rows.len().saturating_sub(1);
        app.design.panel_selected = app.design.panel_selected.min(app.design.panel_max);
        if app.design.panel_selected < app.design.panel_offset {
            app.design.panel_offset = app.design.panel_selected;
        }
        if app.design.panel_selected >= app.design.panel_offset + page {
            app.design.panel_offset = app.design.panel_selected + 1 - page;
        }
        for (i, (label, target)) in rows
            .into_iter()
            .enumerate()
            .skip(app.design.panel_offset)
            .take(page)
        {
            let r = Rect::new(
                body.x,
                body.y + (i - app.design.panel_offset) as u16,
                body.width,
                1,
            );
            let style = if i == app.design.panel_selected {
                palette.selection()
            } else {
                palette.text()
            };
            if matches!(target, Target::SavedPlace(_) | Target::Place(_)) {
                quick_access_row(app, frame, r, &label, target, style, Some(token));
                continue;
            }
            frame.render_widget(
                Paragraph::new(files::shorten_name(&label, r.width as usize)).style(style),
                r,
            );
            hit(
                app,
                r,
                Target::PanelLink {
                    revision: app.library.revision,
                    token,
                    target: Box::new(target),
                },
            );
        }
    }
    for (x, label, delta) in [(area.x + 1, " ↑ ", -1), (area.x + 5, " ↓ ", 1)] {
        let r = Rect::new(x, area.bottom() - 2, 3, 1);
        frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
        hit(app, r, Target::PanelScroll { token, delta });
    }
    let r = Rect::new(area.right() - 4, area.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), r);
    hit(app, r, Target::PanelClose(token));
}

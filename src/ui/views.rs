use super::*;
use crate::{app::Dialog, view::ViewMode};
use ratatui::widgets::{Clear, Wrap};
pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::View(menu)) = &app.dialog else {
        return;
    };
    let token = menu.token;
    let selected = menu.selected;
    let screen = frame.area();
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(50) / 2,
        screen.y + screen.height.saturating_sub(12) / 2,
        screen.width.min(50),
        screen.height.min(12),
    );
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
    let block = panel(palette, " Вид файлов ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 || inner.height < 8 {
        return;
    }
    for (i, mode) in ViewMode::ALL.into_iter().enumerate() {
        let r = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                " {} {}",
                if mode == app.settings.view_mode() {
                    "●"
                } else {
                    "○"
                },
                mode.name()
            ))
            .style(if selected == i {
                palette.selection()
            } else {
                palette.text()
            }),
            r,
        );
        hit(app, r, Target::ViewChoose { token, mode });
    }
    let y = inner.y + 5;
    let label = format!("Столбцов: {}", app.settings.view_columns);
    frame.render_widget(
        Paragraph::new(label).style(palette.muted()),
        Rect::new(inner.x, y, inner.width.saturating_sub(10), 1),
    );
    for (x, label, delta) in [
        (inner.right() - 9, " − ", -1),
        (inner.right() - 4, " + ", 1),
    ] {
        let r = Rect::new(x, y, 3, 1);
        frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
        hit(app, r, Target::ViewColumns { token, delta });
    }
    frame.render_widget(
        Paragraph::new(ViewMode::ALL[selected].description())
            .wrap(Wrap { trim: false })
            .style(palette.muted()),
        Rect::new(
            inner.x,
            inner.y + 7,
            inner.width,
            inner.height.saturating_sub(7),
        ),
    );
    let close = Rect::new(area.right().saturating_sub(4), area.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
}

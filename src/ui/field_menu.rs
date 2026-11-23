use super::*;
use crate::app::Dialog;
use ratatui::widgets::Clear;
pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::FieldMenu(m)) = &app.dialog else {
        return;
    };
    let token = m.token;
    let selected = m.selected;
    let title = m.title;
    let screen = frame.area();
    let w = 34.min(screen.width);
    let h = (m.rows.len() as u16 + 2).min(screen.height);
    let rect = Rect::new(
        m.anchor.x.min(screen.right() - w),
        m.anchor.y.min(screen.bottom() - h),
        w,
        h,
    );
    app.hits.clear();
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|s| !s.area.intersection(rect).is_empty())
    {
        app.preview_slot = None;
    }
    hit(app, screen, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    frame.render_widget(Clear, rect);
    let block = panel(palette, format!(" {title} "), true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let Some(Dialog::FieldMenu(menu)) = &app.dialog else {
        return;
    };
    for (i, option) in menu.rows.iter().enumerate().take(usize::from(inner.height)) {
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let enabled = option.enabled();
        let marker = match option.checked {
            Some(true) => "●",
            Some(false) => "○",
            None => "↕",
        };
        let label = &option.label;
        frame.render_widget(
            Paragraph::new(format!(
                " {marker} {}",
                files::shorten_name(label, usize::from(row.width.saturating_sub(3)))
            ))
            .style(if !enabled {
                palette.muted()
            } else if i == selected {
                palette.selection()
            } else {
                palette.text()
            }),
            row,
        );
        if enabled {
            app.hits.push((row, Target::FieldMenuRow { token, row: i }));
        }
    }
    let close = Rect::new(rect.right().saturating_sub(4), rect.y, 3.min(w), 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
}

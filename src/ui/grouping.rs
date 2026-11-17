use super::*;
use crate::{app::Dialog, columns::Column};
use ratatui::widgets::Clear;
pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::Grouping(m)) = &app.dialog else {
        return;
    };
    let token = m.token;
    let selected = m.selected;
    let policy = app.settings.grouping;
    let screen = frame.area();
    let w = 34.min(screen.width);
    let h = (if policy.enabled { 14 } else { 3 }).min(screen.height);
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
    let block = panel(palette, " Группировка ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut labels = vec![format!(
        "{} Группировка",
        if policy.enabled { "√" } else { "□" }
    )];
    if policy.enabled {
        labels.push(format!(
            "{} {}",
            if policy.descending { "↓" } else { "↑" },
            policy.order_label()
        ));
        labels.extend(Column::ALL.map(|c| {
            format!(
                "{} {}{}",
                if c == policy.field { "●" } else { "○" },
                c.label(),
                if matches!(c, Column::Created | Column::Modified | Column::Accessed) {
                    " · месяц"
                } else {
                    ""
                }
            )
        }));
    }
    for (i, label) in labels
        .into_iter()
        .enumerate()
        .take(usize::from(inner.height))
    {
        let row = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                " {}",
                files::shorten_name(&label, usize::from(row.width.saturating_sub(1)))
            ))
            .style(if i == selected {
                palette.selection()
            } else {
                palette.text()
            }),
            row,
        );
        hit(app, row, Target::GroupControl { token, row: i });
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
}

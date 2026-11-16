use super::*;
use crate::{app::Dialog, columns::Column};
use ratatui::widgets::Clear;
pub fn menu(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::Columns(menu)) = &app.dialog else {
        return;
    };
    let token = menu.token;
    let selected = menu.selected;
    let revision = menu.context.revision;
    let screen = frame.area();
    let w = 32.min(screen.width);
    let h = 13.min(screen.height);
    let area = Rect::new(
        menu.anchor.x.min(screen.right() - w),
        menu.anchor.y.min(screen.bottom() - h),
        w,
        h,
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
    let block = panel(palette, " Колонки ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    for (i, column) in Column::ALL
        .into_iter()
        .enumerate()
        .take(usize::from(inner.height))
    {
        let enabled = app
            .settings
            .list_columns
            .columns
            .iter()
            .any(|s| s.column == column);
        let r = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                " {} {}",
                if enabled { "√" } else { "□" },
                column.label()
            ))
            .style(if selected == i {
                palette.selection()
            } else {
                palette.text()
            }),
            r,
        );
        hit(
            app,
            r,
            Target::ColumnToggle {
                token,
                revision,
                column,
            },
        );
    }
    let close = Rect::new(area.right() - 4, area.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
    hit(app, close, Target::Cancel);
    if inner.height > 10 {
        frame.render_widget(
            Paragraph::new("Имя всегда включено").style(palette.muted()),
            Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
        );
    }
}

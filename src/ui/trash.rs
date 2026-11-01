//! Trash supplies location-specific controls; file panes own all other chrome and geometry.
use super::*;

pub fn controls(app: &mut App, frame: &mut Frame, area: Rect, palette: Palette) {
    if area.is_empty() {
        return;
    }
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Block::default().style(palette.text()), area);
    let count = app.trash.as_ref().unwrap().selected_entries().count();
    let can_restore = app.trash.as_ref().unwrap().can_restore() && !app.busy;
    let label = if count > 1 {
        format!(" Восстановить ({count}) ")
    } else {
        " Восстановить ".into()
    };
    let width = (label.width() as u16).min(area.width);
    let button = Rect::new(area.x, area.y, width, 1);
    frame.render_widget(
        Paragraph::new(label).style(if can_restore {
            palette.accent()
        } else {
            palette.muted()
        }),
        button,
    );
    if can_restore {
        hit(app, button, Target::Action(Action::RestoreTrash));
    }
    let delete_width = 10u16;
    if area.width > width + delete_width {
        let rect = Rect::new(button.right(), area.y, delete_width, 1);
        let can_delete = app.trash.as_ref().unwrap().can_delete() && !app.busy;
        frame.render_widget(
            Paragraph::new(" Удалить ").style(if can_delete {
                palette.accent()
            } else {
                palette.muted()
            }),
            rect,
        );
        if can_delete {
            hit(app, rect, Target::Action(Action::DeleteTrash));
        }
        if rect.right() < area.right() {
            let view = app.trash.as_ref().unwrap();
            let filter = if view.filter.is_empty() {
                String::new()
            } else {
                format!(" · фильтр: {}", fs::clean(&view.filter))
            };
            frame.render_widget(
                Paragraph::new(format!(" · {} элементов{filter}", view.visible.len()))
                    .style(palette.muted()),
                Rect::new(rect.right(), area.y, area.right() - rect.right(), 1),
            );
        }
    }
}
pub fn details(app: &App) -> String {
    let Some(view) = &app.trash else {
        return String::new();
    };
    if view.explicit_selection || !view.marked.is_empty() {
        return format!(" Выбрано: {}", view.marked.len());
    }
    if let Some(entry) = view.entry() {
        if let Some(issue) = &entry.issue {
            return format!(" {}", fs::clean(issue));
        }
        return entry
            .original
            .as_ref()
            .map(|p| format!(" Был: {}", fs::clean(&p.to_string_lossy())))
            .unwrap_or_default();
    }
    " Корзина пуста".into()
}

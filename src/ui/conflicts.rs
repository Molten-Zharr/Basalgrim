use super::*;
use crate::{
    app::Dialog,
    conflicts::{Choice, ItemInfo},
};
use ratatui::widgets::Clear;

fn describe(info: &ItemInfo) -> String {
    format!(
        "{}{}",
        info.kind,
        info.bytes
            .map_or(String::new(), |n| format!(" · {}", fs::size(n)))
    )
}
fn clipped(text: &str, width: u16) -> String {
    super::files::shorten_name(&fs::clean(text), usize::from(width))
}

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let Some(Dialog::Conflict(current)) = &app.dialog else {
        return;
    };
    let area = frame.area();
    frame.render_widget(Block::default().style(palette.text()), area);
    app.icon_slots.clear();
    app.hits.clear();
    let rect = Rect::new(
        area.x + area.width.saturating_sub(72) / 2,
        area.y + area.height.saturating_sub(16) / 2,
        area.width.min(72),
        area.height.min(16),
    );
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Совпадение имён ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 7 || inner.width < 22 {
        frame.render_widget(Paragraph::new("Увеличьте окно"), inner);
        if inner.height > 0 {
            let close = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
            frame.render_widget(
                Paragraph::new(" Esc Отменить ").style(palette.accent()),
                close,
            );
            hit(app, close, Target::Cancel);
        }
        return;
    }
    let details_height = inner.height.saturating_sub(7);
    let mut lines = Vec::new();
    if details_height >= 6 {
        lines.push(Line::raw(clipped(
            &format!(" Из: {}", current.info.source.display()),
            inner.width,
        )));
        lines.push(Line::styled(
            format!(
                " {} · {}",
                describe(&current.info.incoming),
                fs::date(current.info.incoming.modified)
            ),
            palette.muted(),
        ));
        lines.push(Line::raw(clipped(
            &format!(" В:  {}", current.info.destination.display()),
            inner.width,
        )));
        lines.push(Line::styled(
            format!(
                " {} · {}",
                describe(&current.info.existing),
                fs::date(current.info.existing.modified)
            ),
            palette.muted(),
        ));
    } else {
        let name = current
            .info
            .destination
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        lines.push(Line::raw(clipped(&format!(" {name}"), inner.width)));
        lines.push(Line::styled(
            clipped(
                &format!(" Новое: {}", describe(&current.info.incoming)),
                inner.width,
            ),
            palette.muted(),
        ));
        lines.push(Line::styled(
            clipped(
                &format!(" Цель: {}", describe(&current.info.existing)),
                inner.width,
            ),
            palette.muted(),
        ));
    }
    let warning = if !current.info.can_replace {
        " Один объект: замена недоступна."
    } else if current.info.existing.kind == "Папка" {
        " Замена всей папки, без слияния."
    } else if current.info.existing.kind == "Ссылка" {
        " Заменяется ссылка, не её цель."
    } else {
        " Прежняя цель будет удалена."
    };
    lines.push(Line::styled(
        clipped(warning, inner.width),
        palette.accent(),
    ));
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x, inner.y, inner.width, details_height),
    );
    let base_y = inner.bottom() - 7;
    let remaining = if current.info.remaining > 0 {
        format!(
            " [{}] Для оставшихся ({})",
            if current.remaining { 'x' } else { ' ' },
            current.info.remaining
        )
    } else {
        " Последний элемент пакета".into()
    };
    let style = if current.selected == 4 {
        palette.selection()
    } else {
        palette.text()
    };
    let row = Rect::new(inner.x, base_y, inner.width, 1);
    frame.render_widget(
        Paragraph::new(clipped(&remaining, inner.width)).style(style),
        row,
    );
    let has_remaining = current.info.remaining > 0;
    let selected = current.selected;
    let allowed = Choice::ALL.map(|c| current.info.allows(c));
    if has_remaining {
        hit(app, row, Target::ConflictRemaining);
    }
    for (index, choice) in Choice::ALL.into_iter().enumerate() {
        let row = Rect::new(inner.x, base_y + 2 + index as u16, inner.width, 1);
        let style = if !allowed[index] {
            palette.muted()
        } else if selected == index {
            palette.selection()
        } else {
            palette.accent()
        };
        frame.render_widget(
            Paragraph::new(format!(" {}", choice.label())).style(style),
            row,
        );
        hit(app, row, Target::ConflictChoice(choice));
    }
    let help = if has_remaining {
        " ↑↓/Tab · Enter · Пробел — всем"
    } else {
        " ↑↓/Tab — выбор · Enter · Esc"
    };
    frame.render_widget(
        Paragraph::new(clipped(help, inner.width)).style(palette.muted()),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
    if rect.width >= 4 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
        hit(app, close, Target::Cancel);
    }
}

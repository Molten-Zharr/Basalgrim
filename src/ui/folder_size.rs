use super::*;
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let rect = Rect::new(
        area.x + area.width.saturating_sub(68) / 2,
        area.y + area.height.saturating_sub(20) / 2,
        area.width.min(68),
        area.height.min(20),
    );
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    app.preview_slot = None;
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Размер папки ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.height < 6 {
        return;
    }
    let result = &app.folder_sizes.result;
    let path = app
        .folder_sizes
        .path
        .as_ref()
        .map(|p| fs::clean(&p.to_string_lossy()))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(files::shorten_name(&path, inner.width as usize)).style(palette.muted()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(result.status()).style(palette.accent()),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    let approximate = if !result.complete() { "~ " } else { "" };
    let mut lines = vec![
        Line::raw(format!(
            "Размер файлов: {approximate}{}",
            fs::size(result.bytes)
        )),
        Line::raw(format!(
            "На диске: {approximate}{}",
            fs::size(result.allocated)
        )),
        Line::raw(format!(
            "Файлов: {} · папок: {}",
            result.files, result.folders
        )),
        Line::raw(format!(
            "Ссылок: {} · прочих: {}",
            result.links, result.other
        )),
        Line::raw(format!("Повторных ссылок: {}", result.duplicates)),
        Line::raw(format!("Пропуски: {}", result.skipped)),
    ];
    if let Some(time) = result.measured {
        let time: chrono::DateTime<chrono::Local> = time.into();
        lines.push(Line::raw(format!("Снимок: {}", time.format("%H:%M:%S"))));
    }
    if let Some(error) = &result.failure {
        lines.push(Line::styled(
            fs::clean(error),
            palette.text().fg(palette.color("error")),
        ));
    }
    lines.extend(result.issues.iter().map(|s| Line::raw(s.clone())));
    lines.push(Line::styled(
        "Скрытые файлы включены. Ссылки не обходятся; одинаковые данные считаются один раз.",
        palette.muted(),
    ));
    if result.cached {
        lines.push(Line::styled(
            "Кэш до 30 секунд. Пересчитайте для свежего результата.",
            palette.muted(),
        ));
    }
    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 5);
    let paragraph = Paragraph::new(lines)
        .style(palette.text())
        .wrap(Wrap { trim: false });
    app.folder_size_max = paragraph
        .line_count(body.width)
        .saturating_sub(body.height as usize);
    app.folder_size_offset = app
        .folder_size_offset
        .min(app.folder_size_max)
        .min(u16::MAX as usize);
    frame.render_widget(paragraph.scroll((app.folder_size_offset as u16, 0)), body);
    let id = app.folder_sizes.id;
    let stop = (!app.folder_sizes.result.done).then_some(Target::FolderStop(id));
    let rows: [(String, Option<Target>, u16); 3] = [
        (" ↑ ".into(), Some(Target::FolderScroll(-3)), 3),
        (" ↓ ".into(), Some(Target::FolderScroll(3)), 3),
        (
            if stop.is_some() {
                " Стоп · пробел ".into()
            } else {
                "".into()
            },
            stop,
            inner.width.saturating_sub(6),
        ),
    ];
    let mut x = inner.x;
    for (label, target, width) in rows {
        let r = Rect::new(x, inner.bottom() - 3, width, 1);
        x += width;
        frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
        if let Some(target) = target {
            hit(app, r, target);
        }
    }
    for (y, label, target) in [
        (
            inner.bottom() - 2,
            format!(" Пересчитать · {}", app.settings.shortcut(Action::Refresh)),
            Target::FolderRetry(id),
        ),
        (inner.bottom() - 1, " Закрыть · Esc ".into(), Target::Cancel),
    ] {
        let r = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(Paragraph::new(label).style(palette.accent()), r);
        hit(app, r, target);
    }
}

use super::*;
use ratatui::widgets::Clear;
pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    let area = frame.area();
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(96)) / 2,
        area.y + (area.height.saturating_sub(28)) / 2,
        area.width.min(96),
        area.height.min(28),
    );
    app.icon_slots
        .retain(|s| s.area.intersection(rect).is_empty());
    app.preview_slot = None;
    frame.render_widget(Clear, rect);
    let block = panel(palette, " Поиск в подпапках ", true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.height < 9 || inner.width < 28 {
        return;
    }
    let root = app
        .search
        .as_ref()
        .map(|s| fs::clean(&s.owner.cwd.to_string_lossy()))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(root).style(palette.muted()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    super::text_edit::field(
        app,
        frame,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
        palette,
    );
    let Some(view) = &mut app.search else {
        return;
    };
    view.page = usize::from(inner.height - 7);
    view.selected = view
        .selected
        .min(view.worker.results.total.saturating_sub(1));
    if view.selected < view.offset {
        view.offset = view.selected;
    }
    if view.selected >= view.offset + view.page {
        view.offset = view.selected + 1 - view.page;
    }
    view.worker.request_page(view.offset);
    let result = &view.worker.results;
    let status = if view.query.trim().is_empty() {
        "Введите часть имени".into()
    } else if inner.width < 60 {
        format!("{} · найдено {}", result.status(), result.total)
    } else {
        format!(
            "{} · {} найдено · {} проверено",
            result.status(),
            result.total,
            result.scanned
        )
    };
    frame.render_widget(
        Paragraph::new(status).style(palette.text()),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
    let details = result.failure.clone().unwrap_or_else(|| {
        format!(
            "Ошибок: {} · скрытые {}",
            result.errors,
            if app.settings.hidden {
                "включены"
            } else {
                "исключены"
            }
        )
    });
    frame.render_widget(
        Paragraph::new(details).style(palette.muted()),
        Rect::new(inner.x, inner.y + 3, inner.width, 1),
    );
    let mut hits = Vec::new();
    for (index, path) in result
        .paths
        .iter()
        .enumerate()
        .map(|(index, path)| (result.offset + index, path))
        .filter(|(index, _)| *index >= view.offset && *index < view.offset + view.page)
    {
        let line = Rect::new(
            inner.x,
            inner.y + 4 + (index - view.offset) as u16,
            inner.width,
            1,
        );
        let text = fs::clean(
            &path
                .strip_prefix(&view.owner.cwd)
                .unwrap_or(path)
                .to_string_lossy(),
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{} {}",
                if index == view.selected { ">" } else { " " },
                super::files::shorten_name(&text, usize::from(inner.width.saturating_sub(2)))
            ))
            .style(if index == view.selected {
                palette.accent()
            } else {
                palette.text()
            }),
            line,
        );
        hits.push((line, Target::SearchOpen(view.worker.id, index)));
    }
    let index = view.selected;
    let id = view.worker.id;
    let page = view.page as isize;
    app.hits.extend(hits);
    let retry = Rect::new(inner.x + 16, inner.bottom() - 3, inner.width - 16, 1);
    frame.render_widget(
        Paragraph::new("Повторить F5").style(palette.accent()),
        retry,
    );
    hit(app, retry, Target::SearchRetry);
    for (x, y, w, text, target) in [
        (
            inner.x,
            inner.bottom() - 3,
            8,
            " ↑ ",
            Target::SearchMove(-page),
        ),
        (
            inner.x + 8,
            inner.bottom() - 3,
            8,
            " ↓ ",
            Target::SearchMove(page),
        ),
        (
            inner.x,
            inner.bottom() - 2,
            19,
            " Перейти · Enter",
            Target::SearchOpen(id, index),
        ),
        (
            inner.x + 19,
            inner.bottom() - 2,
            inner.width - 19,
            " В конец",
            Target::SearchMove(isize::MAX),
        ),
        (inner.x, inner.bottom() - 1, 14, " Стоп", Target::SearchStop),
        (
            inner.x + 14,
            inner.bottom() - 1,
            inner.width - 14,
            " Закрыть · Esc",
            Target::Cancel,
        ),
    ] {
        let r = Rect::new(x, y, w, 1);
        frame.render_widget(Paragraph::new(text).style(palette.accent()), r);
        hit(app, r, target);
    }
    super::text_edit::menu(app, frame, palette);
}

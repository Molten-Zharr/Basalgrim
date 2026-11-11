use super::*;
use crate::app::{DetailControl as C, Target};
use ratatui::widgets::{Clear, Wrap};
fn button(app: &mut App, frame: &mut Frame, p: Palette, r: Rect, label: &str, c: C, enabled: bool) {
    let view = app.detail_view.as_ref().unwrap();
    let selected = matches!(c, C::General) && !view.permissions
        || matches!(c, C::Permissions) && view.permissions;
    frame.render_widget(
        Paragraph::new(label).style(if selected {
            p.selection()
        } else if enabled {
            p.accent()
        } else {
            p.muted()
        }),
        r,
    );
    hit(
        app,
        r,
        Target::Detail {
            token: app.detail_view.as_ref().unwrap().token,
            revision: app.detail_view.as_ref().unwrap().revision,
            control: c,
        },
    );
}
fn date(value: Option<std::time::SystemTime>) -> String {
    value
        .map(|v| {
            chrono::DateTime::<chrono::Local>::from(v)
                .format("%d.%m.%Y %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".into())
}
pub fn render(app: &mut App, frame: &mut Frame, p: Palette) {
    app.filter_applications();
    let Some(v) = &app.detail_view else {
        return;
    };
    let chooser = v.chooser;
    let name = fs::clean(
        &v.source
            .path
            .file_name()
            .unwrap_or(v.source.path.as_os_str())
            .to_string_lossy(),
    );

    let area = frame.area();
    let width = area.width.min(76);
    let height = area.height.min(22);
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
    let block = panel(
        p,
        if chooser {
            " Открыть с помощью "
        } else {
            " Свойства "
        },
        true,
    );
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    app.hits.clear();
    hit(app, area, Target::DismissMenu);
    hit(app, rect, Target::MenuBackground);
    if inner.width < 30 || inner.height < 10 {
        return;
    }
    let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
    frame.render_widget(Paragraph::new(" × ").style(p.accent()), close);
    hit(app, close, Target::Cancel);
    frame.render_widget(
        Paragraph::new(files::shorten_name(&name, inner.width as usize))
            .style(p.text().add_modifier(Modifier::BOLD)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let body = Rect::new(inner.x, inner.y + 3, inner.width, inner.height - 6);
    if chooser {
        frame.render_widget(
            Paragraph::new("Поиск: ").style(p.muted()),
            Rect::new(inner.x, inner.y + 1, 7, 1),
        );
        super::text_edit::field(
            app,
            frame,
            Rect::new(inner.x + 7, inner.y + 1, inner.width - 7, 1),
            p,
        );
        let v = app.detail_view.as_mut().unwrap();
        v.page = (body.height - 1) as usize;
        if v.selected < v.offset {
            v.offset = v.selected;
        }
        if v.selected >= v.offset + v.page {
            v.offset = v.selected + 1 - v.page;
        }
        let rows: Vec<_> = v
            .visible
            .iter()
            .enumerate()
            .skip(v.offset)
            .take(v.page)
            .map(|(row, &i)| (row, v.applications[i].clone()))
            .collect();
        let selected = v.selected;
        for (line, (row, a)) in rows.into_iter().enumerate() {
            let r = Rect::new(body.x, body.y + line as u16, body.width, 1);
            let name = if a.default {
                format!("{} · по умолчанию", a.name)
            } else {
                a.name
            };
            frame.render_widget(
                Paragraph::new(files::shorten_name(&name, r.width as usize)).style(
                    if row == selected {
                        p.selection()
                    } else {
                        p.text()
                    },
                ),
                r,
            );
            hit(
                app,
                r,
                Target::Detail {
                    token: app.detail_view.as_ref().unwrap().token,
                    revision: app.detail_view.as_ref().unwrap().revision,
                    control: C::Row(row),
                },
            );
        }
        let v = app.detail_view.as_ref().unwrap();
        if v.visible.is_empty() && !v.loading && v.error.is_none() {
            frame.render_widget(
                Paragraph::new("Нет программ. Выберите «Все». ")
                    .style(p.muted())
                    .wrap(Wrap { trim: true }),
                body,
            );
        }
        if let Some(a) = v.visible.get(v.selected).map(|&i| &v.applications[i]) {
            frame.render_widget(
                Paragraph::new(files::shorten_name(&a.description, body.width as usize))
                    .style(p.muted()),
                Rect::new(body.x, body.bottom() - 1, body.width, 1),
            );
        }
        let launch = !v.loading && !v.visible.is_empty();
        let all = v.all;
        button(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 2, 11, 1),
            " Открыть ",
            C::Launch,
            launch,
        );
        button(
            app,
            frame,
            p,
            Rect::new(inner.x + 11, inner.bottom() - 2, inner.width - 11, 1),
            if all {
                " Рекомендуемые F4 "
            } else {
                " Все программы F4 "
            },
            C::All,
            true,
        );
    } else {
        button(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.y + 1, 13, 1),
            " Сведения ",
            C::General,
            true,
        );
        button(
            app,
            frame,
            p,
            Rect::new(inner.x + 14, inner.y + 1, 14, 1),
            " Права F4 ",
            C::Permissions,
            true,
        );
        let v = app.detail_view.as_ref().unwrap();
        if let Some(props) = &v.properties {
            if v.permissions && props.editable() {
                let mode = v.mode;
                let before = props.mode;
                let focus = v.bit;
                let editable = !v.loading && v.error.is_none() && !app.busy;
                let last = if props.kind == "Папка" {
                    "Вход"
                } else {
                    "Запуск"
                };
                for (col, label) in ["Чтение", "Запись", last].iter().enumerate() {
                    frame.render_widget(
                        Paragraph::new(*label).style(p.muted()),
                        Rect::new(body.x + 10 + col as u16 * 7, body.y, 7, 1),
                    );
                }
                for (row, label) in ["Владелец", "Группа", "Остальные"].iter().enumerate()
                {
                    frame.render_widget(
                        Paragraph::new(*label).style(p.text()),
                        Rect::new(body.x, body.y + 1 + row as u16, 10, 1),
                    );
                    for col in 0..3 {
                        let index = row * 3 + col;
                        let bit = 0o400 >> index;
                        let r =
                            Rect::new(body.x + 10 + col as u16 * 7, body.y + 1 + row as u16, 7, 1);
                        frame.render_widget(
                            Paragraph::new(if mode & bit != 0 { " [x] " } else { " [ ] " }).style(
                                if focus == index {
                                    p.selection()
                                } else if editable {
                                    p.accent()
                                } else {
                                    p.muted()
                                },
                            ),
                            r,
                        );
                        hit(
                            app,
                            r,
                            Target::Detail {
                                token: app.detail_view.as_ref().unwrap().token,
                                revision: app.detail_view.as_ref().unwrap().revision,
                                control: C::Toggle(bit),
                            },
                        );
                    }
                }
                frame.render_widget(
                    Paragraph::new(format!(
                        "{:04o} → {:04o} · Tab / пробел",
                        before & 0o7777,
                        (before & 0o7000) | mode
                    ))
                    .style(p.muted()),
                    Rect::new(body.x, body.y + 4, body.width, 1),
                );
                frame.render_widget(
                    Paragraph::new("Только этот объект, без подпапок").style(p.muted()),
                    Rect::new(body.x, body.y + 5, body.width, 1),
                );
                if body.height > 6 && before & 0o7000 != 0 {
                    frame.render_widget(
                        Paragraph::new(format!("Особые биты {:04o} сохраняются", before & 0o7000))
                            .style(p.muted()),
                        Rect::new(body.x, body.y + 6, body.width, 1),
                    );
                }
                button(
                    app,
                    frame,
                    p,
                    Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
                    " Сохранить · Ctrl+S ",
                    C::Apply,
                    editable && mode != before & 0o777,
                );
            } else {
                let mut lines = vec![
                    format!("Имя: {name}"),
                    format!("Путь: {}", fs::clean(&props.source.path.to_string_lossy())),
                    format!("Тип: {} · {}", props.kind, props.mime),
                    format!(
                        "{}: {} ({} байт)",
                        if props.kind == "Папка" {
                            "Запись папки"
                        } else {
                            "Размер"
                        },
                        fs::size(props.size),
                        props.size
                    ),
                    format!("Изменён: {}", date(props.modified)),
                ];
                if let Some(bytes) = props.allocated {
                    lines.push(format!("На диске: {}", fs::size(bytes)));
                }
                if let Some(i) = &props.identity {
                    lines.push(format!("Владелец UID: {} · группа GID: {}", i.uid, i.gid));
                }
                lines.push(format!("Права: {:04o}", props.mode & 0o7777));
                if let Some(link) = &props.link {
                    lines.push(format!("Ссылка на: {}", fs::clean(&link.to_string_lossy())));
                }
                if !props.editable() {
                    lines.push("Права этого объекта не изменяются".into());
                }
                let paragraph = Paragraph::new(lines.join("\n"))
                    .style(p.text())
                    .wrap(Wrap { trim: false });
                let v = app.detail_view.as_mut().unwrap();
                v.max_scroll = paragraph
                    .line_count(body.width)
                    .saturating_sub(body.height as usize);
                v.scroll = v.scroll.min(v.max_scroll);
                frame.render_widget(
                    paragraph.scroll((v.scroll.min(u16::MAX as usize) as u16, 0)),
                    body,
                );
                button(
                    app,
                    frame,
                    p,
                    Rect::new(inner.x, inner.bottom() - 2, 12, 1),
                    " Коп. путь ",
                    C::CopyPath,
                    true,
                );
                if app
                    .detail_view
                    .as_ref()
                    .unwrap()
                    .properties
                    .as_ref()
                    .is_some_and(|p| p.link.is_none() && p.kind.starts_with("Файл"))
                {
                    button(
                        app,
                        frame,
                        p,
                        Rect::new(inner.x + 13, inner.bottom() - 2, inner.width - 13, 1),
                        " Открыть с… ",
                        C::OpenWith,
                        true,
                    );
                }
            }
        }
    }
    let v = app.detail_view.as_mut().unwrap();
    if v.loading {
        frame.render_widget(Clear, body);
        frame.render_widget(Paragraph::new("Чтение…").style(p.muted()), body);
    }
    if let Some(error) = &v.error {
        let paragraph = Paragraph::new(error.lines().map(fs::clean).collect::<Vec<_>>().join("\n"))
            .style(p.text())
            .wrap(Wrap { trim: true });
        v.max_scroll = paragraph
            .line_count(body.width)
            .saturating_sub(body.height as usize);
        v.scroll = v.scroll.min(v.max_scroll);
        frame.render_widget(Clear, body);
        frame.render_widget(
            paragraph.scroll((v.scroll.min(u16::MAX as usize) as u16, 0)),
            body,
        );
    }
    button(
        app,
        frame,
        p,
        Rect::new(inner.x, inner.bottom() - 1, 3, 1),
        " ↑ ",
        C::Scroll(-3),
        true,
    );
    button(
        app,
        frame,
        p,
        Rect::new(inner.x + 3, inner.bottom() - 1, 3, 1),
        " ↓ ",
        C::Scroll(3),
        true,
    );
    button(
        app,
        frame,
        p,
        Rect::new(inner.x + 7, inner.bottom() - 1, 11, 1),
        " Обновить ",
        C::Refresh,
        true,
    );
    let close = Rect::new(inner.right() - 11, inner.bottom() - 1, 11, 1);
    frame.render_widget(Paragraph::new(" Закрыть ").style(p.accent()), close);
    hit(app, close, Target::Cancel);
    if chooser {
        super::text_edit::menu(app, frame, p);
    }
}

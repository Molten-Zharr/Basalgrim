use super::*;
use crate::{
    app::{Dialog, NetworkControl as C, NetworkField as F},
    network::Prompt,
};
use ratatui::widgets::{Clear, Wrap};

fn control(app: &mut App, frame: &mut Frame, p: Palette, r: Rect, label: &str, c: C) {
    frame.render_widget(
        Paragraph::new(fs::clean(label))
            .style(p.accent())
            .wrap(Wrap { trim: true }),
        r,
    );
    hit(
        app,
        r,
        Target::Network {
            revision: app.network_view.revision,
            control: c,
        },
    );
}
pub fn render(app: &mut App, frame: &mut Frame, p: Palette) {
    let area = frame.area();
    let width = area.width.min(72);
    let height = area.height.min(18);
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
    let block = panel(p, " Сеть ", true);
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
    let current = match &app.dialog {
        Some(Dialog::Edit {
            kind: crate::app::EditKind::Network(field),
            ..
        }) => Some(*field),
        _ => None,
    };
    if let Some(location) = &app.network_view.confirm {
        frame.render_widget(
            Paragraph::new(format!(
                "Отключить сетевую папку?\n\n{}\n\nОна может использоваться другими программами.",
                location.uri
            ))
            .style(p.text())
            .wrap(Wrap { trim: true }),
            Rect::new(inner.x, inner.y, inner.width, inner.height - 2),
        );
        control(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 1, 13, 1),
            " Отключить ",
            C::ConfirmDisconnect,
        );
        let cancel = Rect::new(inner.x + 15, inner.bottom() - 1, 10, 1);
        frame.render_widget(Paragraph::new(" Отмена ").style(p.accent()), cancel);
        hit(app, cancel, Target::Cancel);
        return;
    }
    if let Some((_, Prompt::Question { message, choices })) = &app.network_view.prompt {
        let choices = choices.clone();
        let message = message.clone();
        let height = inner.height - 7;
        let paragraph = Paragraph::new(message)
            .style(p.text())
            .wrap(Wrap { trim: true });
        app.network_view.scroll = app.network_view.scroll.min(
            paragraph
                .line_count(inner.width)
                .saturating_sub(height as usize),
        );
        frame.render_widget(
            paragraph.scroll((app.network_view.scroll as u16, 0)),
            Rect::new(inner.x, inner.y, inner.width, height),
        );
        for (row, index) in (app.network_view.choices..choices.len())
            .take(3)
            .enumerate()
        {
            control(
                app,
                frame,
                p,
                Rect::new(inner.x, inner.y + height + row as u16 * 2, inner.width, 2),
                &format!(" {} ", choices[index]),
                C::Choice(index),
            );
        }
        control(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 1, 3, 1),
            " ↑ ",
            C::Scroll(-3),
        );
        control(
            app,
            frame,
            p,
            Rect::new(inner.x + 3, inner.bottom() - 1, 3, 1),
            " ↓ ",
            C::Scroll(3),
        );
        if choices.len() > 3 {
            control(
                app,
                frame,
                p,
                Rect::new(inner.x + 7, inner.bottom() - 1, 6, 1),
                " Ещё ",
                C::MoreChoices,
            );
        }
    } else if current.is_some() {
        let mut fields = vec![];
        if let Some((
            _,
            Prompt::Password {
                message,
                need_user,
                need_domain,
                need_password,
                ..
            },
        )) = &app.network_view.prompt
        {
            let paragraph = Paragraph::new(message.clone())
                .style(p.muted())
                .wrap(Wrap { trim: true });
            app.network_view.scroll = app
                .network_view
                .scroll
                .min(paragraph.line_count(inner.width - 6).saturating_sub(2));
            frame.render_widget(
                paragraph.scroll((app.network_view.scroll as u16, 0)),
                Rect::new(inner.x, inner.y, inner.width - 6, 2),
            );
            if *need_user {
                fields.push(F::User);
            }
            if *need_domain {
                fields.push(F::Domain);
            }
            if *need_password {
                fields.push(F::Password);
            }
            control(
                app,
                frame,
                p,
                Rect::new(inner.right() - 6, inner.y, 3, 1),
                " ↑ ",
                C::Scroll(-2),
            );
            control(
                app,
                frame,
                p,
                Rect::new(inner.right() - 3, inner.y, 3, 1),
                " ↓ ",
                C::Scroll(2),
            );
        } else {
            fields.push(F::Address);
            frame.render_widget(
                Paragraph::new("Укажите сервер и папку").style(p.muted()),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
        }
        for (index, field) in fields.into_iter().enumerate() {
            let row = inner.y + 2 + index as u16 * 2;
            frame.render_widget(
                Paragraph::new(field.label()).style(p.muted()),
                Rect::new(inner.x, row, inner.width, 1),
            );
            let area = Rect::new(inner.x, row + 1, inner.width, 1);
            if current == Some(field) {
                super::text_edit::field(app, frame, area, p);
            } else {
                let value = match field {
                    F::User => app.network_view.user.clone(),
                    F::Domain => app.network_view.domain.clone(),
                    F::Password => "*".repeat(crate::text_edit::width(&app.network_view.password)),
                    F::Address => app.network_view.uri.clone(),
                };
                control(
                    app,
                    frame,
                    p,
                    area,
                    &files::shorten_name(&value, inner.width as usize),
                    C::Field(field),
                );
            }
        }
        if current == Some(F::Address) {
            for (i, (x, label)) in [(0, "SFTP"), (6, "SMB"), (11, "WebDAV"), (20, "FTP")]
                .into_iter()
                .enumerate()
            {
                control(
                    app,
                    frame,
                    p,
                    Rect::new(inner.x + x, inner.y + 5, label.width() as u16, 1),
                    label,
                    C::Scheme(i),
                );
            }
            let example = if app.network_view.uri.starts_with("smb:") {
                "Пример: smb://server/share/"
            } else if app.network_view.uri.starts_with("dav") {
                "Пример: davs://server/folder/"
            } else if app.network_view.uri.starts_with("ftp:") {
                "Пример: ftp://server/folder/"
            } else {
                "Пример: sftp://server/folder/"
            };
            if app.network_view.error.is_none() && app.network.error.is_none() {
                frame.render_widget(
                    Paragraph::new(example)
                        .style(p.muted())
                        .wrap(Wrap { trim: true }),
                    Rect::new(inner.x, inner.y + 7, inner.width, 2),
                );
            }
        }
        control(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 1, 13, 1),
            " Подключить ",
            C::Submit,
        );
        if matches!(
            app.network_view.prompt,
            Some((
                _,
                Prompt::Password {
                    anonymous: true,
                    ..
                }
            ))
        ) {
            control(
                app,
                frame,
                p,
                Rect::new(inner.x + 13, inner.bottom() - 1, 8, 1),
                " Гость ",
                C::Anonymous,
            );
        }
    } else if app.network.working.is_some() {
        frame.render_widget(
            Paragraph::new(format!(
                "{}…\n{}\n\nМожно отменить, не дожидаясь ответа сервера.",
                if app.network_disconnecting() {
                    "Отключение"
                } else {
                    "Подключение"
                },
                app.network_view.uri
            ))
            .style(p.text())
            .wrap(Wrap { trim: true }),
            Rect::new(inner.x, inner.y, inner.width, inner.height - 2),
        );
    } else {
        let page = (inner.height - 5) as usize;
        app.network_view.page = page;
        if app.network_view.selected < app.network_view.offset {
            app.network_view.offset = app.network_view.selected;
        }
        if app.network_view.selected >= app.network_view.offset + page {
            app.network_view.offset = app.network_view.selected + 1 - page;
        }
        let rows: Vec<_> = app
            .network
            .locations
            .iter()
            .enumerate()
            .skip(app.network_view.offset)
            .take(page)
            .map(|(i, l)| (i, l.label.clone()))
            .collect();
        for (row, (index, label)) in rows.into_iter().enumerate() {
            let r = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
            frame.render_widget(
                Paragraph::new(files::shorten_name(&label, inner.width as usize)).style(
                    if index == app.network_view.selected {
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
                Target::Network {
                    revision: app.network_view.revision,
                    control: C::Row(index),
                },
            );
        }
        if app.network.locations.is_empty() {
            frame.render_widget(
                Paragraph::new("Нет сетевых подключений").style(p.muted()),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
        }
        if let Some(location) = app.network.locations.get(app.network_view.selected) {
            frame.render_widget(
                Paragraph::new(location.uri.clone()).style(p.muted()),
                Rect::new(inner.x, inner.bottom() - 5, inner.width, 1),
            );
        }
        control(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 2, 13, 1),
            " Подключить ",
            C::New,
        );
        if !app.network.locations.is_empty() {
            control(
                app,
                frame,
                p,
                Rect::new(inner.x + 13, inner.bottom() - 2, 9, 1),
                " Открыть ",
                C::Open,
            );
            control(
                app,
                frame,
                p,
                Rect::new(inner.x + 22, inner.bottom() - 2, 10, 1),
                "Отключить",
                C::Disconnect,
            );
        }
        control(
            app,
            frame,
            p,
            Rect::new(inner.x, inner.bottom() - 1, 3, 1),
            " ↑ ",
            C::Row(app.network_view.selected.saturating_sub(1)),
        );
        control(
            app,
            frame,
            p,
            Rect::new(inner.x + 3, inner.bottom() - 1, 3, 1),
            " ↓ ",
            C::Row(
                (app.network_view.selected + 1).min(app.network.locations.len().saturating_sub(1)),
            ),
        );
    }
    if let Some(error) = app
        .network_view
        .error
        .as_ref()
        .or(app.network.error.as_ref())
    {
        let paragraph = Paragraph::new(error.lines().map(fs::clean).collect::<Vec<_>>().join("\n"))
            .style(p.text())
            .wrap(Wrap { trim: true });
        app.network_view.scroll = app
            .network_view
            .scroll
            .min(paragraph.line_count(inner.width - 6).saturating_sub(2));
        frame.render_widget(
            paragraph.scroll((app.network_view.scroll as u16, 0)),
            Rect::new(inner.x, inner.bottom() - 4, inner.width - 6, 2),
        );
        control(
            app,
            frame,
            p,
            Rect::new(inner.right() - 6, inner.bottom() - 4, 3, 2),
            " ↑ ",
            C::Scroll(-2),
        );
        control(
            app,
            frame,
            p,
            Rect::new(inner.right() - 3, inner.bottom() - 4, 3, 2),
            " ↓ ",
            C::Scroll(2),
        );
    }
    let cancel = Rect::new(inner.right() - 10, inner.bottom() - 1, 10, 1);
    frame.render_widget(Paragraph::new(" Отмена ").style(p.accent()), cancel);
    hit(app, cancel, Target::Cancel);
    if current.is_some() {
        super::text_edit::menu(app, frame, p);
    }
}

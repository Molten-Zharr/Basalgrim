use super::*;
use crate::app::Dialog;
use ratatui::widgets::{Clear, Wrap};

pub fn render(app: &mut App, frame: &mut Frame, palette: Palette) {
    if app.palette_dialog() {
        super::palette::render(app, frame, palette);
        return;
    }
    if app.details_dialog() {
        super::details::render(app, frame, palette);
        return;
    }
    if app.network_dialog() {
        super::network::render(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(Dialog::Devices)) {
        super::devices::render(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(Dialog::Places)) {
        super::places::render(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(Dialog::FolderSize)) {
        super::folder_size::render(app, frame, palette);
        return;
    }
    if app.recursive_editing() {
        super::search::render(app, frame, palette);
        return;
    }
    if app.batch_editing() {
        super::batch::render(app, frame, palette);
        return;
    }
    if matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: crate::app::EditKind::Path(_),
            ..
        })
    ) {
        // The editor is drawn by the address bar. Outside clicks dismiss it without click-through.
        app.hits.retain(|(_, target)| {
            matches!(
                target,
                Target::TextSelect { .. } | Target::PathTextCopy | Target::Confirm | Target::Cancel
            )
        });
        app.hits.insert(0, (frame.area(), Target::DismissMenu));
        super::text_edit::menu(app, frame, palette);
        return;
    }
    if matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: crate::app::EditKind::Rename(_),
            ..
        })
    ) && let Some(field) = app.rename_area
    {
        app.hits
            .retain(|(_, target)| matches!(target, Target::Confirm | Target::Cancel));
        app.hits.insert(0, (frame.area(), Target::DismissMenu));
        app.icon_slots
            .retain(|slot| slot.area.intersection(field).is_empty());
        super::text_edit::field(app, frame, field, palette);
        super::text_edit::menu(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(Dialog::Paths(_))) {
        super::path::menu(app, frame, palette);
        return;
    }
    if matches!(app.dialog, Some(Dialog::Commands(_))) {
        super::commands::render(app, frame, palette);
        return;
    }
    if let Some(Dialog::CaptureBinding { index, error }) = &app.dialog {
        super::settings::capture(app, frame, palette, *index, error.clone());
        return;
    }
    if matches!(
        app.dialog,
        Some(Dialog::Settings | Dialog::Operations | Dialog::HistoryStorage)
    ) {
        return;
    }
    let Some(dialog) = &app.dialog else {
        return;
    };
    let area = frame.area();
    let height = if matches!(dialog, Dialog::Help) {
        28
    } else if matches!(dialog, Dialog::Trash(_)) {
        12
    } else {
        10
    };
    let width = if matches!(dialog, Dialog::Help) {
        78
    } else {
        68
    };
    let rect = Rect::new(
        area.x + (area.width - area.width.min(width)) / 2,
        area.y + (area.height - area.height.min(height)) / 2,
        area.width.min(width),
        area.height.min(height),
    );
    app.icon_slots
        .retain(|slot| slot.area.intersection(rect).is_empty());
    if app
        .preview_slot
        .as_ref()
        .is_some_and(|slot| !slot.area.intersection(rect).is_empty())
    {
        app.preview_slot = None;
    }
    frame.render_widget(Clear, rect);
    let title = match dialog {
        Dialog::Edit { kind, .. } => kind.title(),
        Dialog::Trash(_) => "Отправить в корзину?",
        Dialog::RestoreTrash { .. } => "Восстановить из корзины?",
        Dialog::DeleteTrash { empty: true, .. } => "Очистить всю корзину?",
        Dialog::DeleteTrash { .. } => "Удалить безвозвратно?",
        Dialog::Help => "Управление · basalgrim",
        Dialog::CloseTerminal => "Завершить терминал?",
        Dialog::CleanupHistory(_) => "Очистить данные undo/redo?",
        Dialog::Details
        | Dialog::Network
        | Dialog::Devices
        | Dialog::Places
        | Dialog::Settings
        | Dialog::CaptureBinding { .. }
        | Dialog::Operations
        | Dialog::HistoryStorage
        | Dialog::Conflict(_)
        | Dialog::Commands(_) => unreachable!(),
        Dialog::Paths(_) | Dialog::FolderSize => unreachable!(),
    };
    let block = panel(palette, format!(" {title} "), true);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut lines = vec![Line::raw("")];
    match dialog {
        Dialog::Details
        | Dialog::Network
        | Dialog::Devices
        | Dialog::Places
        | Dialog::Settings
        | Dialog::CaptureBinding { .. }
        | Dialog::Operations
        | Dialog::HistoryStorage
        | Dialog::Conflict(_)
        | Dialog::Commands(_) => unreachable!(),
        Dialog::Paths(_) | Dialog::FolderSize => unreachable!(),
        Dialog::CloseTerminal => {
            lines = vec![
                Line::raw(" Оболочка и активная команда будут"),
                Line::raw(" завершены. Чтобы оставить их работать,"),
                Line::raw(" отмените и нажмите «Скрыть». "),
            ];
        }
        Dialog::CleanupHistory(selection) => {
            lines = vec![
                Line::raw(format!(
                    " Записей: {} · {}",
                    selection.entries.len(),
                    fs::size(selection.bytes)
                )),
                Line::raw(" Сохранённые файлы будут удалены."),
                Line::raw(" Отмена и повтор этих записей"),
                Line::raw(" станут недоступны."),
                Line::raw(" Журнал операций сохранится."),
            ];
        }
        Dialog::Edit { error, kind, .. } => {
            lines.push(Line::raw(""));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                error.clone().unwrap_or_else(|| match kind {
                    crate::app::EditKind::ArchiveCreate { .. } => {
                        " Имя архива и папка назначения".into()
                    }
                    crate::app::EditKind::ArchiveExtract { .. } => {
                        " Создать папку с содержимым архива".into()
                    }
                    _ => " Shift+стрелки — выделить · ПКМ — буфер · Ctrl+U — очистить".into(),
                }),
                palette.muted(),
            ));
        }
        Dialog::DeleteTrash { targets, empty } => {
            lines.push(Line::styled(
                if *empty {
                    format!(" Вся корзина: {} элемент(ов)", targets.len())
                } else {
                    format!(" Выбрано: {} элемент(ов)", targets.len())
                },
                palette.accent(),
            ));
            lines.push(Line::styled(
                " Это удаление невозможно отменить.",
                palette.text(),
            ));
            for target in targets.iter().take(2) {
                lines.push(Line::styled(
                    format!(
                        " {}",
                        app.trash
                            .as_ref()
                            .and_then(|view| view
                                .catalog
                                .entries
                                .iter()
                                .find(|entry| entry.data == target.path()))
                            .map(|entry| entry.name.clone())
                            .unwrap_or_else(|| fs::clean(
                                &target
                                    .path()
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                            ))
                    ),
                    palette.muted(),
                ));
            }
        }
        Dialog::RestoreTrash {
            entries,
            destination,
        } => {
            lines.push(Line::styled(
                format!(" {} элемент(ов)", entries.len()),
                palette.accent(),
            ));
            lines.push(Line::styled(
                if let Some(path) = destination {
                    format!(" В папку: {}", fs::clean(&path.to_string_lossy()))
                } else {
                    " На исходные места".into()
                },
                palette.text(),
            ));
            for entry in entries.iter().take(2) {
                lines.push(Line::styled(format!(" {}", entry.name), palette.text()));
            }
            lines.push(Line::styled(
                " Совпадения имён требуют отдельного выбора.",
                palette.muted(),
            ));
        }
        Dialog::Trash(paths) => {
            lines.push(Line::styled(
                format!(" {} элемент(ов) → системная корзина", paths.len()),
                palette.accent(),
            ));
            for path in paths.iter().take(3) {
                lines.push(Line::styled(
                    format!(
                        " {}",
                        fs::clean(&path.file_name().unwrap_or_default().to_string_lossy())
                    ),
                    palette.text(),
                ));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                " Восстановление доступно в разделе «Корзина».",
                palette.muted(),
            ));
        }
        Dialog::Help => {
            for (keys, description) in [
                ("↑ ↓ ← → / Home End / PgUp PgDn", "Навигация по файлам"),
                ("Shift + навигация / щелчок", "Выделить диапазон файлов"),
                (
                    "Ctrl + Shift + навигация / щелчок",
                    "Добавить диапазон к выбору",
                ),
                ("Esc", "Сбросить выделение и фильтр"),
            ] {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {keys:<30}"), palette.accent()),
                    Span::styled(description, palette.text()),
                ]));
            }
            for &action in Action::ALL {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {:<30}", app.settings.shortcuts(action)),
                        palette.accent(),
                    ),
                    Span::styled(action.name(), palette.text()),
                ]));
            }
            lines.push(Line::styled(
                " Колесо над предпросмотром — прокрутить содержимое",
                palette.muted(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                " Перенос между дисками: проверка копии перед удалением.",
                palette.muted(),
            ));
        }
    }
    let content = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    if matches!(app.dialog, Some(Dialog::Help)) {
        app.help_max = paragraph
            .line_count(content.width)
            .saturating_sub(usize::from(content.height));
        app.help_offset = app.help_offset.min(app.help_max);
        frame.render_widget(paragraph.scroll((app.help_offset as u16, 0)), content);
    } else {
        frame.render_widget(paragraph, content);
    }
    // Only modal hit regions remain interactive while a dialog is open.
    app.hits.clear();
    if matches!(app.dialog, Some(Dialog::Edit { .. })) {
        super::text_edit::field(
            app,
            frame,
            Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
            palette,
        );
    }
    if matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: crate::app::EditKind::ArchiveCreate { .. },
            ..
        })
    ) {
        use crate::archive::Format;
        let mut x = inner.x + 1;
        for (text, format) in [
            (" ZIP ", Format::Zip),
            (" TAR ", Format::Tar),
            (" TAR.GZ ", Format::TarGz),
        ] {
            let area = Rect::new(x, inner.y + 5, text.len() as u16, 1);
            frame.render_widget(Paragraph::new(text).style(palette.accent()), area);
            hit(app, area, Target::ArchiveFormat(format));
            x += area.width + 1;
        }
    }
    if rect.width >= 4 && rect.height > 0 {
        let close = Rect::new(rect.right() - 4, rect.y, 3, 1);
        frame.render_widget(Paragraph::new(" × ").style(palette.accent()), close);
        hit(app, close, Target::Cancel);
    }
    let bottom = Rect::new(
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        inner.width.saturating_sub(2),
        1,
    );
    let confirm = Rect::new(bottom.x, bottom.y, 22.min(bottom.width), 1);
    frame.render_widget(
        Paragraph::new(if matches!(app.dialog, Some(Dialog::Trash(_))) {
            " Enter  В корзину "
        } else if matches!(
            app.dialog,
            Some(Dialog::DeleteTrash { empty: true, .. } | Dialog::CleanupHistory(_))
        ) {
            " Enter  Очистить "
        } else if matches!(app.dialog, Some(Dialog::DeleteTrash { .. })) {
            " Enter  Удалить "
        } else if matches!(app.dialog, Some(Dialog::RestoreTrash { .. })) {
            " Enter  Восстановить "
        } else if matches!(app.dialog, Some(Dialog::Help)) {
            " Enter  Закрыть "
        } else {
            if matches!(app.dialog, Some(Dialog::CloseTerminal)) {
                " Enter  Завершить "
            } else {
                " Enter  Применить "
            }
        })
        .style(
            palette
                .text()
                .fg(palette.color(if palette.light() {
                    "button_text"
                } else {
                    "background"
                }))
                .bg(palette.color("confirm_background")),
        ),
        confirm,
    );
    hit(app, confirm, Target::Confirm);
    if bottom.width > 25 {
        let cancel = Rect::new(bottom.x + 24, bottom.y, 16.min(bottom.width - 24), 1);
        frame.render_widget(
            Paragraph::new(" Esc  Закрыть ").style(palette.accent()),
            cancel,
        );
        hit(app, cancel, Target::Cancel);
    }
    if matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: crate::app::EditKind::Search { .. },
            ..
        })
    ) && !app.in_archive()
    {
        let r = Rect::new(inner.x + 1, inner.y + 4, inner.width.saturating_sub(2), 1);
        frame.render_widget(Paragraph::new(" Подпапки · F3").style(palette.accent()), r);
        hit(app, r, Target::RecursiveSearch);
    }
    super::text_edit::menu(app, frame, palette);
}

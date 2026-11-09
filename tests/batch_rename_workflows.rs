use basalgrim::{
    app::{App, Dialog, EditKind, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Position, Rect},
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(6);
    while app.loading() || app.journal_loading {
        assert!(Instant::now() < end, "{}", app.notice);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup(grid: bool, count: usize) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    for i in 0..count {
        fs::write(files.join(format!("file-{i:03}.txt")), format!("data {i}")).unwrap();
    }
    let mut app = App::new(
        files,
        Settings {
            grid,
            preview: false,
            ..Settings::default()
        },
    );
    app.attach_journal(root.path().join("journal"));
    settle(&mut app);
    (root, app)
}
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
fn hit(app: &App, which: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| which(t).then_some(*r))
        .expect("missing mouse control")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
fn key(app: &mut App, key: KeyPress) {
    app.handle(Input::Key(key));
}
fn edit(app: &mut App, text: &str) {
    key(app, KeyPress::ctrl('a'));
    app.handle(Input::Text(text.into()));
}
fn ready(app: &App) -> bool {
    app.batch
        .as_ref()
        .and_then(|v| v.preview.as_ref())
        .is_some_and(|p| p.ready())
}

#[test]
fn group_f2_and_mouse_preset_apply_work_at_minimum_size_in_grid_and_list() {
    for grid in [false, true] {
        for (w, h) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid, 3);
            app.action(Action::SelectAll);
            app.action(Action::Copy);
            let clipboard = app.clipboard.as_ref().unwrap().paths.clone();
            let focus = app.tab().entry().unwrap().name.clone();
            key(&mut app, KeyPress::plain(Key::F(2)));
            settle(&mut app);
            render(&mut app, w, h);
            assert!(matches!(
                app.dialog,
                Some(Dialog::Edit {
                    kind: EditKind::BatchRename,
                    ..
                })
            ));
            assert!(ready(&app));
            let number = hit(&app, |t| matches!(t, Target::BatchNumber));
            click(&mut app, number);
            settle(&mut app);
            render(&mut app, w, h);
            assert!(!ready(&app));
            assert!(!app.hits.iter().any(|(_, t)| matches!(t, Target::Confirm)));
            let number = hit(&app, |t| matches!(t, Target::BatchNumber));
            click(&mut app, number);
            settle(&mut app);
            render(&mut app, w, h);
            assert!(ready(&app));
            assert_eq!(
                app.batch
                    .as_ref()
                    .unwrap()
                    .preview
                    .as_ref()
                    .unwrap()
                    .changed,
                3
            );
            assert!(
                app.hits
                    .iter()
                    .all(|(r, _)| r.right() <= w && r.bottom() <= h)
            );
            let apply = hit(&app, |t| matches!(t, Target::Confirm));
            click(&mut app, apply);
            settle(&mut app);
            render(&mut app, w, h);
            assert!(app.dialog.is_none());
            assert_eq!(app.operation_count, 1);
            assert_eq!(app.clipboard.as_ref().unwrap().paths, clipboard);
            for i in 0..3 {
                assert!(
                    root.path()
                        .join(format!("files/file-{i:03}_{:03}.txt", i + 1))
                        .exists()
                );
            }
            assert_eq!(app.tab().marked.len(), 3);
            assert_eq!(
                app.tab().entry().unwrap().name,
                focus.replace(".txt", "_001.txt")
            );
            key(&mut app, KeyPress::ctrl('z'));
            settle(&mut app);
            for i in 0..3 {
                assert!(root.path().join(format!("files/file-{i:03}.txt")).exists());
            }
            key(&mut app, KeyPress::ctrl('y'));
            settle(&mut app);
            assert!(root.path().join("files/file-002_003.txt").exists());
        }
    }
}

#[test]
fn contextual_group_rename_reuses_shared_action_and_single_file_stays_inline() {
    let (_root, mut app) = setup(false, 3);
    render(&mut app, 100, 32);
    app.action(Action::Rename);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Rename(_),
            ..
        })
    ));
    app.cancel_dialog();
    app.action(Action::SelectAll);
    let item = hit(&app, |t| matches!(t, Target::Entry { index: 1, .. }));
    app.handle(Input::SecondaryClick {
        x: item.x + 5,
        y: item.y,
    });
    render(&mut app, 100, 32);
    let index = app
        .command_items()
        .iter()
        .position(|item| item.action == Action::Rename)
        .unwrap();
    let command = hit(&app, |t| matches!(t,Target::Command(i) if *i==index));
    click(&mut app, command);
    settle(&mut app);
    assert!(app.batch_editing());
    assert_eq!(
        app.batch
            .as_ref()
            .unwrap()
            .preview
            .as_ref()
            .unwrap()
            .rows
            .len(),
        3
    );
}

#[test]
fn errors_block_apply_and_refresh_rechecks_the_filesystem_without_losing_input() {
    let (root, mut app) = setup(false, 3);
    app.action(Action::SelectAll);
    app.action(Action::Rename);
    key(&mut app, KeyPress::plain(Key::F(4)));
    edit(&mut app, "same.txt");
    settle(&mut app);
    render(&mut app, 34, 14);
    assert_eq!(
        app.batch
            .as_ref()
            .unwrap()
            .preview
            .as_ref()
            .unwrap()
            .conflicts,
        3
    );
    assert!(!app.hits.iter().any(|(_, t)| matches!(t, Target::Confirm)));
    key(&mut app, KeyPress::plain(Key::Enter));
    assert!(app.batch_editing());
    assert_eq!(app.operation_count, 0);
    edit(&mut app, "new_{full}");
    settle(&mut app);
    assert!(ready(&app));
    fs::write(root.path().join("files/new_file-001.txt"), "external").unwrap();
    let refresh = hit(&app, |t| matches!(t, Target::BatchRefresh));
    click(&mut app, refresh);
    settle(&mut app);
    assert!(!ready(&app));
    assert_eq!(
        app.batch
            .as_ref()
            .unwrap()
            .preview
            .as_ref()
            .unwrap()
            .conflicts,
        1
    );
    assert!(matches!(&app.dialog,Some(Dialog::Edit{text,..}) if text=="new_{full}"));
    fs::remove_file(root.path().join("files/new_file-001.txt")).unwrap();
    key(&mut app, KeyPress::plain(Key::F(5)));
    settle(&mut app);
    assert!(ready(&app));
    fs::write(
        root.path().join("files/file-001.txt"),
        "edited after preview",
    )
    .unwrap();
    key(&mut app, KeyPress::plain(Key::Enter));
    settle(&mut app);
    assert_eq!(app.operations[0].progress.items_done, 0);
    assert!(app.notice_error);
    assert!(!root.path().join("files/new_file-000.txt").exists());
}

#[test]
fn preview_scroll_and_text_selection_never_change_files_selection_or_clipboard() {
    let (_root, mut app) = setup(true, 50);
    app.action(Action::SelectAll);
    app.action(Action::Copy);
    let targets = app.tab().targets();
    let selected = app.tab().selected;
    app.action(Action::Rename);
    edit(&mut app, "Фото");
    settle(&mut app);
    render(&mut app, 34, 14);
    key(&mut app, KeyPress::ctrl('a'));
    key(&mut app, KeyPress::ctrl('c'));
    assert_eq!(app.take_text_copy().unwrap().text, "Фото");
    assert_eq!(app.clipboard.as_ref().unwrap().paths, targets);
    app.handle(Input::Scroll {
        x: 10,
        y: 8,
        delta: 3,
    });
    assert!(app.batch.as_ref().unwrap().offset > 0);
    key(&mut app, KeyPress::plain(Key::PageDown));
    render(&mut app, 34, 14);
    let offset = app.batch.as_ref().unwrap().offset;
    assert!(offset > 3);
    let up = hit(&app, |t| matches!(t,Target::BatchScroll(d) if *d<0));
    click(&mut app, up);
    assert!(app.batch.as_ref().unwrap().offset < offset);
    assert_eq!(app.tab().targets(), targets);
    assert_eq!(app.tab().selected, selected);
    let background = hit(&app, |t| matches!(t, Target::MenuBackground));
    app.handle(Input::Pointer(Some(Position::new(
        background.x,
        background.y,
    ))));
    assert!(app.hovered_area().is_none());
    let cancel = hit(&app, |t| matches!(t, Target::Cancel));
    click(&mut app, cancel);
    assert!(app.batch.is_none() && app.dialog.is_none());
    assert_eq!(app.operation_count, 0);
}

#[test]
fn stale_previews_and_changed_tab_owners_cannot_apply_a_previous_plan() {
    let (root, mut app) = setup(false, 3);
    app.action(Action::SelectAll);
    app.action(Action::Rename);
    key(&mut app, KeyPress::plain(Key::F(4)));
    edit(&mut app, "old_{full}");
    edit(&mut app, "new_{full}");
    key(&mut app, KeyPress::plain(Key::Enter));
    assert_eq!(app.operation_count, 0);
    assert!(app.batch_editing());
    settle(&mut app);
    assert!(
        app.batch
            .as_ref()
            .unwrap()
            .preview
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .all(|r| r.proposed.starts_with("new_"))
    );
    app.active = 1;
    render(&mut app, 100, 32);
    app.tick();
    assert!(app.dialog.is_none() && app.batch.is_none());
    app.confirm_dialog();
    settle(&mut app);
    assert!(root.path().join("files/file-000.txt").exists());
    assert!(!root.path().join("files/new_file-000.txt").exists());
}

#[test]
fn ordinary_name_needs_no_template_and_keeps_mixed_extensions() {
    for grid in [false, true] {
        for (w, h) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid, 3);
            let source = root.path().join("files");
            for (i, name) in ["IMG_1234.jpg", "IMG_5678.png", "README"]
                .into_iter()
                .enumerate()
            {
                fs::rename(source.join(format!("file-{i:03}.txt")), source.join(name)).unwrap();
            }
            app.action(Action::Refresh);
            settle(&mut app);
            app.action(Action::SelectAll);
            app.action(Action::Rename);
            settle(&mut app);
            let screen = render(&mut app, w, h);
            assert!(screen.contains("Например: Фото"));
            assert!(screen.contains("Общее имя без расширения"));
            assert!(screen.contains("Было:") && screen.contains("Будет:"));
            assert!(!screen.contains("{name}") && !screen.contains("{ext}"));
            let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
            click(&mut app, field);
            app.handle(Input::Text("Фото".into()));
            settle(&mut app);
            let screen = render(&mut app, w, h);
            assert!(screen.contains("Фото_001.jpg") && screen.contains("Фото_002.png"));
            assert_eq!(
                app.batch
                    .as_ref()
                    .unwrap()
                    .preview
                    .as_ref()
                    .unwrap()
                    .rows
                    .iter()
                    .map(|r| r.proposed.as_str())
                    .collect::<Vec<_>>(),
                ["Фото_001.jpg", "Фото_002.png", "Фото_003"]
            );
            let number = hit(&app, |t| matches!(t, Target::BatchNumber));
            click(&mut app, number);
            settle(&mut app);
            assert!(matches!(&app.dialog, Some(Dialog::Edit { text, .. }) if text == "Фото"));
            assert_eq!(
                app.batch.as_ref().unwrap().preview.as_ref().unwrap().rows[0].proposed,
                "Фото.jpg"
            );
            click(&mut app, number);
            settle(&mut app);
            render(&mut app, w, h);
            let apply = hit(&app, |t| matches!(t, Target::Confirm));
            click(&mut app, apply);
            settle(&mut app);
            for (i, name) in ["Фото_001.jpg", "Фото_002.png", "Фото_003"]
                .into_iter()
                .enumerate()
            {
                assert_eq!(
                    fs::read_to_string(source.join(name)).unwrap(),
                    format!("data {i}")
                );
            }
        }
    }
}

#[test]
fn advanced_extension_choices_preview_apply_and_undo_with_mouse_at_all_sizes() {
    for grid in [false, true] {
        for (w, h) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid, 3);
            let directory = root.path().join("files");
            let originals = ["file-000.png", "file-001.jpg", "file-002"];
            for (i, name) in originals.iter().enumerate() {
                fs::rename(
                    directory.join(format!("file-{i:03}.txt")),
                    directory.join(name),
                )
                .unwrap();
            }
            app.action(Action::Refresh);
            settle(&mut app);
            for (pattern, suffixes) in [
                ("Photo_{n:03}", [".png", ".jpg", ""]),
                ("Photo_{n:03}{ext:jpg}", [".jpg"; 3]),
                ("Photo_{n:03}{ext:}", [""; 3]),
            ] {
                app.action(Action::SelectAll);
                app.action(Action::Rename);
                render(&mut app, w, h);
                let mode = hit(&app, |t| matches!(t, Target::BatchMode));
                click(&mut app, mode);
                edit(&mut app, pattern);
                settle(&mut app);
                let expected = suffixes
                    .iter()
                    .enumerate()
                    .map(|(i, ext)| format!("Photo_{:03}{ext}", i + 1))
                    .collect::<Vec<_>>();
                let preview = app.batch.as_ref().unwrap().preview.as_ref().unwrap();
                assert_eq!(
                    preview
                        .rows
                        .iter()
                        .map(|row| &row.proposed)
                        .collect::<Vec<_>>(),
                    expected.iter().collect::<Vec<_>>()
                );
                let screen = render(&mut app, w, h);
                assert!(screen.contains(&format!("Будет: {}", expected[0])));
                let apply = hit(&app, |t| matches!(t, Target::Confirm));
                click(&mut app, apply);
                settle(&mut app);
                for (i, name) in expected.iter().enumerate() {
                    assert_eq!(
                        fs::read_to_string(directory.join(name)).unwrap(),
                        format!("data {i}")
                    );
                    assert!(!directory.join(originals[i]).exists());
                }
                app.action(Action::Undo);
                settle(&mut app);
                for (i, name) in originals.iter().enumerate() {
                    assert_eq!(
                        fs::read_to_string(directory.join(name)).unwrap(),
                        format!("data {i}")
                    );
                }
            }
        }
    }
}

#[test]
fn mode_switches_preserve_both_inputs_and_reject_pending_paste() {
    let (_root, mut app) = setup(false, 3);
    app.action(Action::SelectAll);
    app.action(Action::Copy);
    let clipboard = app.clipboard.clone().unwrap().paths;
    app.action(Action::Rename);
    edit(&mut app, "Фото_{name}");
    settle(&mut app);
    assert_eq!(
        app.batch.as_ref().unwrap().preview.as_ref().unwrap().rows[0].proposed,
        "Фото_{name}_001.txt"
    );
    render(&mut app, 34, 14);
    key(&mut app, KeyPress::ctrl('v'));
    let paste = app.take_text_paste_fallback().unwrap();
    let mode = hit(&app, |t| matches!(t, Target::BatchMode));
    click(&mut app, mode);
    settle(&mut app);
    assert!(app.batch.as_ref().unwrap().advanced);
    app.finish_text_paste(paste, Ok("stale".into()));
    assert!(
        matches!(&app.dialog, Some(Dialog::Edit { text, .. }) if text == "Фото_{{name}}_{n:03}")
    );
    edit(&mut app, "prefix_{full}");
    settle(&mut app);
    key(&mut app, KeyPress::plain(Key::F(4)));
    settle(&mut app);
    assert!(matches!(&app.dialog, Some(Dialog::Edit { text, .. }) if text == "Фото_{name}"));
    key(&mut app, KeyPress::plain(Key::F(3)));
    settle(&mut app);
    assert!(!ready(&app));
    let screen = render(&mut app, 34, 14);
    assert!(screen.contains("включите номера"));
    key(&mut app, KeyPress::plain(Key::F(4)));
    settle(&mut app);
    assert!(matches!(&app.dialog, Some(Dialog::Edit { text, .. }) if text == "prefix_{full}"));
    assert_eq!(app.clipboard.as_ref().unwrap().paths, clipboard);
    app.cancel_dialog();
    assert_eq!(app.operation_count, 0);
}

#[test]
fn invalid_custom_template_explains_the_exact_error_and_scrolls_all_corrections() {
    for grid in [false, true] {
        for (w, h) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid, 3);
            app.action(Action::SelectAll);
            app.action(Action::Rename);
            key(&mut app, KeyPress::plain(Key::F(4)));
            edit(&mut app, "Снимок_{n:03}{jpg}");
            settle(&mut app);
            let mut screen = render(&mut app, w, h);
            assert!(screen.contains("Неизвестная переменная: {jpg}"));
            assert!(screen.contains("Символ 14"));
            assert!(!screen.contains("Будет: Фото_001.jpg") && !screen.contains('�'));
            assert!(!screen.contains("0–0 / 0"));
            assert!(!app.hits.iter().any(|(_, t)| matches!(t, Target::Confirm)));
            key(&mut app, KeyPress::plain(Key::Enter));
            assert!(app.batch_editing());
            assert_eq!(app.operation_count, 0);
            for _ in 0..3 {
                let down = hit(
                    &app,
                    |t| matches!(t,Target::BatchScroll(delta) if *delta > 0),
                );
                click(&mut app, down);
                screen.push_str(&render(&mut app, w, h));
            }
            assert!(screen.contains("Снимок_{n:03}{ext:jpg}"));
            assert!(screen.contains("Снимок_{n:03}"));
            edit(&mut app, "Снимок_{n:03}{ext:jpg}");
            settle(&mut app);
            let screen = render(&mut app, w, h);
            assert_eq!(app.batch.as_ref().unwrap().offset, 0);
            assert!(screen.contains("Будет: Снимок_001.jpg"));
            let apply = hit(&app, |t| matches!(t, Target::Confirm));
            click(&mut app, apply);
            settle(&mut app);
            for i in 0..3 {
                let name = root.path().join(format!("files/Снимок_{:03}.jpg", i + 1));
                assert_eq!(fs::read_to_string(name).unwrap(), format!("data {i}"));
            }
        }
    }
}

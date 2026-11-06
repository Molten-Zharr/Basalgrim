use basalgrim::{
    app::{App, Dialog, EditKind, Target, TextCommand},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let until = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup(grid: bool) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("alpha.txt"), "keep data").unwrap();
    let mut app = App::new(
        root.path().into(),
        Settings {
            grid,
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    (root, app)
}
fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
    *app.hits
        .iter()
        .find_map(|(r, t)| predicate(t).then_some(r))
        .expect("visible mouse control")
}
fn click(app: &mut App, area: Rect) {
    app.handle(Input::PointerDown {
        x: area.x,
        y: area.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::PointerUp {
        x: area.x,
        y: area.y,
    });
}
fn text(app: &App) -> &str {
    match &app.dialog {
        Some(Dialog::Edit { text, .. }) => text,
        _ => panic!("editor closed"),
    }
}
fn key(app: &mut App, press: KeyPress) {
    app.handle(Input::Key(press));
}
fn shift(key: Key) -> KeyPress {
    KeyPress {
        shift: true,
        ..KeyPress::plain(key)
    }
}

#[test]
fn confirming_rename_never_restores_the_old_label_or_moves_its_text() {
    for grid in [false, true] {
        for (width, height) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid);
            render(&mut app, width, height);
            app.action(Action::Rename);
            app.handle(Input::Text("beta".into()));
            let editing = render(&mut app, width, height);
            let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
            let position = |buffer: &ratatui::buffer::Buffer| {
                (field.x..field.right()).find(|&x| buffer[(x, field.y)].symbol() == "b")
            };
            let before = position(&editing).unwrap();
            key(&mut app, KeyPress::plain(Key::Enter));
            // No tick: the worker result has not yet reached the UI.
            let pending = render(&mut app, width, height);
            assert_eq!(position(&pending), Some(before), "name jumped on Enter");
            let line: String = (before..before + 8)
                .map(|x| pending[(x, field.y)].symbol())
                .collect();
            assert_eq!(line, "beta.txt", "pending rename flashed the old name");
            // Presentation must not change the path used by filesystem actions.
            assert_eq!(
                app.tab().entry().unwrap().path,
                root.path().join("alpha.txt")
            );
            settle(&mut app);
            let finished = render(&mut app, width, height);
            assert_eq!(position(&finished), Some(before), "name jumped after scan");
            assert_eq!(
                app.tab().entry().unwrap().path,
                root.path().join("beta.txt")
            );
        }
    }
}

#[test]
fn renamed_entry_keeps_focus_and_marks_when_sorting_changes_its_position() {
    for grid in [false, true] {
        let (root, mut app) = setup(grid);
        fs::write(root.path().join("middle.txt"), "neighbour").unwrap();
        app.refresh();
        settle(&mut app);
        app.action(Action::Mark);
        // Rename uses the one marked entry, even if marking advanced focus.
        app.action(Action::Rename);
        app.handle(Input::Text("zulu".into()));
        app.confirm_dialog();
        settle(&mut app);
        render(&mut app, 150, 44);
        let destination = root.path().join("zulu.txt");
        assert_eq!(app.tab().entry().unwrap().path, destination);
        assert_eq!(app.tab().targets(), [destination]);
        assert_eq!(app.tab().selected, 1);
        assert_eq!(
            fs::read_to_string(root.path().join("middle.txt")).unwrap(),
            "neighbour"
        );
    }
}

#[test]
fn rename_conflicts_use_the_actual_name_and_cancel_or_skip_restore_the_original() {
    use basalgrim::conflicts::Choice;

    for choice in [
        Choice::KeepBoth,
        Choice::Skip,
        Choice::Cancel,
        Choice::Replace,
    ] {
        let (root, mut app) = setup(false);
        fs::write(root.path().join("beta.txt"), "existing data").unwrap();
        app.refresh();
        settle(&mut app);
        app.action(Action::Rename);
        app.handle(Input::Text("beta".into()));
        app.confirm_dialog();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(app.dialog, Some(Dialog::Conflict(_))) {
            app.tick();
            assert!(Instant::now() < deadline, "rename conflict was not shown");
            std::thread::sleep(Duration::from_millis(1));
        }
        app.choose_conflict(choice);
        settle(&mut app);
        render(&mut app, 150, 44);
        let expected = match choice {
            Choice::KeepBoth => "beta (1).txt",
            Choice::Replace => "beta.txt",
            _ => "alpha.txt",
        };
        assert_eq!(app.tab().entry().unwrap().path, root.path().join(expected));
        assert_eq!(
            fs::read_to_string(root.path().join(expected)).unwrap(),
            "keep data"
        );
        if choice != Choice::Replace {
            assert_eq!(
                fs::read_to_string(root.path().join("beta.txt")).unwrap(),
                "existing data"
            );
        }
        assert_eq!(
            app.operations[0].completed_paths.is_empty(),
            matches!(choice, Choice::Skip | Choice::Cancel)
        );
    }
}

#[test]
fn rename_edits_the_entry_and_has_mouse_submit_cancel_at_every_layout() {
    for grid in [true, false] {
        for (width, height) in [(34, 14), (150, 44)] {
            let (root, mut app) = setup(grid);
            render(&mut app, width, height);
            let entry = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
            app.action(Action::Copy);
            app.action(Action::Rename);
            assert_eq!(&text(&app)[app.editor.selection(text(&app))], "alpha");
            render(&mut app, width, height);
            let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
            assert!(entry.contains((field.x, field.y).into()));
            assert!(field.right() <= entry.right());
            assert!(app.hits.iter().any(|(_, t)| matches!(t, Target::Cancel)));
            app.handle(Input::Text("x".into()));
            let buffer = render(&mut app, width, height);
            let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
            let displayed: String = (field.x..field.right())
                .map(|x| buffer[(x, field.y)].symbol())
                .collect();
            assert_eq!(
                displayed.trim(),
                "x.txt",
                "old centered labels must be erased before drawing an editor"
            );
            key(&mut app, KeyPress::plain(Key::Home));
            key(&mut app, shift(Key::Right));
            app.handle(Input::Text("Новое имя".into()));
            assert_eq!(text(&app), "Новое имя.txt");
            render(&mut app, width, height);
            let submit = hit(&app, |t| matches!(t, Target::Confirm));
            click(&mut app, submit);
            settle(&mut app);
            assert_eq!(
                fs::read_to_string(root.path().join("Новое имя.txt")).unwrap(),
                "keep data"
            );
            assert!(!root.path().join("alpha.txt").exists());
            assert_eq!(
                app.clipboard.as_ref().unwrap().paths,
                [root.path().join("alpha.txt")]
            );
        }
    }
}

#[test]
fn caret_selection_cut_and_paste_use_the_same_unicode_graphemes() {
    let (root, mut app) = setup(false);
    app.action(Action::Copy);
    app.action(Action::Rename);
    key(&mut app, KeyPress::ctrl('a'));
    app.handle(Input::Text("文件 e\u{301}x.txt".into()));
    render(&mut app, 150, 44);
    let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
    key(&mut app, KeyPress::plain(Key::Home));
    app.handle(Input::SelectionModifier(true));
    click(&mut app, Rect::new(field.x + 2, field.y, 1, 1));
    key(&mut app, KeyPress::ctrl('c'));
    assert_eq!(app.take_text_copy().unwrap().text, "文");
    app.handle(Input::SelectionModifier(false));
    app.handle(Input::PointerDown {
        x: field.x + 2,
        y: field.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: field.x + 4,
        y: field.y,
    });
    app.handle(Input::PointerUp {
        x: field.x + 4,
        y: field.y,
    });
    key(&mut app, KeyPress::ctrl('c'));
    assert_eq!(app.take_text_copy().unwrap().text, "件");
    key(&mut app, KeyPress::ctrl('x'));
    assert_eq!(text(&app), "文 e\u{301}x.txt");
    key(&mut app, KeyPress::ctrl('v'));
    let id = app.take_text_paste_fallback().unwrap();
    app.finish_text_paste(id, Ok("光".into()));
    assert_eq!(text(&app), "文光 e\u{301}x.txt");
    key(&mut app, KeyPress::plain(Key::End));
    for _ in 0..5 {
        key(&mut app, KeyPress::plain(Key::Left));
    }
    key(&mut app, shift(Key::Left));
    key(&mut app, KeyPress::ctrl('x'));
    assert_eq!(app.take_text_copy().unwrap().text, "e\u{301}");
    assert_eq!(text(&app), "文光 x.txt");
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        [root.path().join("alpha.txt")]
    );
}

#[test]
fn text_context_menu_is_small_contextual_and_never_tints_its_background() {
    let (_root, mut app) = setup(false);
    app.action(Action::Rename);
    render(&mut app, 34, 14);
    let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
    app.handle(Input::SecondaryClick {
        x: field.x,
        y: field.y,
    });
    render(&mut app, 34, 14);
    assert_eq!(
        app.hits
            .iter()
            .filter(|(_, t)| matches!(t, Target::TextCommand(_)))
            .count(),
        4
    );
    let background = hit(&app, |t| matches!(t, Target::TextMenuBackground));
    app.handle(Input::Pointer(Some((background.x, background.y).into())));
    assert_eq!(app.hovered_area(), None);
    let cut = hit(&app, |t| matches!(t, Target::TextCommand(TextCommand::Cut)));
    click(&mut app, cut);
    assert_eq!(text(&app), ".txt");
    render(&mut app, 34, 14);
    let cancel = hit(&app, |t| matches!(t, Target::Cancel));
    click(&mut app, cancel);
    assert!(app.dialog.is_none());
}

#[test]
fn delayed_paste_cannot_replace_later_input_or_a_new_editor() {
    let (_root, mut app) = setup(false);
    app.action(Action::Rename);
    key(&mut app, KeyPress::ctrl('v'));
    let old = app.take_text_paste_fallback().unwrap();
    app.handle(Input::Text("typed".into()));
    app.finish_text_paste(old, Ok("stale".into()));
    assert_eq!(text(&app), "typed.txt");
    key(&mut app, KeyPress::ctrl('v'));
    let old = app.take_text_paste_fallback().unwrap();
    key(&mut app, KeyPress::plain(Key::Escape));
    app.action(Action::NewFile);
    app.finish_text_paste(old, Ok("wrong-editor".into()));
    assert_eq!(text(&app), "");
}

#[test]
fn errors_keep_inline_input_and_outside_dismissal_does_not_navigate() {
    let (root, mut app) = setup(false);
    app.action(Action::Rename);
    key(&mut app, KeyPress::ctrl('a'));
    app.handle(Input::Text("../bad".into()));
    key(&mut app, KeyPress::plain(Key::Enter));
    assert!(matches!(
        &app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Rename(_),
            error: Some(_),
            ..
        })
    ));
    assert_eq!(text(&app), "../bad");
    render(&mut app, 34, 14);
    click(&mut app, Rect::new(0, 0, 1, 1));
    assert!(app.dialog.is_none());
    assert_eq!(app.tab().cwd, root.path());
    assert_eq!(
        fs::read_to_string(root.path().join("alpha.txt")).unwrap(),
        "keep data"
    );
}

#[test]
fn changed_tab_context_cancels_rename_and_unchanged_name_is_a_noop() {
    let (root, mut app) = setup(false);
    app.action(Action::Rename);
    key(&mut app, KeyPress::plain(Key::Enter));
    assert!(app.dialog.is_none() && !app.busy);
    app.action(Action::Rename);
    app.handle(Input::Text("new".into()));
    let another = root.path().join("elsewhere");
    fs::create_dir(&another).unwrap();
    app.navigate(another.clone(), true);
    app.confirm_dialog();
    settle(&mut app);
    assert!(app.dialog.is_none());
    assert!(root.path().join("alpha.txt").exists());
    assert!(!root.path().join("new.txt").exists());
    assert_eq!(app.tab().cwd, another);
}

#[test]
fn create_and_path_fields_share_cursor_shift_selection_and_word_deletion() {
    let (_root, mut app) = setup(false);
    for action in [Action::NewFile, Action::Path] {
        app.action(action);
        key(&mut app, KeyPress::ctrl('a'));
        app.handle(Input::Text("first second".into()));
        key(&mut app, KeyPress::plain(Key::Home));
        for _ in 0..5 {
            key(&mut app, shift(Key::Right));
        }
        app.handle(Input::Text("новый".into()));
        assert_eq!(text(&app), "новый second");
        key(&mut app, KeyPress::plain(Key::End));
        key(
            &mut app,
            KeyPress {
                ctrl: true,
                ..KeyPress::plain(Key::Backspace)
            },
        );
        assert_eq!(text(&app), "новый ");
        app.cancel_dialog();
    }
}

#[test]
fn emoji_clusters_and_long_replacements_remain_bounded_and_renderable() {
    let (_root, mut app) = setup(false);
    app.action(Action::NewFile);
    app.handle(Input::Text("a👩‍👩‍👧‍👦文e\u{301}".into()));
    key(&mut app, KeyPress::plain(Key::Left));
    key(&mut app, KeyPress::plain(Key::Backspace));
    assert_eq!(text(&app), "a👩‍👩‍👧‍👦e\u{301}");
    key(&mut app, shift(Key::Left));
    app.handle(Input::Text("Z".into()));
    assert_eq!(text(&app), "aZe\u{301}");
    key(&mut app, KeyPress::ctrl('a'));
    app.handle(Input::Text("界".repeat(5000)));
    assert_eq!(text(&app).chars().count(), 4096);
    render(&mut app, 34, 14);
    key(&mut app, KeyPress::ctrl('a'));
    app.handle(Input::Text("x\n\0y".into()));
    assert_eq!(text(&app), "xy");
}

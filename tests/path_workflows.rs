use basalgrim::{
    app::{App, Clipboard, Dialog, EditKind, Target},
    clipboard::Destination,
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < end);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| predicate(t).then_some(*r))
        .expect("missing path control")
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
fn key(app: &mut App, press: KeyPress) {
    app.handle(Input::Key(press));
}
fn setup(path: &Path) -> App {
    fs::create_dir_all(path).unwrap();
    let mut app = App::new(
        path.into(),
        Settings {
            preview: false,
            grid: false,
            ..Settings::default()
        },
    );
    settle(&mut app);
    app
}

#[test]
fn breadcrumbs_navigate_with_history_and_hover_does_not_select() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Родитель с пробелами/child");
    let mut app = setup(&path);
    fs::write(path.join("selected.txt"), "content").unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    app.tab_mut().marked.insert(path.join("selected.txt"));
    render(&mut app, 170, 44);
    let current = hit(
        &app,
        |t| matches!(t,Target::PathEdit(owner) if owner.cwd == path),
    );
    click(&mut app, current);
    assert_eq!(app.tab().marked.len(), 1);
    assert!(app.text_active());
    key(&mut app, KeyPress::plain(Key::Escape));
    render(&mut app, 170, 44);
    let parent = path.parent().unwrap();
    let target = hit(
        &app,
        |t| matches!(t,Target::PathGo { path: p, .. } if p == parent),
    );
    app.handle(Input::Pointer(Some((target.x, target.y).into())));
    render(&mut app, 170, 44);
    assert_eq!(app.tab().cwd, path);
    assert_eq!(app.tab().marked.len(), 1);
    click(&mut app, target);
    settle(&mut app);
    assert_eq!(app.tab().cwd, parent);
    app.action(Action::Back);
    settle(&mut app);
    assert_eq!(app.tab().cwd, path);
}

#[test]
fn copy_uses_full_text_and_keeps_file_clipboard_and_inactive_pane() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Кириллица spaces/child");
    let mut app = setup(&path);
    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    app.settings.dual = true;
    app.active = 1;
    app.navigate(other.clone(), true);
    settle(&mut app);
    app.active = 0;
    app.clipboard = Some(Clipboard {
        paths: vec![path.join("file")],
        cut: true,
    });
    app.tab_mut().marked.insert(path.join("file"));
    render(&mut app, 150, 44);
    let copy = hit(
        &app,
        |t| matches!(t,Target::PathCopy(owner) if owner.pane == 1),
    );
    click(&mut app, copy);
    let first = app.take_text_copy().unwrap();
    assert_eq!(first.text, other.to_str().unwrap());
    assert_eq!(app.active, 0);
    assert_eq!(app.tab().marked.len(), 1);
    assert!(app.clipboard.as_ref().unwrap().cut);
    assert_eq!(app.clipboard.as_ref().unwrap().paths, [path.join("file")]);
    app.action(Action::CopyPath);
    let second = app.take_text_copy().unwrap();
    assert_eq!(second.text, path.to_str().unwrap());
    app.finish_text_copy(first.id, Err("stale failure".into()));
    assert!(!app.notice_error);
    app.finish_text_copy(second.id, Ok(Destination::System));
    assert!(app.notice.contains("скопирован"));
    render(&mut app, 150, 44);
    let go = hit(
        &app,
        |t| matches!(t,Target::PathGo { owner, path:p } if owner.pane == 1 && p == Path::new("/")),
    );
    click(&mut app, go);
    settle(&mut app);
    assert_eq!(app.active, 1);
    assert_eq!(app.tab().cwd, Path::new("/"));
}

#[test]
fn compact_long_paths_expose_every_parent_and_only_modal_hit_regions() {
    let root = tempfile::tempdir().unwrap();
    let mut path = root.path().to_path_buf();
    for i in 0..14 {
        path.push(format!("long-parent-{i}"));
    }
    path.push("文件 e\u{301} с длинным названием");
    let mut app = setup(&path);
    for (width, height) in [(34, 14), (70, 24), (150, 44)] {
        render(&mut app, width, height);
        let copy = hit(&app, |t| matches!(t, Target::PathCopy(_)));
        click(&mut app, copy);
        assert_eq!(app.take_text_copy().unwrap().text, path.to_str().unwrap());
        let more = hit(&app, |t| matches!(t, Target::PathParents { .. }));
        click(&mut app, more);
        render(&mut app, width, height);
        assert!(
            !app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Entry { .. } | Target::PathGo { .. }))
        );
        let Dialog::Paths(menu) = app.dialog.as_ref().unwrap() else {
            panic!()
        };
        assert_eq!(menu.parents.len(), path.ancestors().count() - 1);
        assert!(menu.offset > 0);
        key(&mut app, KeyPress::plain(Key::Home));
        render(&mut app, width, height);
        let root_hit = hit(&app, |t| matches!(t, Target::PathParent(0)));
        app.handle(Input::Pointer(Some((root_hit.x, root_hit.y).into())));
        render(&mut app, width, height);
        assert_eq!(app.hovered_area(), Some(root_hit));
        let close = hit(&app, |t| matches!(t, Target::Cancel));
        click(&mut app, close);
        assert_eq!(app.tab().cwd, path);
    }
    render(&mut app, 34, 14);
    let more = hit(&app, |t| matches!(t, Target::PathParents { .. }));
    click(&mut app, more);
    key(&mut app, KeyPress::plain(Key::Home));
    render(&mut app, 34, 14);
    let root_hit = hit(&app, |t| matches!(t, Target::PathParent(0)));
    click(&mut app, root_hit);
    settle(&mut app);
    assert_eq!(app.tab().cwd, Path::new("/"));
}

#[test]
fn path_editor_selects_replaces_and_copies_with_mouse_and_keyboard() {
    let root = tempfile::tempdir().unwrap();
    let mut app = setup(root.path());
    for (width, height) in [(34, 14), (150, 44)] {
        render(&mut app, width, height);
        let edit = hit(&app, |t| matches!(t, Target::PathEdit(_)));
        click(&mut app, edit);
        assert!(
            !app.editor
                .selection(match &app.dialog {
                    Some(Dialog::Edit { text, .. }) => text,
                    _ => panic!(),
                })
                .is_empty()
        );
        key(&mut app, KeyPress::ctrl('c'));
        assert_eq!(
            app.take_text_copy().unwrap().text,
            root.path().to_str().unwrap()
        );
        app.handle(Input::Text("/tmp".into()));
        assert!(matches!(app.dialog,Some(Dialog::Edit { ref text, .. }) if text == "/tmp"));
        render(&mut app, width, height);
        let copy = hit(&app, |t| matches!(t, Target::PathTextCopy));
        click(&mut app, copy);
        assert_eq!(app.take_text_copy().unwrap().text, "/tmp");
        key(&mut app, KeyPress::ctrl('a'));
        key(&mut app, KeyPress::plain(Key::Backspace));
        assert!(matches!(app.dialog,Some(Dialog::Edit { ref text, .. }) if text.is_empty()));
        app.handle(Input::Text(root.path().to_str().unwrap().into()));
        render(&mut app, width, height);
        let go = hit(&app, |t| matches!(t, Target::Confirm));
        click(&mut app, go);
        settle(&mut app);
        assert_eq!(app.tab().cwd, root.path());
    }
}

#[test]
fn stale_geometry_cannot_navigate_a_replaced_tab_and_settings_hide_breadcrumbs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("child");
    let mut app = setup(&path);
    render(&mut app, 150, 44);
    let stale = hit(
        &app,
        |t| matches!(t,Target::PathGo { path:p,.. } if p == Path::new("/")),
    );
    app.panes[0].tab_mut().cwd = root.path().into();
    click(&mut app, stale);
    assert_eq!(app.tab().cwd, root.path());
    app.action(Action::Settings);
    render(&mut app, 150, 44);
    assert!(!app.hits.iter().any(|(_, t)| matches!(
        t,
        Target::PathGo { .. } | Target::PathEdit(_) | Target::PathCopy(_)
    )));
}

#[test]
fn raw_unix_paths_navigate_losslessly_and_refuse_lossy_text_copies() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(OsString::from_vec(b"raw-\xff".to_vec()));
    let mut app = setup(&path);
    app.action(Action::CopyPath);
    assert!(app.notice_error);
    assert!(app.take_text_copy().is_none());
    app.action(Action::Path);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Path(_),
            ..
        })
    ));
    key(&mut app, KeyPress::plain(Key::Enter));
    settle(&mut app);
    assert_eq!(app.tab().cwd, path);
    render(&mut app, 150, 44);
    let parent = hit(
        &app,
        |t| matches!(t,Target::PathGo {path:p,..} if p == root.path()),
    );
    click(&mut app, parent);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path());
}

#[test]
fn new_path_shortcut_preserves_existing_clipboard_bindings() {
    let mut settings = Settings::default();
    settings.bindings.retain(|b| b.action != Action::CopyPath);
    let custom = KeyPress {
        shift: true,
        ..KeyPress::ctrl('c')
    };
    settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Copy)
        .unwrap()
        .key = custom;
    settings.normalize().unwrap();
    assert_eq!(settings.action_for(custom), Some(Action::Copy));
    assert!(
        settings
            .bindings
            .iter()
            .any(|b| b.action == Action::CopyPath && b.key != custom)
    );
    settings.validate_bindings().unwrap();
}

#[test]
fn clicking_current_component_edits_in_place_without_hidden_buttons() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("opt");
    let dest = root.path().join("Другая папка");
    fs::create_dir(&dest).unwrap();
    let mut app = setup(&path);
    let before = render(&mut app, 150, 44);
    let current = hit(
        &app,
        |t| matches!(t, Target::PathEdit(owner) if owner.cwd == path),
    );
    let edit_hits: Vec<_> = app
        .hits
        .iter()
        .filter(|(_, t)| matches!(t, Target::PathEdit(_)))
        .collect();
    assert_eq!(edit_hits.len(), 1);
    assert_eq!(
        current.width, 5,
        "only the visible 'opt' component edits the path"
    );
    app.handle(Input::Pointer(Some(
        (current.right() + 2, current.y).into(),
    )));
    render(&mut app, 150, 44);
    assert_eq!(
        app.hovered_area(),
        None,
        "blank space must not be an invisible button"
    );
    click(&mut app, current);
    let editing = render(&mut app, 150, 44);
    let field = hit(&app, |t| matches!(t, Target::TextSelect { .. }));
    assert_eq!(field.y, current.y);
    for y in 0..current.y {
        for x in 0..150 {
            assert_eq!(before[(x, y)], editing[(x, y)]);
        }
    }
    app.handle(Input::Text(dest.to_str().unwrap().into()));
    render(&mut app, 150, 44);
    let go = hit(&app, |t| matches!(t, Target::Confirm));
    assert_eq!(go.y, current.y);
    click(&mut app, go);
    settle(&mut app);
    assert_eq!(app.tab().cwd, dest);
}

#[test]
fn inline_path_cursor_edits_unicode_and_outside_click_dismisses_without_navigation() {
    let root = tempfile::tempdir().unwrap();
    let mut app = setup(root.path());
    app.action(Action::Path);
    app.handle(Input::Text("/tmp/ёж".into()));
    key(&mut app, KeyPress::plain(Key::Left));
    app.handle(Input::Text("я".into()));
    key(&mut app, KeyPress::plain(Key::Delete));
    assert!(matches!(app.dialog, Some(Dialog::Edit {ref text,..}) if text == "/tmp/ёя"));
    let buffer = render(&mut app, 34, 14);
    assert!(
        text_row(
            &buffer,
            hit(&app, |t| matches!(t, Target::TextSelect { .. })).y
        )
        .contains("/tmp/ёя")
    );
    click(&mut app, Rect::new(0, 0, 1, 1));
    assert!(app.dialog.is_none());
    assert_eq!(app.tab().cwd, root.path());
}
fn text_row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn mouse_positions_caret_and_unicode_arrows_use_the_rendered_graphemes() {
    let root = tempfile::tempdir().unwrap();
    let mut app = setup(root.path());
    app.action(Action::Path);
    app.handle(Input::Text("/tmp/文件 e\u{301}x".into()));
    render(&mut app, 150, 44);
    let field = hit(&app, |target| matches!(target, Target::TextSelect { .. }));
    click(&mut app, Rect::new(field.x + 7, field.y, 1, 1)); // after the first wide character
    app.handle(Input::Text("Z".into()));
    assert!(matches!(app.dialog,Some(Dialog::Edit{ref text,..}) if text=="/tmp/文Z件 e\u{301}x"));
    key(&mut app, KeyPress::plain(Key::End));
    key(&mut app, KeyPress::plain(Key::Left));
    key(&mut app, KeyPress::plain(Key::Left));
    app.handle(Input::Text("Y".into()));
    assert!(matches!(app.dialog,Some(Dialog::Edit{ref text,..}) if text=="/tmp/文Z件 Ye\u{301}x"));
}

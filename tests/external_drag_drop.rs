use basalgrim::{
    app::{App, Clipboard, Target},
    conflicts::Choice,
    dnd::{self, DropAction},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    while app.busy
        || app.journal_loading
        || app.panes.iter().flat_map(|p| &p.tabs).any(|t| t.loading)
    {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, w: u16, h: u16) {
    Terminal::new(TestBackend::new(w, h))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn hit(app: &App, f: impl Fn(&Target) -> bool) -> Position {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| f(t).then_some(Position::new(r.x, r.y)))
        .unwrap()
}
fn fixture(grid: bool) -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("foreign");
    let dest = tmp.path().join("local");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::create_dir(dest.join("folder")).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    fs::write(source.join("b.txt"), "b").unwrap();
    let mut app = App::new(
        dest.clone(),
        Settings {
            grid,
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    render(&mut app, 140, 32);
    (tmp, app, source, dest)
}
fn offer(app: &mut App, id: u64, p: Position, action: DropAction) {
    assert_eq!(
        app.hover_external_drop(id, Some(p), 3, action),
        Some(action)
    );
    assert!(app.capture_external_drop(id));
}
#[test]
fn external_groups_use_common_copy_move_engine_and_preserve_clipboard_at_minimum_size() {
    for grid in [false, true] {
        for size in [(140, 32), (34, 14)] {
            for action in [DropAction::Copy, DropAction::Move] {
                let (_tmp, mut app, source, dest) = fixture(grid);
                render(&mut app, size.0, size.1);
                app.clipboard = Some(Clipboard {
                    paths: vec![source.join("b.txt")],
                    cut: false,
                });
                let before = app.tab().selected;
                let p = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
                assert_eq!(app.hover_external_drop(3, Some(p), 3, action), Some(action));
                assert_eq!(app.tab().selected, before);
                assert_eq!(app.active, 0);
                assert!(app.gesture_area().is_some());
                assert!(app.capture_external_drop(3));
                assert!(app.receive_external_drop(
                    3,
                    Ok(vec![source.join("a.txt"), source.join("b.txt")])
                ));
                settle(&mut app);
                assert_eq!(app.external_drop_result(), Some((3, Some(action))));
                assert_eq!(fs::read_to_string(dest.join("folder/a.txt")).unwrap(), "a");
                assert_eq!(source.join("a.txt").exists(), action == DropAction::Copy);
                assert_eq!(
                    app.clipboard.as_ref().unwrap().paths,
                    vec![source.join("b.txt")]
                );
            }
        }
    }
}
#[test]
fn stale_reads_navigation_resize_and_modal_overlays_cannot_redirect_a_captured_drop() {
    for change in 0..4 {
        let (_tmp, mut app, source, dest) = fixture(false);
        let p = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
        offer(&mut app, 3, p, DropAction::Move);
        match change {
            0 => app.refresh(),
            1 => app.navigate(source.clone(), true),
            2 => render(&mut app, 34, 14),
            _ => app.action(Action::Help),
        }
        assert!(!app.receive_external_drop(3, Ok(vec![source.join("a.txt")])));
        assert!(source.join("a.txt").exists());
        assert!(!dest.join("folder/a.txt").exists());
        assert!(!app.busy);
    }
    let (_tmp, mut app, source, dest) = fixture(false);
    let p = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    offer(&mut app, 7, p, DropAction::Move);
    assert!(!app.receive_external_drop(3, Ok(vec![source.join("a.txt")])));
    assert!(app.receive_external_drop(7, Ok(vec![source.join("b.txt")])));
    settle(&mut app);
    assert!(source.join("a.txt").exists());
    assert!(dest.join("folder/b.txt").exists());
}
#[test]
fn native_exports_capture_groups_restore_on_escape_and_never_remove_sources_on_foreign_success() {
    let (_tmp, mut app, _source, dest) = fixture(false);
    fs::write(dest.join("one.txt"), "one").unwrap();
    fs::write(dest.join("two.txt"), "two").unwrap();
    app.refresh();
    settle(&mut app);
    render(&mut app, 140, 32);
    app.tab_mut().marked = [dest.join("one.txt"), dest.join("two.txt")].into();
    app.tab_mut().explicit_selection = true;
    let index = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "one.txt")
        .unwrap();
    let p = hit(
        &app,
        |t| matches!(t,Target::Entry {pane:0,index:i} if *i==index),
    );
    let offer = app.export_file_drag_at(p).unwrap();
    assert_eq!(offer.paths.len(), 2);
    assert_eq!(offer.id % 2, 0);
    app.handle(Input::PointerUp { x: p.x, y: p.y });
    assert_eq!(app.outgoing_drag_id(), Some(offer.id));
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(app.outgoing_drag_id().is_none());
    assert_eq!(app.tab().marked.len(), 2);
    let offer = app.export_file_drag_at(p).unwrap();
    app.finish_export(offer.id, Ok(()));
    assert!(dest.join("one.txt").exists());
    assert!(dest.join("two.txt").exists());
}
#[test]
fn external_drop_to_an_inactive_tab_keeps_focus_and_retains_durable_undo() {
    let (tmp, mut app, source, dest) = fixture(false);
    app.attach_journal(tmp.path().join("journal.sqlite3"));
    settle(&mut app);
    app.action(Action::NewTab);
    app.navigate(dest.join("folder"), true);
    settle(&mut app);
    app.panes[0].active = 0;
    render(&mut app, 140, 32);
    let p = hit(&app, |t| matches!(t, Target::Tab { pane: 0, index: 1 }));
    offer(&mut app, 3, p, DropAction::Move);
    assert!(app.receive_external_drop(3, Ok(vec![source.join("a.txt")])));
    settle(&mut app);
    assert_eq!(app.panes[0].active, 0);
    assert!(dest.join("folder/a.txt").exists());
    assert_eq!(
        app.external_drop_result(),
        Some((3, Some(DropAction::Move)))
    );
    app.action(Action::Undo);
    settle(&mut app);
    assert!(source.join("a.txt").exists());
    assert!(!dest.join("folder/a.txt").exists());
}
#[test]
fn foreign_conflicts_use_keep_both_and_only_acknowledge_actual_completion() {
    let (_tmp, mut app, source, dest) = fixture(false);
    fs::write(dest.join("folder/a.txt"), "old").unwrap();
    let p = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    offer(&mut app, 3, p, DropAction::Copy);
    assert!(app.receive_external_drop(3, Ok(vec![source.join("a.txt")])));
    let deadline = Instant::now() + Duration::from_secs(8);
    while !matches!(app.dialog, Some(basalgrim::app::Dialog::Conflict(_))) {
        app.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(app.external_drop_result().is_none());
    app.choose_conflict(Choice::KeepBoth);
    settle(&mut app);
    assert_eq!(
        fs::read_to_string(dest.join("folder/a.txt")).unwrap(),
        "old"
    );
    assert_eq!(
        fs::read_to_string(dest.join("folder/a (1).txt")).unwrap(),
        "a"
    );
    assert_eq!(
        app.external_drop_result(),
        Some((3, Some(DropAction::Copy)))
    );
}
#[test]
fn uri_batches_preserve_unix_bytes_and_reject_nonlocal_or_malformed_data_as_a_whole() {
    use std::os::unix::ffi::OsStringExt;
    let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/name-\xff\n x".to_vec()));
    assert_eq!(
        dnd::decode(dnd::encode(vec![raw.clone()]).unwrap()).unwrap(),
        vec![raw]
    );
    for data in [
        b"file:///tmp/good\r\nfile://remote/tmp/bad\r\n".as_slice(),
        b"file:///tmp/good\r\nfile:///bad%XX",
        b"file:///tmp/%00",
        b"/tmp/plain-text",
    ] {
        assert!(dnd::decode(data.into()).is_err());
    }
    assert!(dnd::encode(vec![Path::new("relative").into()]).is_err());
}

#[test]
fn reading_an_external_drop_has_a_mouse_stop_control_and_rejects_late_data() {
    for (width, height) in [(140, 32), (34, 14)] {
        let (_tmp, mut app, source, dest) = fixture(false);
        render(&mut app, width, height);
        let p = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
        offer(&mut app, 3, p, DropAction::Move);
        assert!(app.external_drop_reading());
        render(&mut app, width, height);
        let stop = hit(&app, |t| {
            matches!(t, Target::Action(Action::CancelOperation))
        });
        app.handle(Input::PointerDown {
            x: stop.x,
            y: stop.y,
            double: false,
            toggle: false,
        });
        app.handle(Input::PointerUp {
            x: stop.x,
            y: stop.y,
        });
        assert!(!app.external_drop_reading());
        assert_eq!(app.take_drop_cancellations(), vec![3]);
        assert!(!app.receive_external_drop(3, Ok(vec![source.join("a.txt")])));
        assert!(source.join("a.txt").exists());
        assert!(!dest.join("folder/a.txt").exists());
    }
}

#[test]
fn escape_cancels_both_sides_of_a_native_drag_returning_to_its_source_window() {
    let (_tmp, mut app, _source, dest) = fixture(false);
    fs::write(dest.join("local.txt"), "local").unwrap();
    app.refresh();
    settle(&mut app);
    render(&mut app, 140, 32);
    let index = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "local.txt")
        .unwrap();
    let point = hit(
        &app,
        |t| matches!(t,Target::Entry {pane:0,index:i} if *i==index),
    );
    app.export_file_drag_at(point).unwrap();
    let folder = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    offer(&mut app, 3, folder, DropAction::Move);
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(app.outgoing_drag_id().is_none());
    assert!(!app.external_drop_reading());
    assert_eq!(app.take_drop_cancellations(), vec![3]);
    assert!(!app.receive_external_drop(3, Ok(vec![dest.join("local.txt")])));
    assert!(dest.join("local.txt").exists());
    assert!(!dest.join("folder/local.txt").exists());
}

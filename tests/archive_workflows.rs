use basalgrim::{
    app::{App, Dialog, EditKind, Target},
    archive::Format,
    fs::{Operation, operate},
    input::{Action, Input, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(10);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn hit(app: &App, f: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .find_map(|(r, t)| f(t).then_some(*r))
        .expect("Missing mouse target")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn select(app: &mut App, name: &str) {
    app.tab_mut().selected = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == name)
        .unwrap();
    app.update_preview();
    settle(app);
}
fn pack(source: &Path, path: &Path) {
    let r = operate(Operation::ArchiveCreate {
        sources: vec![source.into()],
        destination: path.into(),
        format: Format::Zip,
    });
    assert!(r.error.is_none(), "{r:?}");
}
#[test]
fn create_and_extract_dialogs_have_working_mouse_controls_at_minimum_size() {
    for grid in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("source");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("привет.txt"), "hello").unwrap();
        let mut app = App::new(
            temp.path().into(),
            Settings {
                grid,
                preview: false,
                ..Default::default()
            },
        );
        settle(&mut app);
        app.action(Action::ArchiveCreate);
        render(&mut app, 34, 14);
        let button = hit(&app, |t| matches!(t, Target::ArchiveFormat(Format::TarGz)));
        click(&mut app, button);
        assert!(
            matches!(&app.dialog,Some(Dialog::Edit{kind:EditKind::ArchiveCreate{..},text,..}) if text.ends_with("source.tar.gz"))
        );
        render(&mut app, 34, 14);
        let button = hit(&app, |t| matches!(t, Target::Confirm));
        click(&mut app, button);
        settle(&mut app);
        assert!(
            temp.path().join("source.tar.gz").is_file(),
            "{}",
            app.notice
        );
        select(&mut app, "source.tar.gz");
        app.action(Action::ArchiveExtract);
        let out = temp.path().join("result");
        app.handle(Input::Key(KeyPress::ctrl('u')));
        app.handle(Input::Text(out.to_string_lossy().into_owned()));
        render(&mut app, 34, 14);
        let button = hit(&app, |t| matches!(t, Target::Confirm));
        click(&mut app, button);
        settle(&mut app);
        assert_eq!(
            fs::read_to_string(out.join("source/привет.txt")).unwrap(),
            "hello"
        );
    }
}
#[test]
fn archive_navigation_reuses_tabs_breadcrumbs_preview_contexts_and_readonly_commands() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("data");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/file.txt"), "archive preview").unwrap();
    let archive = temp.path().join("data.zip");
    pack(&source, &archive);
    let mut app = App::new(
        temp.path().into(),
        Settings {
            grid: false,
            preview: true,
            ..Default::default()
        },
    );
    settle(&mut app);
    select(&mut app, "data.zip");
    render(&mut app, 100, 30);
    let r = hit(
        &app,
        |t| matches!(t,Target::Entry{index,..} if *index==app.tab().selected),
    );
    app.handle(Input::SecondaryClick { x: r.x, y: r.y });
    assert!(
        app.command_items()
            .iter()
            .any(|c| c.action == Action::ArchiveExtract && c.enabled)
    );
    app.cancel_dialog();
    app.action(Action::Open);
    settle(&mut app);
    assert!(app.in_archive());
    assert_eq!(app.tab().cwd, archive);
    app.action(Action::Open);
    settle(&mut app);
    assert_eq!(app.tab().cwd, archive.join("data"));
    app.action(Action::NewTab);
    settle(&mut app);
    app.action(Action::Open);
    settle(&mut app);
    assert_eq!(app.tab().cwd, archive.join("data/nested"));
    assert_eq!(app.panes[0].tabs[0].cwd, archive.join("data"));
    assert!(
        app.preview
            .as_ref()
            .unwrap()
            .lines
            .join("\n")
            .contains("archive preview")
    );
    let before = fs::read(&archive).unwrap();
    for action in [
        Action::Cut,
        Action::Copy,
        Action::Paste,
        Action::Trash,
        Action::Rename,
        Action::NewFile,
        Action::NewFolder,
        Action::ArchiveCreate,
    ] {
        app.action(action);
        assert!(app.dialog.is_none());
    }
    assert!(app.clipboard.is_none());
    assert_eq!(fs::read(&archive).unwrap(), before);
    assert!(render(&mut app, 100, 30).contains("Архив: только чтение"));
    let r = hit(
        &app,
        |t| matches!(t,Target::PathGo{path,..} if path == temp.path()),
    );
    let copy = basalgrim::dnd::DropAction::Copy;
    assert_eq!(
        app.hover_external_drop(501, Some((r.x, r.y).into()), 3, copy),
        Some(copy)
    );
    app.leave_external_drop(501);
    let r = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
    assert!(
        app.hover_external_drop(502, Some((r.x, r.y).into()), 3, copy)
            .is_none()
    );
    let r = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
    app.handle(Input::SecondaryClick { x: r.x, y: r.y });
    let commands: Vec<_> = app.command_items().iter().map(|c| c.action).collect();
    assert_eq!(
        commands,
        vec![
            Action::Open,
            Action::OpenWith,
            Action::Properties,
            Action::ArchiveExtract
        ]
    );
    app.cancel_dialog();
    app.action(Action::ArchiveExtract);
    let out = temp.path().join("chosen");
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text(out.to_string_lossy().into_owned()));
    app.confirm_dialog();
    settle(&mut app);
    assert_eq!(
        fs::read_to_string(out.join("file.txt")).unwrap(),
        "archive preview"
    );
    app.action(Action::Parent);
    settle(&mut app);
    assert_eq!(app.tab().cwd, archive.join("data"));
    app.action(Action::Parent);
    settle(&mut app);
    app.action(Action::Parent);
    settle(&mut app);
    assert!(!app.in_archive());
    assert_eq!(app.tab().cwd, temp.path());
    app.action(Action::Back);
    settle(&mut app);
    assert!(app.in_archive());
}
#[test]
fn stale_archive_scans_cannot_replace_a_new_location_and_refresh_reads_new_contents() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("data");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("one"), "one").unwrap();
    let archive = temp.path().join("data.zip");
    pack(&source, &archive);
    let mut app = App::new(
        archive.join("data"),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    assert_eq!(app.tab().visible.len(), 1);
    fs::write(source.join("two"), "two").unwrap();
    fs::remove_file(&archive).unwrap();
    pack(&source, &archive);
    app.action(Action::Refresh);
    settle(&mut app);
    assert_eq!(app.tab().visible.len(), 2);
    for _ in 0..8 {
        app.navigate(archive.join("data"), true);
        app.navigate(source.clone(), true);
    }
    settle(&mut app);
    assert!(!app.in_archive());
    assert_eq!(app.tab().cwd, source);
}

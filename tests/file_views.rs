use basalgrim::{
    app::{App, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
    view::ViewMode,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    layout::{Position, Rect},
};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    let b = t.backend().buffer();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= w && r.bottom() <= h),
        "hit outside {w}x{h}"
    );
    b.content.iter().map(|c| c.symbol()).collect()
}
fn wait(app: &mut App, done: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        app.tick();
        if done(app) {
            return;
        }
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn settle(app: &mut App) {
    wait(app, |a| !a.loading());
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
fn key(app: &mut App, k: Key) {
    app.handle(Input::Key(KeyPress::plain(k)));
}
fn fixture(mode: ViewMode) -> (tempfile::TempDir, App) {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(
        temp.path().into(),
        Settings {
            view: Some(mode),
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    (temp, app)
}
fn focus(app: &mut App, path: &Path) {
    app.tab_mut().selected = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].path == path)
        .unwrap();
    app.tab_mut().explicit_selection = false;
    app.tab_mut().marked.clear();
}
fn toggle(app: &mut App, path: &Path) {
    render(app, 140, 40);
    let r = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t,Target::TreeToggle {path:p,..} if p==path))
        .unwrap()
        .0;
    click(app, r);
    settle(app);
}
fn names(app: &App) -> Vec<String> {
    app.tab()
        .visible
        .iter()
        .map(|&i| app.tab().entries[i].name.clone())
        .collect()
}
fn entry_rect(app: &App, index: usize) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| matches!(t,Target::Entry{pane:0,index:i} if *i==index).then_some(*r))
        .unwrap()
}
#[test]
fn legacy_settings_and_mouse_mode_chooser_persist_four_distinct_views() {
    let (root, mut app) = fixture(ViewMode::List);
    let path = root.path().join("settings.json");
    for (legacy, mode) in [(true, ViewMode::Icons), (false, ViewMode::List)] {
        fs::write(&path, format!("{{\"grid\":{legacy}}}")).unwrap();
        assert_eq!(Settings::load(&path).unwrap().view_mode(), mode);
    }
    for (w, h) in [(34, 14), (60, 20), (170, 53)] {
        for mode in ViewMode::ALL {
            app.action(Action::View);
            render(&mut app, w, h);
            let r = app
                .hits
                .iter()
                .find_map(|(r, t)| {
                    matches!(t,Target::ViewChoose{mode:m,..} if *m==mode).then_some(*r)
                })
                .unwrap();
            click(&mut app, r);
            assert_eq!(app.settings.view_mode(), mode);
            assert!(app.dialog.is_none());
            render(&mut app, w, h);
            app.persist_preferences(Some(&path));
            assert_eq!(Settings::load(&path).unwrap().view_mode(), mode);
        }
    }
    app.action(Action::View);
    render(&mut app, 34, 14);
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::ViewColumns { delta: 1, .. }).then_some(*r))
        .unwrap();
    click(&mut app, r);
    assert_eq!(app.settings.view_columns, 3);
    app.handle(Input::Pointer(Some(Position::new(0, 0))));
    assert!(app.hovered_area().is_none());
    key(&mut app, Key::Escape);
    app.persist_preferences(Some(&path));
    assert_eq!(Settings::load(&path).unwrap().view_columns, 3);
}
#[test]
fn horizontal_columns_fill_down_then_right_scroll_without_selection_and_reflow() {
    let (root, mut app) = fixture(ViewMode::Columns);
    for i in 0..100 {
        fs::write(root.path().join(format!("file_{i:03}.txt")), "x").unwrap();
    }
    app.action(Action::Refresh);
    settle(&mut app);
    for count in [1, 2, 4, 16] {
        app.settings.view_columns = count;
        render(&mut app, 140, 32);
        let rows = app.tab().column_rows;
        assert!(rows > 1);
        let columns = app.tab().columns;
        assert_eq!(app.tab().page_size, rows * columns);
        let a = entry_rect(&app, 0);
        let b = entry_rect(&app, 1);
        assert_eq!(a.x, b.x);
        assert_eq!(b.y, a.y + 1);
        if columns > 1 {
            let c = entry_rect(&app, rows);
            assert!(c.x > a.x);
            assert_eq!(c.y, a.y);
        }
        app.handle(Input::Scroll {
            x: a.x,
            y: a.y,
            delta: 1,
        });
        render(&mut app, 140, 32);
        assert_eq!(app.tab().selected, 0);
        assert_eq!(
            app.tab().offset,
            if app.tab().visible.len().div_ceil(rows) > columns {
                rows
            } else {
                0
            }
        );
        assert!(!app.tab().follow_selection);
        let offset = app.tab().offset;
        render(&mut app, 140, 32);
        assert_eq!(app.tab().offset, offset);
        key(&mut app, Key::Right);
        assert_eq!(app.tab().selected, rows);
        render(&mut app, 140, 32);
        key(&mut app, Key::Down);
        assert_eq!(app.tab().selected, rows + 1);
        app.handle(Input::Key(KeyPress {
            shift: true,
            ..KeyPress::plain(Key::Down)
        }));
        assert_eq!(app.tab().targets().len(), 2);
        let selected = app.tab().entry().unwrap().path.clone();
        render(&mut app, 70, 24);
        assert_eq!(app.tab().entry().unwrap().path, selected);
        assert_eq!(app.settings.view_columns, count);
        app.tab_mut().selected = 0;
        app.tab_mut().offset = 0;
        app.tab_mut().marked.clear();
        app.tab_mut().explicit_selection = false;
        app.tab_mut().follow_selection = true;
    }
    app.select_view(ViewMode::List);
    render(&mut app, 170, 53);
    let xs: std::collections::BTreeSet<_> = app
        .hits
        .iter()
        .filter_map(|(r, t)| matches!(t, Target::Entry { .. }).then_some(r.x))
        .collect();
    assert_eq!(xs.len(), 1);
    assert_eq!(app.tab().columns, 1);
    assert_eq!(app.tab().column_rows, 0);
}
#[test]
fn lazy_tree_expands_sorts_each_folder_and_collapses_focus_marks_and_stale_hits() {
    let (root, mut app) = fixture(ViewMode::Tree);
    let folder = root.path().join("b-folder");
    let nested = folder.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(folder.join("z-child"), "x").unwrap();
    fs::write(nested.join("leaf"), "x").unwrap();
    fs::write(root.path().join("a-file"), "x").unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    assert_eq!(names(&app), ["b-folder", "a-file"]);
    render(&mut app, 140, 40);
    let stale = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t,Target::TreeToggle{path,..} if path==&folder))
        .unwrap()
        .clone();
    toggle(&mut app, &folder);
    assert_eq!(names(&app), ["b-folder", "nested", "z-child", "a-file"]);
    toggle(&mut app, &nested);
    assert_eq!(
        names(&app),
        ["b-folder", "nested", "leaf", "z-child", "a-file"]
    );
    app.hits = vec![stale.clone()];
    click(&mut app, stale.0);
    assert!(
        names(&app).contains(&"leaf".into()),
        "stale expansion collapsed a changed tree"
    );
    focus(&mut app, &nested.join("leaf"));
    app.tab_mut().marked.insert(nested.join("leaf"));
    app.tab_mut().explicit_selection = true;
    toggle(&mut app, &folder);
    assert_eq!(app.tab().entry().unwrap().path, folder);
    assert!(app.tab().marked.is_empty());
    assert!(app.tab().targets().is_empty());
    assert_eq!(names(&app), ["b-folder", "a-file"]);
    toggle(&mut app, &folder);
    focus(&mut app, &folder);
    key(&mut app, Key::Right);
    assert_eq!(app.tab().entry().unwrap().path, nested);
    key(&mut app, Key::Left);
    assert_eq!(app.tab().entry().unwrap().path, folder);
    app.select_view(ViewMode::List);
    assert_eq!(names(&app), ["b-folder", "a-file"]);
    assert!(app.tab().tree.is_none());
}
#[test]
fn tree_watch_nested_rename_duplicate_and_parent_child_transfers_use_real_paths() {
    let (root, mut app) = fixture(ViewMode::Tree);
    let folder = root.path().join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("old.txt");
    fs::write(&file, "preserved").unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    toggle(&mut app, &folder);
    let external = folder.join("external.txt");
    fs::write(&external, "new").unwrap();
    wait(&mut app, |a| {
        a.tab().entries.iter().any(|e| e.path == external)
    });
    focus(&mut app, &file);
    app.action(Action::Rename);
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("renamed.txt".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    let renamed = folder.join("renamed.txt");
    assert!(renamed.exists());
    assert!(!file.exists());
    assert_eq!(app.tab().entry().unwrap().path, renamed);
    app.action(Action::Duplicate);
    settle(&mut app);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 3);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    app.tab_mut().marked = [folder.clone(), renamed.clone()].into_iter().collect();
    app.tab_mut().explicit_selection = true;
    assert_eq!(app.tab().targets(), std::slice::from_ref(&folder));
    app.action(Action::Copy);
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        std::slice::from_ref(&folder)
    );
    focus(&mut app, &folder);
    app.action(Action::Rename);
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("renamed-folder".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    let renamed_folder = root.path().join("renamed-folder");
    assert!(renamed_folder.join("renamed.txt").exists());
    assert!(
        app.tab()
            .tree
            .as_ref()
            .unwrap()
            .branches
            .contains_key(&renamed_folder)
    );
    assert!(
        app.tab()
            .entries
            .iter()
            .any(|e| e.path == renamed_folder.join("renamed.txt"))
    );
}
#[test]
fn tree_rejects_symlink_replacement_and_late_results_after_collapse_or_navigation() {
    use std::os::unix::fs::symlink;
    let (root, mut app) = fixture(ViewMode::Tree);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("must-not-read"), "x").unwrap();
    let branch = root.path().join("branch");
    fs::create_dir(&branch).unwrap();
    fs::write(branch.join("inside"), "x").unwrap();
    symlink(outside.path(), root.path().join("link")).unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    render(&mut app, 140, 40);
    assert!(
        !app.hits.iter().any(
            |(_, t)| matches!(t,Target::TreeToggle{path,..} if path==&root.path().join("link"))
        )
    );
    fs::rename(&branch, root.path().join("old-branch")).unwrap();
    symlink(outside.path(), &branch).unwrap();
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t,Target::TreeToggle{path,..} if path==&branch).then_some(*r))
        .unwrap();
    click(&mut app, r);
    settle(&mut app);
    assert!(!names(&app).contains(&"must-not-read".into()));
    app.navigate(outside.path().into(), true);
    settle(&mut app);
    assert_eq!(names(&app), ["must-not-read"]);
    assert!(app.tab().tree.as_ref().unwrap().branches.is_empty());
}

#[test]
fn nested_drag_moves_to_the_current_root_and_branch_reads_wait_for_dialogs() {
    let (root, mut app) = fixture(ViewMode::Tree);
    let folder = root.path().join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("nested.txt");
    fs::write(&file, "keep me").unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    toggle(&mut app, &folder);
    app.action(Action::View);
    let later = folder.join("later.txt");
    fs::write(&later, "watch me").unwrap();
    for _ in 0..30 {
        app.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!app.tab().entries.iter().any(|e| e.path == later));
    key(&mut app, Key::Escape);
    wait(&mut app, |a| {
        a.tab().entries.iter().any(|e| e.path == later)
    });
    focus(&mut app, &file);
    render(&mut app, 140, 40);
    let r = entry_rect(&app, app.tab().selected);
    let area = app.file_areas[0];
    app.handle(Input::PointerDown {
        x: r.x + 10,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: area.x + 8,
        y: area.bottom() - 2,
    });
    assert_eq!(
        app.drag_feedback().unwrap().destination.as_deref(),
        Some(root.path())
    );
    app.handle(Input::PointerUp {
        x: area.x + 8,
        y: area.bottom() - 2,
    });
    settle(&mut app);
    assert_eq!(
        fs::read_to_string(root.path().join("nested.txt")).unwrap(),
        "keep me"
    );
    assert!(!file.exists());
}

#[test]
fn archive_tree_stays_read_only_and_minimum_size_controls_reach_nested_entries() {
    use basalgrim::{
        archive::Format,
        fs::{Operation, operate},
    };
    let (root, mut app) = fixture(ViewMode::Tree);
    let folder = root.path().join("folder");
    fs::create_dir_all(folder.join("inner")).unwrap();
    fs::write(folder.join("inner/leaf.txt"), "archive").unwrap();
    let archive = root.path().join("data.zip");
    assert!(
        operate(Operation::ArchiveCreate {
            sources: vec![folder],
            destination: archive.clone(),
            format: Format::Zip
        })
        .error
        .is_none()
    );
    app.navigate(archive.clone(), true);
    settle(&mut app);
    assert!(app.tab().archive.is_some());
    for path in [archive.join("folder"), archive.join("folder/inner")] {
        focus(&mut app, &path);
        render(&mut app, 34, 14);
        let r = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t,Target::TreeToggle{path:p,..} if p==&path).then_some(*r))
            .unwrap();
        click(&mut app, r);
        settle(&mut app);
    }
    let path = archive.join("folder/inner/leaf.txt");
    focus(&mut app, &path);
    assert!(render(&mut app, 34, 14).contains("leaf.txt"));
    app.action(Action::Rename);
    assert!(app.dialog.is_none());
    assert!(archive.exists());
}

#[test]
fn column_drag_autoscrolls_sideways_and_mouse_buttons_keep_selection() {
    let (root, mut app) = fixture(ViewMode::Columns);
    for i in 0..100 {
        fs::write(root.path().join(format!("{i:03}.txt")), "x").unwrap();
    }
    app.settings.view_columns = 2;
    app.action(Action::Refresh);
    settle(&mut app);
    render(&mut app, 140, 32);
    let selected = app.tab().entry().unwrap().path.clone();
    let r = entry_rect(&app, 0);
    let area = app.file_areas[0];
    app.handle(Input::PointerDown {
        x: r.x + 5,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: area.right() - 1,
        y: r.y + 1,
    });
    std::thread::sleep(Duration::from_millis(130));
    app.tick();
    render(&mut app, 140, 32);
    assert!(
        app.tab().offset > 0,
        "dragging across the right edge did not scroll columns"
    );
    assert_eq!(app.tab().entry().unwrap().path, selected);
    key(&mut app, Key::Escape);
    assert!(!app.dragging_files());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 100);
    render(&mut app, 140, 32);
    let before = app.tab().offset;
    let button = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::FileColumns { delta: 1, .. }).then_some(*r))
        .unwrap();
    click(&mut app, button);
    render(&mut app, 140, 32);
    assert!(app.tab().offset > before);
    assert_eq!(app.tab().entry().unwrap().path, selected);
    assert!(app.clipboard.is_none());
}

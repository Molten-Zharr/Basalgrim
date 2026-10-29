use basalgrim::{
    app::{App, Target},
    input::{Action, Input},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn render(app: &mut App) {
    Terminal::new(TestBackend::new(100, 32))
        .unwrap()
        .draw(|frame| ui::render(app, frame))
        .unwrap();
}
fn wait(app: &mut App, description: &str, predicate: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        app.tick();
        render(app);
        if predicate(app) {
            break;
        }
        assert!(Instant::now() < deadline, "{description}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pump(app: &mut App, millis: u64) {
    let deadline = Instant::now() + Duration::from_millis(millis);
    while Instant::now() < deadline {
        app.tick();
        render(app);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn app(path: &Path, grid: bool) -> App {
    let mut app = App::new(
        path.into(),
        Settings {
            grid,
            preview: false,
            ..Settings::default()
        },
    );
    wait(&mut app, "initial scan", |a| !a.loading());
    // Let the watch registration's reconciliation scan finish too.
    pump(&mut app, 400);
    app
}
fn has(app: &App, name: &str) -> bool {
    app.tab().entries.iter().any(|entry| entry.name == name)
}

#[test]
fn external_create_write_atomic_replace_rename_and_delete_refresh_without_input() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path(), false);
    let path = root.path().join("alpha.txt");
    let before = "before\n".repeat(80);
    fs::write(&path, before).unwrap();
    wait(&mut app, "create", |a| has(a, "alpha.txt"));
    app.action(Action::Preview);
    wait(&mut app, "preview", |a| {
        a.preview
            .as_ref()
            .is_some_and(|p| p.lines.iter().any(|s| s.contains("before")))
    });
    app.preview_offset = 10;
    let after = "after: larger content\n".repeat(80);
    fs::write(&path, &after).unwrap();
    wait(&mut app, "same-path write and preview", |a| {
        a.tab()
            .entry()
            .is_some_and(|e| e.size == after.len() as u64)
            && a.preview
                .as_ref()
                .is_some_and(|p| p.lines.iter().any(|s| s.contains("larger content")))
    });
    assert_eq!(app.preview_offset, 10);
    let replacement = root.path().join("replacement");
    fs::write(&replacement, "atomic replacement").unwrap();
    fs::rename(replacement, &path).unwrap();
    wait(&mut app, "atomic replacement", |a| {
        a.preview
            .as_ref()
            .is_some_and(|p| p.lines.iter().any(|s| s.contains("atomic replacement")))
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        app.preview = None;
        app.preview_error = Some("Previous read failed".into());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        wait(&mut app, "permission-only change", |a| {
            a.preview_error.is_none()
                && a.preview
                    .as_ref()
                    .is_some_and(|p| p.details.iter().any(|s| s.contains("0600")))
        });
    }
    fs::rename(&path, root.path().join("renamed.txt")).unwrap();
    wait(&mut app, "rename", |a| {
        has(a, "renamed.txt") && !has(a, "alpha.txt")
    });
    fs::remove_file(root.path().join("renamed.txt")).unwrap();
    wait(&mut app, "delete", |a| {
        a.tab().entries.is_empty() && a.preview.is_none()
    });
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let raw = root
            .path()
            .join(std::ffi::OsString::from_vec(b"invalid-\xff".to_vec()));
        fs::write(&raw, "x").unwrap();
        wait(&mut app, "non-UTF-8 event path", |a| {
            a.tab().entries.iter().any(|e| e.path == raw)
        });
        fs::remove_file(raw).unwrap();
        wait(&mut app, "non-UTF-8 deletion", |a| {
            a.tab().entries.is_empty()
        });
    }
}

#[test]
fn refresh_preserves_paths_marks_filter_sort_and_manual_viewport_in_grid_and_list() {
    for grid in [false, true] {
        let root = tempfile::tempdir().unwrap();
        for i in 0..100 {
            fs::write(root.path().join(format!("file-{i:03}")), "x").unwrap();
        }
        let mut app = app(root.path(), grid);
        let selected = root.path().join("file-002");
        app.tab_mut().selected = 2;
        app.tab_mut()
            .marked
            .extend([selected.clone(), root.path().join("file-099")]);
        app.tab_mut().explicit_selection = true;
        app.tab_mut().filter = "file-".into();
        let area = app.file_areas[0];
        app.handle(Input::Scroll {
            x: area.x + 1,
            y: area.y + 1,
            delta: 5,
        });
        render(&mut app);
        let marks = app.tab().marked.clone();
        let top = app.tab().entries[app.tab().visible[app.tab().offset]]
            .path
            .clone();
        let columns = app.tab().columns;
        for i in 0..columns {
            fs::write(root.path().join(format!("file--new-{i:03}")), "x").unwrap();
        }
        wait(&mut app, "insert before viewport", |a| {
            has(a, "file--new-000")
        });
        assert_eq!(app.tab().entry().unwrap().path, selected);
        assert_eq!(app.tab().marked, marks);
        assert_eq!(app.tab().filter, "file-");
        assert!(!app.tab().follow_selection);
        assert_eq!(
            app.tab().entries[app.tab().visible[app.tab().offset]].path,
            top
        );
        assert_eq!(app.active, 0);
        fs::remove_file(&selected).unwrap();
        wait(&mut app, "remove a marked path", |a| !has(a, "file-002"));
        assert_eq!(app.tab().targets(), [root.path().join("file-099")]);
        fs::remove_file(root.path().join("file-099")).unwrap();
        wait(&mut app, "remove last mark", |a| !has(a, "file-099"));
        assert!(app.tab().explicit_selection);
        assert!(app.tab().targets().is_empty());
        fs::write(root.path().join("unmatched"), "x").unwrap();
        wait(&mut app, "filtered change", |a| has(a, "unmatched"));
        assert!(app.tab().targets().is_empty());
        assert!(
            app.tab()
                .visible
                .iter()
                .all(|&i| app.tab().entries[i].name.contains("file-"))
        );
    }
}

#[test]
fn all_open_tabs_and_panes_update_without_switching_keyboard_focus() {
    let root = tempfile::tempdir().unwrap();
    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    let mut app = app(root.path(), false);
    app.action(Action::NewTab);
    app.navigate(other.clone(), false);
    wait(&mut app, "second tab", |a| !a.loading());
    app.action(Action::NextTab);
    pump(&mut app, 400);
    fs::write(other.join("background-tab"), "x").unwrap();
    fs::write(root.path().join("both-panes"), "x").unwrap();
    wait(&mut app, "all open tabs", |a| {
        a.panes[0].tabs[1]
            .entries
            .iter()
            .any(|e| e.name == "background-tab")
            && a.panes
                .iter()
                .all(|p| p.tabs[0].entries.iter().any(|e| e.name == "both-panes"))
    });
    assert_eq!(app.active, 0);
    assert_eq!(app.panes[0].active, 0);
    assert_eq!(app.panes[1].active, 0);
    // Discard old scan results and notifications while navigating rapidly.
    for _ in 0..20 {
        app.navigate(other.clone(), false);
        app.navigate(root.path().into(), false);
    }
    app.action(Action::NewTab);
    app.navigate(other, false);
    app.action(Action::CloseTab);
    app.action(Action::NextTab);
    wait(&mut app, "cancel obsolete scans", |a| !a.loading());
    pump(&mut app, 300);
    assert_eq!(app.tab().cwd, root.path());
    assert!(has(&app, "both-panes"));
    assert!(!has(&app, "background-tab"));
}

#[test]
fn background_results_wait_for_dialogs_and_marquee_without_blanking_the_files() {
    let root = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        fs::write(root.path().join(name), "x").unwrap();
    }
    let mut app = app(root.path(), false);
    fs::write(root.path().join("gamma"), "x").unwrap();
    wait(&mut app, "background scan started", |a| a.tab().refreshing);
    assert!(
        app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::Entry { .. }))
    );
    app.action(Action::Rename);
    pump(&mut app, 350);
    assert!(
        !has(&app, "gamma"),
        "A late result moved the editor's underlying list"
    );
    app.cancel_dialog();
    wait(&mut app, "dialog released update", |a| has(a, "gamma"));
    let area = app.file_areas[0];
    app.handle(Input::PointerDown {
        x: area.right() - 2,
        y: area.y + 1,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: area.x + 1,
        y: area.y,
    });
    let marks = app.tab().marked.clone();
    fs::write(root.path().join("delta"), "x").unwrap();
    pump(&mut app, 400);
    assert!(!has(&app, "delta"));
    assert_eq!(app.tab().marked, marks);
    assert!(app.selection_area().is_some());
    app.handle(Input::PointerUp {
        x: area.x + 1,
        y: area.y,
    });
    wait(&mut app, "marquee released update", |a| has(a, "delta"));
    assert_eq!(app.tab().marked, marks);
}

#[test]
fn deleted_directory_recovers_and_rearms_after_recreation_at_the_same_path() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("watched");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("old"), "x").unwrap();
    let mut app = app(&path, false);
    fs::rename(&path, root.path().join("moved-away")).unwrap();
    wait(&mut app, "directory moved away", |a| {
        a.tab().error.is_some()
    });
    fs::create_dir(&path).unwrap();
    fs::write(path.join("new"), "x").unwrap();
    wait(&mut app, "replacement directory recovered", |a| {
        a.tab().error.is_none() && has(a, "new")
    });
    assert!(!has(&app, "old"));
    fs::write(path.join("watch-rearmed"), "x").unwrap();
    wait(&mut app, "replacement inode is watched", |a| {
        has(a, "watch-rearmed")
    });
}

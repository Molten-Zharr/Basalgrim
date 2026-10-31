use basalgrim::{
    app::{App, Clipboard, SettingsSection, Target},
    input::{Action, Input, Key, KeyPress},
    options::Options,
    session::State,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        app.tick();
        if predicate(app) {
            return;
        }
        assert!(Instant::now() < end, "session timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn settle(app: &mut App) {
    wait(app, |a| !a.loading());
}
fn options(directory: &Path, explicit: bool, no_session: bool) -> Options {
    Options {
        directory: directory.into(),
        explicit_directory: explicit,
        no_session,
    }
}
fn settings() -> Settings {
    Settings {
        dual: true,
        preview: false,
        ..Settings::default()
    }
}
fn folder(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    path
}
fn saved(root: &Path) -> (PathBuf, State, Vec<PathBuf>) {
    let dirs: Vec<_> = (0..4).map(|i| folder(root, &format!("tab-{i}"))).collect();
    let mut app = App::new(dirs[0].clone(), settings());
    settle(&mut app);
    app.action(Action::NewTab);
    app.navigate(dirs[1].clone(), true);
    settle(&mut app);
    app.active = 1;
    app.navigate(dirs[2].clone(), true);
    settle(&mut app);
    app.action(Action::NewTab);
    app.navigate(dirs[3].clone(), true);
    settle(&mut app);
    app.panes[0].active = 0;
    let state = State::capture(&app);
    let path = root.join("session.json");
    state.save(&path).unwrap();
    (path, state, dirs)
}
fn startup(root: &Path, path: &Path, settings: Settings, explicit: bool, no_session: bool) -> App {
    App::startup_with_session(
        options(root, explicit, no_session),
        settings,
        None,
        Some(path.into()),
    )
}

#[test]
fn both_panes_restore_tab_order_active_tabs_and_active_pane_without_transient_file_state() {
    let root = tempfile::tempdir().unwrap();
    let (path, state, dirs) = saved(root.path());
    let mut app = startup(root.path(), &path, settings(), false, false);
    settle(&mut app);
    assert_eq!(State::capture(&app), state);
    assert_eq!(app.active, 1);
    assert_eq!(app.panes[0].tab().cwd, dirs[0]);
    assert_eq!(app.panes[1].tab().cwd, dirs[3]);
    assert_eq!(app.panes[0].tabs[1].cwd, dirs[1]);
    assert_eq!(app.panes[1].tabs[0].cwd, dirs[2]);
    assert!(app.clipboard.is_none() && app.operations.is_empty() && app.dialog.is_none());
    for tab in app.panes.iter().flat_map(|p| &p.tabs) {
        assert!(
            tab.marked.is_empty()
                && !tab.explicit_selection
                && tab.filter.is_empty()
                && tab.back.is_empty()
        );
    }
    app.clipboard = Some(Clipboard {
        paths: vec![root.path().join("secret")],
        cut: true,
    });
    app.tab_mut().filter = "private query".into();
    app.tab_mut()
        .marked
        .insert(root.path().join("private selection"));
    let bytes = serde_json::to_string(&State::capture(&app)).unwrap();
    assert!(!bytes.contains("secret") && !bytes.contains("private"));
}

#[test]
fn explicit_path_disabled_preference_and_no_session_have_distinct_startup_semantics() {
    for mode in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let (path, _state, _dirs) = saved(root.path());
        let original = fs::read(&path).unwrap();
        let mut prefs = settings();
        prefs.restore_session = mode != 1;
        let mut app = startup(root.path(), &path, prefs, mode == 0, mode == 2);
        settle(&mut app);
        assert_eq!(app.active, 0);
        assert!(
            app.panes
                .iter()
                .all(|p| p.tabs.len() == 1 && p.tab().cwd == root.path())
        );
        drop(app);
        if mode == 0 {
            assert_ne!(fs::read(&path).unwrap(), original);
        } else {
            assert_eq!(fs::read(&path).unwrap(), original);
        }
    }
}

#[test]
fn a_hidden_right_pane_retains_its_tabs_but_focus_stays_on_the_visible_left_pane() {
    let root = tempfile::tempdir().unwrap();
    let (path, _, dirs) = saved(root.path());
    let mut prefs = settings();
    prefs.dual = false;
    let mut app = startup(root.path(), &path, prefs, false, false);
    settle(&mut app);
    assert_eq!(app.active, 0);
    assert_eq!(app.panes[1].tabs.len(), 2);
    assert_eq!(app.panes[1].tab().cwd, dirs[3]);
}

#[test]
fn missing_directories_remain_error_tabs_and_other_tabs_continue_working() {
    let root = tempfile::tempdir().unwrap();
    let (path, _, dirs) = saved(root.path());
    fs::remove_dir(&dirs[3]).unwrap();
    let mut app = startup(root.path(), &path, settings(), false, false);
    settle(&mut app);
    assert_eq!(app.tab().cwd, dirs[3]);
    assert!(app.tab().error.is_some());
    app.action(Action::NextTab);
    assert_eq!(app.tab().cwd, dirs[2]);
    assert!(app.tab().error.is_none());
}

#[test]
fn malformed_or_future_sessions_are_preserved_and_never_replaced_by_fallback_state() {
    let valid = serde_json::json!({"version":1,"active":0,"panes":[{"tabs":["/"],"active":0},{"tabs":["/"],"active":0}]});
    let mut invalid = vec![b"{broken".to_vec(), vec![b' '; 1024 * 1024 + 1]];
    for pointer in ["/version", "/active", "/panes/0/active", "/panes/0/tabs"] {
        let mut value = valid.clone();
        *value.pointer_mut(pointer).unwrap() = if pointer.ends_with("tabs") {
            serde_json::json!([])
        } else {
            serde_json::json!(99)
        };
        invalid.push(serde_json::to_vec(&value).unwrap());
    }
    for path in ["relative", "/nul\0path"] {
        let mut value = valid.clone();
        value["panes"][0]["tabs"][0] = path.into();
        invalid.push(serde_json::to_vec(&value).unwrap());
    }
    for bytes in invalid {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.json");
        fs::write(&path, &bytes).unwrap();
        let mut app = startup(root.path(), &path, settings(), false, false);
        settle(&mut app);
        assert!(app.session_load_error.is_some() && app.notice_error);
        assert_eq!(app.tab().cwd, root.path());
        app.action(Action::NewTab);
        drop(app);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn autosave_coalesces_navigation_tab_close_and_pane_changes_and_flushes_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state/session.json");
    let a = folder(root.path(), "a");
    let b = folder(root.path(), "b");
    let mut app = startup(root.path(), &path, settings(), true, false);
    for _ in 0..10 {
        app.navigate(a.clone(), true);
        app.persist_session();
        app.navigate(b.clone(), true);
        app.persist_session();
    }
    app.action(Action::NewTab);
    app.persist_session();
    app.action(Action::CloseTab);
    app.persist_session();
    app.active = 1;
    app.navigate(a.clone(), true);
    app.persist_session();
    let expected = State::capture(&app);
    wait(&mut app, |_| {
        State::load(&path).ok().flatten().as_ref() == Some(&expected)
    });
    app.navigate(b, true); // This final change must survive a close before the next tick.
    let final_state = State::capture(&app);
    drop(app);
    assert_eq!(State::load(&path).unwrap(), Some(final_state));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn the_last_closed_instance_publishes_its_own_final_session() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session.json");
    let a = folder(root.path(), "a");
    let b = folder(root.path(), "b");
    let mut first = startup(&a, &path, settings(), true, false);
    let expected_first = State::capture(&first);
    wait(&mut first, |_| {
        State::load(&path).ok().flatten().as_ref() == Some(&expected_first)
    });
    let mut second = startup(&b, &path, settings(), true, false);
    let expected_second = State::capture(&second);
    wait(&mut second, |_| {
        State::load(&path).ok().flatten().as_ref() == Some(&expected_second)
    });
    drop(first);
    assert_eq!(State::load(&path).unwrap(), Some(expected_first));
    drop(second);
    assert_eq!(State::load(&path).unwrap(), Some(expected_second));
}

#[test]
fn the_session_toggle_is_mouse_accessible_in_compact_settings_and_cancels_a_pending_save() {
    let root = tempfile::tempdir().unwrap();
    let (path, _, _) = saved(root.path());
    let original = fs::read(&path).unwrap();
    let mut app = startup(root.path(), &path, settings(), true, false);
    app.persist_session(); // Enqueued, still within the worker debounce.
    app.action(Action::Settings);
    app.settings_section(SettingsSection::Session);
    let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let row = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Setting { index: 0, .. }).then_some(*r))
        .unwrap();
    app.handle(Input::Click {
        x: row.x,
        y: row.y,
        double: false,
        toggle: false,
    });
    assert!(!app.settings.restore_session);
    drop(app);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn failed_background_saves_can_be_retried_from_settings_and_reenable_saves_current_tabs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state/session.json");
    let mut prefs = settings();
    prefs.restore_session = false;
    let mut app = startup(root.path(), &path, prefs, true, false);
    app.action(Action::Settings);
    app.settings_section(SettingsSection::Session);
    app.setting(0, true, 1); // Reenable without reloading another session into the live UI.
    assert!(app.settings.restore_session);
    fs::write(root.path().join("state"), "blocked").unwrap();
    wait(&mut app, |a| a.session_save_error.is_some());
    assert!(
        app.settings_rows()
            .iter()
            .any(|r| r.label == "Повторить сохранение")
    );
    fs::remove_file(root.path().join("state")).unwrap();
    app.setting(1, true, 1);
    let expected = State::capture(&app);
    wait(&mut app, |a| {
        a.session_save_error.is_none()
            && State::load(&path).ok().flatten().as_ref() == Some(&expected)
    });
}

#[test]
fn session_settings_are_reachable_with_keyboard_and_legacy_settings_enable_restore() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    fs::write(&path, "{}").unwrap();
    assert!(Settings::load(&path).unwrap().restore_session);
    let mut app = App::new(root.path().into(), settings());
    app.action(Action::Settings);
    for key in [Key::Down, Key::Down, Key::Right, Key::Enter] {
        app.handle(Input::Key(KeyPress::plain(key)));
    }
    assert!(!app.settings.restore_session);
    app.persist_preferences(Some(&path));
    assert!(!Settings::load(&path).unwrap().restore_session);
}

#[cfg(unix)]
#[test]
fn non_utf8_directory_paths_round_trip_without_loss_and_session_files_are_private() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let dir = root
        .path()
        .join(std::ffi::OsString::from_vec(b"raw-\xff".to_vec()));
    fs::create_dir(&dir).unwrap();
    let mut app = App::new(dir.clone(), settings());
    settle(&mut app);
    let path = root.path().join("session.json");
    State::capture(&app).save(&path).unwrap();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    let mut restored = startup(root.path(), &path, settings(), false, false);
    settle(&mut restored);
    assert_eq!(restored.tab().cwd, dir);
}

#[cfg(target_os = "linux")]
#[test]
fn nonregular_session_files_and_symlinks_are_rejected_without_blocking_or_following() {
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(State::load(&fifo).is_err());
    let target = root.path().join("target");
    fs::write(&target, "precious").unwrap();
    let link = root.path().join("session.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let app = startup(root.path(), &link, settings(), false, false);
    drop(app);
    assert_eq!(fs::read_to_string(&target).unwrap(), "precious");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
}

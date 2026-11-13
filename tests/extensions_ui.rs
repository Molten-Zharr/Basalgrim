use basalgrim::{
    app::{App, Dialog, ExtensionControl as C, Target},
    extensions::Store,
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

#[test]
fn extension_bindings_migrate_without_resetting_appearance_or_user_shortcuts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut settings = Settings {
        grid: false,
        preview: false,
        ..Default::default()
    };
    settings.appearance.font_size = 22;
    settings
        .bindings
        .retain(|b| !matches!(b.action, Action::Extensions | Action::ExtensionCommands));
    let occupied = KeyPress {
        alt: true,
        ..KeyPress::ctrl('x')
    };
    settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Help)
        .unwrap()
        .key = occupied;
    fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let loaded = Settings::load(&path).unwrap();
    loaded.validate_bindings().unwrap();
    assert!(!loaded.grid && !loaded.preview);
    assert_eq!(loaded.appearance.font_size, 22);
    assert_eq!(loaded.action_for(occupied), Some(Action::Help));
    assert!(
        loaded
            .bindings
            .iter()
            .any(|b| b.action == Action::Extensions && b.key != occupied)
    );
    assert!(
        loaded
            .bindings
            .iter()
            .any(|b| b.action == Action::ExtensionCommands)
    );
    Settings::default().validate_bindings().unwrap();
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end, "{}", app.extensions.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn render(app: &mut App) -> String {
    let mut t = Terminal::new(TestBackend::new(34, 14)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
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
        .rev()
        .find_map(|(r, t)| f(t).then_some(*r))
        .unwrap()
}
fn click(app: &mut App, r: Rect, double: bool) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double,
        toggle: false,
    });
}
#[test]
fn compact_mouse_install_enable_context_run_stale_rows_disable_and_remove() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("package");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("basalgrim-plugin.json"),r#"{"api":1,"id":"ui","name":"Плагин","version":"1","executable":"run.py","commands":[{"id":"run","name":"Проверка","contexts":["file"],"extensions":["json"]}]}"#).unwrap();
    fs::write(source.join("run.py"),"#!/usr/bin/python3\nimport json,sys,pathlib\nr=json.load(sys.stdin)\npathlib.Path('result').write_text('exact')\nprint(json.dumps({'message':'Успешно'}))\n").unwrap();
    fs::set_permissions(source.join("run.py"), fs::Permissions::from_mode(0o700)).unwrap();
    let root = temp.path().join("files");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.json"), "{}").unwrap();
    let mut app = App::new(
        root.clone(),
        Settings {
            preview: false,
            grid: false,
            ..Default::default()
        },
    );
    app.configure_extensions(Store {
        root: temp.path().join("extensions"),
    });
    settle(&mut app);
    app.action(Action::Extensions);
    assert!(render(&mut app).contains("Установить"));
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Install,
                ..
            }
        )
    });
    click(&mut app, r, false);
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Text(source.to_string_lossy().into_owned()));
    render(&mut app);
    let r = hit(&app, |t| matches!(t, Target::Confirm));
    click(&mut app, r, false);
    settle(&mut app);
    assert!(!app.extensions.catalog[0].enabled);
    render(&mut app);
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Toggle,
                ..
            }
        )
    });
    click(&mut app, r, false);
    settle(&mut app);
    assert!(app.extensions.catalog[0].enabled);
    render(&mut app);
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Commands,
                ..
            }
        )
    });
    click(&mut app, r, false);
    assert!(render(&mut app).contains("Проверка"));
    let (token, revision) = {
        let v = app.extension_view.as_ref().unwrap();
        (v.token, v.revision)
    };
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Row(0),
                ..
            }
        )
    });
    click(&mut app, r, false);
    assert!(!root.join("result").exists());
    click(&mut app, r, true);
    settle(&mut app);
    assert_eq!(fs::read_to_string(root.join("result")).unwrap(), "exact");
    fs::remove_file(root.join("result")).unwrap();
    app.extension_control(token, revision, C::Run, false);
    settle(&mut app);
    assert!(!root.join("result").exists());
    app.cancel_dialog();
    assert!(app.dialog.is_none());
    app.open_extensions(false, false);
    render(&mut app);
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Toggle,
                ..
            }
        )
    });
    click(&mut app, r, false);
    settle(&mut app);
    assert!(!app.has_extension_commands(false));
    render(&mut app);
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Extension {
                control: C::Remove,
                ..
            }
        )
    });
    click(&mut app, r, false);
    settle(&mut app);
    assert!(app.extensions.catalog.is_empty());
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(!matches!(app.dialog, Some(Dialog::Extensions)));
}

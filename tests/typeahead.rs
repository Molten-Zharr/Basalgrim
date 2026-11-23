use basalgrim::{
    app::{App, Dialog, SettingsSection, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
    view::ViewMode,
};
use ratatui::{Terminal, backend::TestBackend, style::Modifier};
use std::{
    fs,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    for name in ["alpha.txt", "beta.txt", "Фото тест.png"] {
        fs::write(root.path().join(name), "fixture").unwrap();
    }
    let mut app = App::new(
        root.path().into(),
        Settings {
            preview: false,
            typeahead_timeout_ms: 2000,
            ..Default::default()
        },
    );
    settle(&mut app);
    (root, app)
}
fn render(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= w && r.bottom() <= h)
    );
    t.backend().buffer().clone()
}
fn click(app: &mut App, test: impl Fn(&Target) -> bool) {
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| test(t).then_some(*r))
        .unwrap();
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
#[test]
fn every_view_highlights_matching_file_without_hiding_entries_at_minimum_size() {
    for mode in [
        ViewMode::Icons,
        ViewMode::List,
        ViewMode::Columns,
        ViewMode::Tree,
    ] {
        let (_root, mut app) = fixture();
        app.settings.view = Some(mode);
        render(&mut app, 34, 14);
        let count = app.tab().visible.len();
        app.handle(Input::Text("BeT".into()));
        assert_eq!(app.tab().entry().unwrap().name, "beta.txt");
        assert_eq!(app.tab().visible.len(), count);
        let buffer = render(&mut app, 34, 14);
        let text = buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("Набор: BeT"), "{mode:?}: {text}");
        assert!(buffer.content.iter().any(
            |c| c.symbol() == "b" && c.modifier.contains(Modifier::BOLD | Modifier::UNDERLINED)
        ));
        click(&mut app, |t| matches!(t, Target::ClearTypeahead));
        assert!(app.typeahead().is_none());
        assert_eq!(app.tab().entry().unwrap().name, "beta.txt");
    }
}
#[test]
fn no_match_unicode_substring_modal_navigation_and_paste_are_isolated() {
    let (root, mut app) = fixture();
    app.handle(Input::Text("фОто".into()));
    assert_eq!(app.tab().entry().unwrap().name, "Фото тест.png");
    app.handle(Input::Text(" !".into()));
    assert!(!app.typeahead().unwrap().matched);
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    app.handle(Input::Text("тест".into()));
    assert!(app.typeahead().unwrap().matched);
    app.handle(Input::CancelDrag);
    assert!(
        app.typeahead().is_some(),
        "Pointer loss must not erase typed text"
    );
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(app.typeahead().is_none());
    app.handle(Input::Paste("alpha".into()));
    assert!(app.typeahead().is_none());
    app.action(Action::Search);
    app.handle(Input::Text("alpha".into()));
    assert!(app.typeahead().is_none());
    assert_eq!(app.tab().filter, "alpha");
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    app.handle(Input::Text("a".into()));
    app.action(Action::NewTab);
    assert!(app.typeahead().is_none());
    settle(&mut app);
    app.handle(Input::Text("a".into()));
    app.navigate(root.path().parent().unwrap().into(), true);
    assert!(app.typeahead().is_none());
}
#[test]
fn timeout_setting_has_compact_mouse_editor_validation_and_persistence() {
    let (root, mut app) = fixture();
    app.action(Action::Settings);
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(t, Target::SettingsSection(SettingsSection::Navigation))
    });
    for (input, expected) in [("0,2", 200), ("2", 2000)] {
        render(&mut app, 34, 14);
        click(&mut app, |t| {
            matches!(
                t,
                Target::Setting {
                    index: 0,
                    activate: true,
                    ..
                }
            )
        });
        app.handle(Input::Text(input.into()));
        app.handle(Input::Key(KeyPress::plain(Key::Enter)));
        assert!(matches!(app.dialog, Some(Dialog::Settings)));
        assert_eq!(app.settings.typeahead_timeout_ms, expected);
    }
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(
            t,
            Target::Setting {
                index: 0,
                activate: true,
                ..
            }
        )
    });
    app.handle(Input::Text("NaN".into()));
    app.handle(Input::Key(KeyPress::plain(Key::Enter)));
    assert!(matches!(
        &app.dialog,
        Some(Dialog::Edit { error: Some(_), .. })
    ));
    assert_eq!(app.settings.typeahead_timeout_ms, 2000);
    let path = root.path().join("settings.json");
    app.persist_preferences(Some(&path));
    assert_eq!(Settings::load(&path).unwrap().typeahead_timeout_ms, 2000);
    fs::write(&path, "{}").unwrap();
    assert_eq!(Settings::load(&path).unwrap().typeahead_timeout_ms, 1000);
    fs::write(&path, r#"{"typeahead_timeout_ms":0}"#).unwrap();
    assert_eq!(Settings::load(&path).unwrap().typeahead_timeout_ms, 100);
}
#[cfg(feature = "tui")]
#[test]
fn terminal_adapter_preserves_composed_text_spaces_and_command_bindings() {
    use basalgrim::tui_input;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (_root, mut app) = fixture();
    for c in "ФоТо тест".chars() {
        tui_input::key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    assert_eq!(app.typeahead().unwrap().text, "ФоТо тест");
    assert!(app.typeahead().unwrap().matched);
    tui_input::key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    assert!(app.typeahead().is_none());
    assert!(app.clipboard.is_some());
    tui_input::key(
        &mut app,
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
    );
    assert!(matches!(app.dialog, Some(Dialog::Edit { .. })));
    assert!(app.typeahead().is_none());
}

#[test]
fn parent_default_migrates_once_without_overwriting_custom_bindings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let plain = KeyPress::plain(Key::Backspace);
    let control = KeyPress {
        ctrl: true,
        ..plain
    };
    let defaults = Settings::default();
    assert_eq!(defaults.action_for(plain), None);
    assert_eq!(defaults.action_for(control), Some(Action::Parent));
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    legacy.as_object_mut().unwrap().remove("bindings_version");
    for binding in legacy["bindings"].as_array_mut().unwrap() {
        if binding["action"] == "Parent" && binding["key"]["key"] == "Backspace" {
            binding["key"]["ctrl"] = false.into();
        }
    }
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut upgraded = Settings::load(&path).unwrap();
    assert_eq!(upgraded.action_for(plain), None);
    assert_eq!(upgraded.action_for(control), Some(Action::Parent));
    assert_eq!(upgraded.bindings, defaults.bindings);
    upgraded.save(&path).unwrap();
    assert_eq!(
        Settings::load(&path).unwrap().bindings_version,
        defaults.bindings_version
    );
    // Explicit changes made after migration remain user choices on restart.
    upgraded
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Parent)
        .unwrap()
        .key = plain;
    upgraded.save(&path).unwrap();
    assert_eq!(
        Settings::load(&path).unwrap().action_for(plain),
        Some(Action::Parent)
    );
    // An existing custom Ctrl+Backspace keeps its command. Parent retains Alt+Up.
    for binding in legacy["bindings"].as_array_mut().unwrap() {
        if binding["action"] == "Copy" {
            binding["key"] = serde_json::to_value(control).unwrap();
        }
    }
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let upgraded = Settings::load(&path).unwrap();
    assert_eq!(upgraded.action_for(plain), None);
    assert_eq!(upgraded.action_for(control), Some(Action::Copy));
    assert_eq!(
        upgraded.action_for(KeyPress {
            alt: true,
            ..KeyPress::plain(Key::Up)
        }),
        Some(Action::Parent)
    );
    // A legacy file with no alternative Parent still gets an available binding.
    legacy["bindings"]
        .as_array_mut()
        .unwrap()
        .retain(|b| b["action"] != "Parent" || b["key"]["key"] == "Backspace");
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let upgraded = Settings::load(&path).unwrap();
    assert_eq!(upgraded.action_for(plain), None);
    assert_eq!(upgraded.action_for(control), Some(Action::Copy));
    assert_eq!(
        upgraded.action_for(KeyPress {
            alt: true,
            ..KeyPress::plain(Key::Up)
        }),
        Some(Action::Parent)
    );
}

use basalgrim::{
    app::{App, Dialog, EditKind, SettingsFocus, SettingsSection, SettingsTab, Target},
    input::{self, Action, Input, Key, KeyPress},
    preferences::{BorderWeight, Corners, FontFace, IconMode, Theme},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::{fs, path::Path};

fn app() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let app = App::new(root.path().to_path_buf(), Settings::default());
    (root, app)
}
fn key(app: &mut App, key: Key) {
    app.handle(Input::Key(KeyPress::plain(key)));
}
fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|c| c.symbol()).collect()
}
fn binding_index(app: &App, action: Action) -> usize {
    app.settings
        .bindings
        .iter()
        .position(|b| b.action == action)
        .unwrap()
}
fn capture(app: &mut App, action: Action) {
    let index = binding_index(app, action);
    app.dialog = Some(Dialog::CaptureBinding { index, error: None });
}
fn page(app: &mut App, tab: SettingsTab) {
    app.dialog = Some(Dialog::Settings);
    app.settings_tab(tab);
    app.settings_view.focus = SettingsFocus::Parameters;
}

#[test]
fn legacy_settings_keep_font_size_and_receive_default_themes_and_bindings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    fs::write(
        &path,
        r#"{"grid":false,"preview":false,"dual":true,"hidden":true,"font_size":24}"#,
    )
    .unwrap();
    let settings = Settings::load(&path).unwrap();
    assert!(!settings.grid);
    assert!(!settings.preview);
    assert!(settings.dual && settings.hidden);
    assert_eq!(settings.appearance.font_size, 24);
    assert_eq!(settings.appearance.theme, Theme::MoltenZharr);
    assert_eq!(settings.appearance.icon_mode, IconMode::Images);
    assert_eq!(settings.bindings, input::default_bindings());
    settings.save(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(json["font_size"], 24);
    assert!(json.get("appearance").is_none());
}

#[test]
fn icon_mode_is_editable_and_persists_independently_of_backend_capabilities() {
    use basalgrim::icons::IconBackend;

    let (root, mut app) = app();
    let path = root.path().join("settings.json");
    page(&mut app, SettingsTab::Icons);
    let buffer = render(&mut app, 120, 36);
    assert!(text(&buffer).contains("Иконки файлов"));
    assert!(
        app.settings_rows()[0]
            .description
            .contains("не поддерживает")
    );
    key(&mut app, Key::Right);
    assert_eq!(app.settings.appearance.icon_mode, IconMode::Symbols);
    app.persist_preferences(Some(&path));
    assert_eq!(
        Settings::load(&path).unwrap().appearance.icon_mode,
        IconMode::Symbols
    );
    app.icon_backend = IconBackend::Gui;
    assert!(app.settings_rows()[0].description.contains("В окне"));
    assert_eq!(app.settings.appearance.icon_mode, IconMode::Symbols);
    let button = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(
                target,
                Target::Setting {
                    index: 0,
                    activate: false,
                    ..
                }
            )
            .then_some(*area)
        })
        .unwrap();
    app.handle(Input::Click {
        x: button.x,
        y: button.y,
        double: false,
        toggle: false,
    });
    assert_eq!(app.settings.appearance.icon_mode, IconMode::Images);
    app.persist_preferences(Some(&path));
    let restored = Settings::load(&path).unwrap();
    assert_eq!(restored.appearance.icon_mode, IconMode::Images);
    app.icon_backend = IconBackend::Symbols;
    render(&mut app, 120, 36);
    assert_eq!(app.settings.appearance.icon_mode, IconMode::Images);
    assert!(app.icon_slots.is_empty());
}

#[test]
fn keyboard_settings_navigation_changes_theme_and_font_without_affecting_files() {
    let (root, mut app) = app();
    fs::write(root.path().join("keep"), "unchanged").unwrap();
    key(&mut app, Key::F(10));
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
    key(&mut app, Key::Tab);
    key(&mut app, Key::Tab);
    key(&mut app, Key::Down);
    key(&mut app, Key::Enter);
    assert_eq!(app.settings.appearance.theme, Theme::ClassicAmber);
    app.settings_tab(SettingsTab::Typography);
    key(&mut app, Key::Tab);
    key(&mut app, Key::Right);
    assert_eq!(app.settings.appearance.font, FontFace::FiraCodeBold);
    key(&mut app, Key::Down);
    key(&mut app, Key::Right);
    assert_eq!(app.settings.appearance.font_size, 17);
    key(&mut app, Key::Escape);
    assert!(app.dialog.is_none());
    assert!(!app.busy);
    assert_eq!(
        fs::read_to_string(root.path().join("keep")).unwrap(),
        "unchanged"
    );
}

#[test]
fn palette_hex_editor_validates_cancel_and_keeps_overrides_per_theme() {
    let (_, mut app) = app();
    page(&mut app, SettingsTab::Palette);
    app.setting(0, true, 1);
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("#XYZ123".into()));
    key(&mut app, Key::Enter);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit { error: Some(_), .. })
    ));
    assert!(app.settings.appearance.colors.is_empty());
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("#123456".into()));
    key(&mut app, Key::Enter);
    assert_eq!(
        app.settings.appearance.palette()["background"],
        [0x12, 0x34, 0x56]
    );
    app.settings.appearance.theme = Theme::ClassicViolet;
    assert_ne!(
        app.settings.appearance.palette()["background"],
        [0x12, 0x34, 0x56]
    );
    app.settings.appearance.theme = Theme::MoltenZharr;
    app.setting(0, true, 1);
    app.handle(Input::Text("junk".into()));
    key(&mut app, Key::Escape);
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
    assert_eq!(app.settings_view.tab, SettingsTab::Palette);
    assert_eq!(
        app.settings.appearance.palette()["background"],
        [0x12, 0x34, 0x56]
    );
    app.setting(16, true, 1);
    assert_eq!(app.settings.appearance.palette()["background"], [0, 0, 0]);
}

#[test]
fn font_size_editor_accepts_only_the_supported_range() {
    let (_, mut app) = app();
    page(&mut app, SettingsTab::Typography);
    app.setting(1, true, 1);
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("99".into()));
    key(&mut app, Key::Enter);
    assert_eq!(app.settings.appearance.font_size, 16);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Setting(_),
            error: Some(_),
            ..
        })
    ));
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("32".into()));
    key(&mut app, Key::Enter);
    assert_eq!(app.settings.appearance.font_size, 32);
    app.setting(1, false, 1);
    assert_eq!(app.settings.appearance.font_size, 32);
}

#[test]
fn shortcut_capture_rejects_conflicts_and_navigation_and_never_dispatches_quit() {
    let (_, mut app) = app();
    capture(&mut app, Action::Settings);
    let original = app.settings.bindings.clone();
    for press in [
        KeyPress::plain(Key::Up),
        KeyPress::plain(Key::F(1)),
        KeyPress::ctrl('q'),
        KeyPress::ctrl('u'),
    ] {
        app.handle(Input::Key(press));
        assert!(!app.quit);
        assert_eq!(app.settings.bindings, original);
        assert!(matches!(
            app.dialog,
            Some(Dialog::CaptureBinding { error: Some(_), .. })
        ));
    }
    key(&mut app, Key::Escape);
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
    capture(&mut app, Action::Settings);
    let assigned = KeyPress {
        alt: true,
        ..KeyPress::ctrl('о')
    };
    app.handle(Input::Key(assigned));
    assert_eq!(
        app.settings.bindings[binding_index(&app, Action::Settings)].key,
        KeyPress {
            alt: true,
            ..KeyPress::ctrl('j')
        }
    );
    key(&mut app, Key::Escape);
    key(&mut app, Key::F(10));
    assert!(app.dialog.is_none());
    app.handle(Input::Key(assigned));
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
}

#[test]
fn modifier_matching_is_exact_and_reassigned_keys_update_help_and_command_bar() {
    let (_, mut app) = app();
    let grid = app.settings.grid;
    app.handle(Input::Key(KeyPress {
        shift: true,
        ..KeyPress::ctrl('g')
    }));
    assert_eq!(app.settings.grid, grid);
    capture(&mut app, Action::Rename);
    key(&mut app, Key::F(4));
    key(&mut app, Key::Escape);
    let buffer = render(&mut app, 150, 40);
    assert!(text(&buffer).contains("F4 Имя"));
    app.action(Action::Help);
    let buffer = render(&mut app, 150, 60);
    let text = text(&buffer);
    assert!(text.contains("F4"));
    assert!(text.contains("Переименовать"));
    assert!(!text.contains("F2"));
}

#[test]
fn mouse_settings_and_modifier_picker_use_the_same_capture_logic() {
    let (_, mut app) = app();
    app.action(Action::Settings);
    render(&mut app, 130, 36);
    let (rect, _) = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t, Target::SettingsSection(SettingsSection::Shortcuts)))
        .unwrap()
        .clone();
    app.handle(Input::Click {
        x: rect.x,
        y: rect.y,
        double: false,
        toggle: false,
    });
    assert_eq!(app.settings_view.section, SettingsSection::Shortcuts);
    capture(&mut app, Action::Help);
    render(&mut app, 130, 36);
    for target in [
        Target::BindingModifier(0),
        Target::BindingModifier(1),
        Target::BindingKey(Key::Char('j')),
    ] {
        let (rect, _) = app
            .hits
            .iter()
            .find(|(_, t)| match (&target, t) {
                (Target::BindingModifier(a), Target::BindingModifier(b)) => a == b,
                (Target::BindingKey(a), Target::BindingKey(b)) => a == b,
                _ => false,
            })
            .unwrap()
            .clone();
        app.handle(Input::Click {
            x: rect.x,
            y: rect.y,
            double: false,
            toggle: false,
        });
        render(&mut app, 130, 36);
    }
    assert_eq!(app.settings.shortcut(Action::Help), "Ctrl+Alt+J");
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
}

#[test]
fn changed_preferences_roundtrip_and_failed_saves_remain_visible_and_retryable() {
    let (root, mut app) = app();
    let blocker = root.path().join("config");
    fs::write(&blocker, "keep").unwrap();
    let path = blocker.join("settings.json");
    page(&mut app, SettingsTab::Typography);
    app.setting(0, false, 1);
    app.settings.appearance.theme = Theme::Light;
    app.persist_preferences(Some(&path));
    assert!(app.settings_dirty);
    assert!(app.settings_save_error.is_some());
    assert_eq!(fs::read_to_string(&blocker).unwrap(), "keep");
    fs::remove_file(&blocker).unwrap();
    app.handle(Input::Key(KeyPress::ctrl('s')));
    app.persist_preferences(Some(&path));
    assert!(!app.settings_dirty);
    assert!(app.settings_save_error.is_none());
    let restored = Settings::load(&path).unwrap();
    assert_eq!(restored.appearance, app.settings.appearance);
    assert_eq!(restored.bindings, app.settings.bindings);
    app.setting(0, false, 1);
    app.persist_preferences(None);
    assert!(app.settings_dirty);
    assert!(!app.settings_save_available);
}

#[test]
fn themes_are_isolated_per_app_and_layouts_handle_all_geometry_modes() {
    let (_, mut first) = app();
    let (_, mut second) = app();
    first.settings.appearance.theme = Theme::Light;
    let light = render(&mut first, 100, 30);
    let dark = render(&mut second, 100, 30);
    assert_eq!(light[(0, 0)].bg, Color::Rgb(239, 242, 246));
    assert_eq!(dark[(0, 0)].bg, Color::Rgb(0, 0, 0));
    assert_eq!(
        render(&mut first, 100, 30)[(0, 0)].bg,
        Color::Rgb(239, 242, 246)
    );
    for borders in BorderWeight::ALL {
        for corners in Corners::ALL {
            first.settings.appearance.borders = borders;
            first.settings.appearance.corners = corners;
            for section in SettingsSection::ALL {
                for (width, height) in [(1, 1), (24, 8), (34, 14), (50, 18), (80, 26), (150, 40)] {
                    first.dialog = Some(Dialog::Settings);
                    first.settings_section(section);
                    key(&mut first, Key::Tab);
                    key(&mut first, Key::End);
                    let buffer = render(&mut first, width, height);
                    assert!(
                        first
                            .hits
                            .iter()
                            .all(|(r, _)| r.right() <= buffer.area.right()
                                && r.bottom() <= buffer.area.bottom())
                    );
                }
            }
        }
    }
}

#[test]
fn reset_all_bindings_restores_aliases_and_palette_changes_survive_reload() {
    let (root, mut app) = app();
    capture(&mut app, Action::Help);
    key(&mut app, Key::F(4));
    app.settings_section(SettingsSection::Shortcuts);
    app.setting(0, true, 1);
    assert_eq!(app.settings.bindings, input::default_bindings());
    app.settings
        .appearance
        .colors
        .entry(Theme::MoltenZharr)
        .or_default()
        .insert("active".into(), [1, 2, 3]);
    app.preferences_changed();
    let path = root.path().join("settings.json");
    app.persist_preferences(Some(&path));
    assert_eq!(
        Settings::load(&path).unwrap().appearance.palette()["active"],
        [1, 2, 3]
    );
}

#[test]
fn invalid_binding_configuration_is_reported_without_rewriting_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut settings = Settings::default();
    settings.bindings[0].key = settings.bindings[1].key;
    let bytes = serde_json::to_vec(&settings).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(Settings::load(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(
        settings
            .save(Path::new("/should-not-write-invalid-config"))
            .is_err()
    );
}

#[cfg(feature = "guitui")]
#[test]
fn all_bundled_fonts_render_settings_at_supported_sizes() {
    use basalgrim::preferences::Appearance;
    use basalgrim::raster;
    let (_, mut app) = app();
    page(&mut app, SettingsTab::Typography);
    let mut hashes = Vec::new();
    for font in FontFace::ALL {
        for size in [10, 16, 32] {
            app.settings.appearance = Appearance {
                font,
                font_size: size,
                ..Appearance::default()
            };
            let mut terminal = Terminal::new(raster::font_backend(font, size, 100, 30)).unwrap();
            terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
            let data = terminal.backend().get_pixmap_data();
            assert!(data.iter().any(|v| *v != 0));
            hashes.push(data.iter().fold(0u64, |hash, &b| {
                hash.wrapping_mul(31).wrapping_add(u64::from(b))
            }));
        }
    }
    hashes.sort_unstable();
    hashes.dedup();
    assert_eq!(hashes.len(), 9);
}

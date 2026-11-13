use basalgrim::{
    app::{App, Dialog, PanelKind, ProfileControl, SettingsSection, Target},
    appearance::{self, Outcome, PaletteFile, Request},
    icons::IconBackend,
    input::{Action, Input, Key, KeyPress},
    operations::Cancellation,
    preferences::{Appearance, BorderWeight, PanelMode, Theme},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    os::unix::fs::symlink,
    time::{Duration, Instant},
};
fn render(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    t.backend().buffer().clone()
}
fn click(app: &mut App, find: impl Fn(&Target) -> bool) {
    let r = app.hits.iter().rev().find(|(_, t)| find(t)).unwrap().0;
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn key(app: &mut App, k: Key) {
    app.handle(Input::Key(KeyPress::plain(k)));
}
fn text(app: &mut App, s: &str) {
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text(s.into()));
    key(app, Key::Enter);
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if !app.loading() && !app.design.working() {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn profiles(app: &mut App) {
    app.action(Action::Settings);
    app.settings_section(SettingsSection::Profiles);
}
#[test]
fn named_appearance_mouse_lifecycle_preserves_unrelated_preferences_and_roundtrips() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    app.settings.appearance.theme = Theme::Light;
    app.settings.appearance.sidebar_mode = PanelMode::Hidden;
    app.settings.appearance.animations = true;
    let saved = app.settings.appearance.clone();
    let bindings = app.settings.bindings.clone();
    profiles(&mut app);
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(
            t,
            Target::Profile {
                control: ProfileControl::Save,
                ..
            }
        )
    });
    text(&mut app, "Моя светлая");
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
    assert_eq!(app.settings.appearance_presets.len(), 1);
    let id = app.settings.appearance_presets[0].id;
    app.settings.appearance = Appearance::default();
    app.settings.appearance.colors.insert(
        Theme::ClassicAmber,
        std::collections::BTreeMap::from([("active".into(), [1, 2, 3])]),
    );
    app.settings.grid = false;
    app.settings.dual = true;
    app.settings_view.selected = 3;
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(
            t,
            Target::Profile {
                control: ProfileControl::Apply(_),
                ..
            }
        )
    });
    assert_eq!(app.settings.appearance.theme, saved.theme);
    assert_eq!(app.settings.appearance.sidebar_mode, PanelMode::Hidden);
    assert!(app.settings.appearance.animations);
    assert_eq!(
        app.settings.appearance.colors[&Theme::ClassicAmber]["active"],
        [1, 2, 3]
    );
    assert!(!app.settings.grid && app.settings.dual);
    assert_eq!(app.settings.bindings, bindings);
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(
            t,
            Target::Profile {
                control: ProfileControl::Rename(_),
                ..
            }
        )
    });
    text(&mut app, "Рабочая");
    let path = root.path().join("settings.json");
    app.persist_preferences(Some(&path));
    let restored = Settings::load(&path).unwrap();
    assert_eq!(restored.appearance_presets[0].name, "Рабочая");
    assert_eq!(restored.appearance, app.settings.appearance);
    let before = app.settings.appearance.clone();
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(
            t,
            Target::Profile {
                control: ProfileControl::Remove(_),
                ..
            }
        )
    });
    assert!(app.settings.appearance_presets.is_empty());
    assert_eq!(app.settings.appearance, before);
    let revision = app.design.revision;
    app.settings_section(SettingsSection::Themes);
    app.profile_control(revision, ProfileControl::Apply(id));
    assert_eq!(app.settings.appearance, before);
}
#[test]
fn invalid_palette_never_changes_appearance_and_export_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("colors.json");
    let a = Appearance::default();
    let cancel = Cancellation::default();
    let palette = PaletteFile::capture(&a);
    appearance::execute(
        Request::Export {
            path: path.clone(),
            palette: palette.clone(),
        },
        &cancel,
    )
    .unwrap();
    let original = fs::read(&path).unwrap();
    assert!(
        appearance::execute(
            Request::Export {
                path: path.clone(),
                palette: palette.clone()
            },
            &cancel
        )
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    let Outcome::Imported(loaded) =
        appearance::execute(Request::Import(path.clone()), &cancel).unwrap()
    else {
        panic!()
    };
    assert_eq!(loaded.validate().unwrap(), a.palette());
    let mut target = Appearance {
        theme: Theme::Light,
        ..Default::default()
    };
    loaded.apply(&mut target, Theme::Light).unwrap();
    assert_eq!(target.palette(), a.palette());
    for variant in 0..5 {
        let mut p = palette.clone();
        match variant {
            0 => p.version = 2,
            1 => {
                p.colors.insert("background".into(), "bad".into());
            }
            2 => {
                p.colors.remove("background");
            }
            3 => {
                p.colors.remove("background");
                p.colors.insert("invented".into(), "#112233".into());
            }
            _ => p.name = "bad\nname".into(),
        }
        let before = target.clone();
        assert!(p.apply(&mut target, Theme::Light).is_err());
        assert_eq!(target, before);
        fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
        assert!(appearance::execute(Request::Import(path.clone()), &cancel).is_err());
    }
    fs::write(&path, vec![b' '; 16385]).unwrap();
    assert!(appearance::execute(Request::Import(path.clone()), &cancel).is_err());
    assert!(appearance::execute(Request::Import(dir.path().into()), &cancel).is_err());
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert!(appearance::execute(Request::Import(link), &cancel).is_err());
    cancel.cancel();
    let cancelled = dir.path().join("cancelled.json");
    assert!(
        appearance::execute(
            Request::Export {
                path: cancelled.clone(),
                palette
            },
            &cancel
        )
        .is_err()
    );
    assert!(!cancelled.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}
#[test]
fn asynchronous_import_cannot_apply_after_closing_or_changing_settings() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("import.json");
    let mut palette = PaletteFile::capture(&Appearance::default());
    palette.colors.insert("background".into(), "#123456".into());
    fs::write(&p, serde_json::to_vec(&palette).unwrap()).unwrap();
    let mut app = App::new(dir.path().into(), Settings::default());
    settle(&mut app);
    for stale in 0..3 {
        profiles(&mut app);
        app.profile_control(app.design.revision, ProfileControl::Import);
        text(&mut app, p.to_str().unwrap());
        assert!(app.design.working());
        match stale {
            0 => key(&mut app, Key::Escape),
            1 => {
                app.settings.appearance.theme = Theme::Light;
                app.preferences_changed();
            }
            _ => {}
        }
        settle(&mut app);
        if stale < 2 {
            assert_ne!(
                app.settings.appearance.palette()["background"],
                [0x12, 0x34, 0x56]
            );
        } else {
            assert_eq!(
                app.settings.appearance.palette()["background"],
                [0x12, 0x34, 0x56]
            );
        }
        app.dialog = None;
    }
}
#[test]
fn hidden_panels_have_mouse_access_without_hover_navigation_or_click_through() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "preview line\n".repeat(40)).unwrap();
    let mut app = App::new(dir.path().into(), Settings::default());
    settle(&mut app);
    app.settings.grid = false;
    app.icon_backend = IconBackend::Symbols;
    render(&mut app, 170, 53);
    assert!(app.design.sidebar_visible && app.design.preview_visible);
    let selected = app.tab().selected;
    let request = app.tab().request;
    for border in BorderWeight::ALL {
        app.settings.appearance.borders = border;
        render(&mut app, 34, 14);
        assert!(!app.design.sidebar_visible && !app.design.preview_visible);
        for kind in [PanelKind::Places, PanelKind::Preview] {
            click(&mut app, |t| matches!(t,Target::PanelOpen(k) if *k==kind));
            let buffer = render(&mut app, 34, 14);
            assert!(
                app.hits
                    .iter()
                    .all(|(r, _)| buffer.area.contains((r.x, r.y).into())
                        && r.right() <= 34
                        && r.bottom() <= 14)
            );
            app.handle(Input::Pointer(Some((0, 0).into())));
            assert!(app.hovered_area().is_none());
            assert_eq!(app.tab().selected, selected);
            assert_eq!(app.tab().request, request);
            click(&mut app, |t| {
                matches!(t, Target::PanelScroll { delta: 1, .. })
            });
            render(&mut app, 34, 14);
            click(&mut app, |t| matches!(t, Target::PanelClose(_)));
            assert!(app.dialog.is_none());
            render(&mut app, 34, 14);
        }
    }
    click(&mut app, |t| {
        matches!(t, Target::PanelOpen(PanelKind::Places))
    });
    render(&mut app, 34, 14);
    app.handle(Input::Click {
        x: 0,
        y: 0,
        double: false,
        toggle: false,
    });
    assert!(app.dialog.is_none());
    assert_eq!(app.tab().request, request);
    assert_eq!(app.tab().selected, selected);
    app.settings.appearance.sidebar_mode = PanelMode::Hidden;
    app.settings.appearance.preview_mode = PanelMode::Hidden;
    render(&mut app, 170, 53);
    assert!(!app.design.sidebar_visible && !app.design.preview_visible);
    click(&mut app, |t| {
        matches!(t, Target::PanelOpen(PanelKind::Places))
    });
    render(&mut app, 170, 53);
    let old_token = app.design.panel_token;
    key(&mut app, Key::Escape);
    app.open_panel(PanelKind::Places);
    app.hits = vec![(Rect::new(0, 0, 1, 1), Target::PanelClose(old_token))];
    app.handle(Input::Click {
        x: 0,
        y: 0,
        double: false,
        toggle: false,
    });
    assert!(matches!(app.dialog, Some(Dialog::Panel(_))));
}
#[test]
fn names_are_bounded_unique_and_animation_does_not_change_geometry() {
    assert!(appearance::name("\n").is_err());
    assert!(appearance::name(&"Я".repeat(49)).is_err());
    let p = appearance::Preset {
        id: 1,
        name: "Работа".into(),
        appearance: Appearance::default(),
    };
    assert!(
        appearance::validate(&mut [
            p.clone(),
            appearance::Preset {
                id: 2,
                name: "РАБОТА".into(),
                ..p.clone()
            }
        ])
        .is_err()
    );
    assert!(appearance::validate(&mut vec![p; 33]).is_err());
    use unicode_width::UnicodeWidthStr;
    for time in [0, 160, 320, 480, 640] {
        assert_eq!(
            appearance::activity(false, time),
            appearance::activity(false, 0)
        );
        assert_eq!(appearance::activity(true, time).width(), 3);
    }
    assert_ne!(
        appearance::activity(true, 0),
        appearance::activity(true, 160)
    );
}

#[test]
fn all_settings_sections_and_compact_preview_content_remain_accessible() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("plain.txt"),
        "VISIBLE PREVIEW\nsecond line",
    )
    .unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    app.action(Action::Settings);
    for height in 14..26 {
        render(&mut app, 50, height);
        for section in SettingsSection::ALL {
            assert!(
                app.hits
                    .iter()
                    .any(|(_, t)| matches!(t,Target::SettingsSection(s) if *s==section)),
                "missing {} at {height}",
                section.name()
            );
        }
    }
    key(&mut app, Key::Escape);
    render(&mut app, 34, 14);
    click(&mut app, |t| {
        matches!(t, Target::PanelOpen(PanelKind::Preview))
    });
    let buffer = render(&mut app, 34, 14);
    let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
    assert!(text.contains("VISIBLE PREVIEW"), "{text}");
    key(&mut app, Key::Escape);
    // The overlay dispatches the same actual navigation target, never a second filesystem path.
    let destination = root.path().join("destination");
    fs::create_dir(&destination).unwrap();
    app.places[0].path = destination.clone();
    app.open_panel(PanelKind::Places);
    render(&mut app, 34, 14);
    click(
        &mut app,
        |t| matches!(t,Target::PanelLink {target,..} if matches!(target.as_ref(),Target::Place(path) if *path == destination)),
    );
    assert!(app.dialog.is_none());
    settle(&mut app);
    assert_eq!(app.tab().cwd, destination);
}

#[test]
fn palette_result_cannot_hide_settings_save_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    profiles(&mut app);
    app.design.status = "Палитра применена".into();
    app.preferences_changed();
    let blocker = root.path().join("blocker");
    fs::write(&blocker, "keep").unwrap();
    app.persist_preferences(Some(&blocker.join("settings.json")));
    render(&mut app, 34, 14);
    assert!(app.settings_save_error.is_some());
    assert!(app.hits.iter().any(|(_, t)| matches!(t, Target::RetrySave)));
    assert_eq!(fs::read_to_string(blocker).unwrap(), "keep");
}

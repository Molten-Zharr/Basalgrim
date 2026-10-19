use basalgrim::{
    app::{App, Dialog, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < deadline, "Background operation timed out");
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("folder")).unwrap();
    for name in ["alpha.rs", "beta.md", ".secret"] {
        fs::write(root.path().join(name), name).unwrap();
    }
    let mut app = App::new(root.path().to_path_buf(), Settings::default());
    settle(&mut app);
    (root, app)
}
fn key(app: &mut App, key: Key) {
    app.handle(Input::Key(KeyPress::plain(key)));
}
fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(app, frame)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn navigation_history_tabs_and_panes_are_independent() {
    let (root, mut app) = setup();
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path().join("folder"));
    app.action(Action::Back);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path());
    app.action(Action::Forward);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path().join("folder"));
    app.action(Action::NewTab);
    settle(&mut app);
    app.action(Action::Parent);
    settle(&mut app);
    assert_eq!(app.panes[0].tabs[0].cwd, root.path().join("folder"));
    assert_eq!(app.tab().cwd, root.path());
    app.action(Action::Dual);
    key(&mut app, Key::Tab);
    assert_eq!(app.active, 1);
    app.action(Action::Parent);
    settle(&mut app);
    assert_eq!(app.panes[0].tab().cwd, root.path());
    app.action(Action::Dual);
    assert_eq!(app.active, 0);
}

#[test]
fn search_is_live_cancel_restores_filter_and_hidden_toggle_is_shared() {
    let (_, mut app) = setup();
    assert_eq!(app.tab().visible.len(), 3);
    app.action(Action::Search);
    app.handle(Input::Text("beta".into()));
    assert_eq!(app.tab().visible.len(), 1);
    key(&mut app, Key::Escape);
    assert_eq!(app.tab().visible.len(), 3);
    assert!(app.tab().filter.is_empty());
    app.action(Action::Hidden);
    assert_eq!(app.tab().visible.len(), 4);
    app.action(Action::Search);
    app.handle(Input::Text(".secret".into()));
    key(&mut app, Key::Enter);
    assert_eq!(app.tab().entry().unwrap().name, ".secret");
}

#[test]
fn stale_directory_and_preview_results_do_not_replace_current_selection() {
    let (root, mut app) = setup();
    for _ in 0..20 {
        app.navigate(root.path().join("folder"), true);
        app.navigate(root.path().to_path_buf(), true);
    }
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path());
    assert_eq!(app.tab().visible.len(), 3);
    app.tab_mut().selected = 1;
    app.update_preview();
    app.tab_mut().selected = 2;
    app.update_preview();
    settle(&mut app);
    assert_eq!(app.preview.as_ref().unwrap().title, "beta.md");
}

#[test]
fn create_rename_copy_and_cut_work_through_shared_actions() {
    let (root, mut app) = setup();
    app.action(Action::NewFile);
    app.handle(Input::Text("created.txt".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert!(root.path().join("created.txt").exists());
    let pos = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "created.txt")
        .unwrap();
    app.tab_mut().selected = pos;
    app.action(Action::Rename);
    app.handle(Input::Key(KeyPress::ctrl('u')));
    app.handle(Input::Text("renamed.txt".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert!(!root.path().join("created.txt").exists());
    assert!(root.path().join("renamed.txt").exists());
    let pos = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "renamed.txt")
        .unwrap();
    app.tab_mut().selected = pos;
    app.action(Action::Cut);
    app.navigate(root.path().join("folder"), true);
    settle(&mut app);
    app.action(Action::Paste);
    settle(&mut app);
    assert!(root.path().join("folder/renamed.txt").exists());
    assert!(!root.path().join("renamed.txt").exists());
    assert!(app.clipboard.as_ref().unwrap().paths.is_empty());
}

#[test]
fn delete_requires_confirmation_and_cancel_preserves_files() {
    let (root, mut app) = setup();
    app.tab_mut().selected = 1;
    app.action(Action::Trash);
    assert!(matches!(app.dialog, Some(Dialog::Trash(_))));
    assert!(root.path().join("alpha.rs").exists());
    key(&mut app, Key::Escape);
    assert!(app.dialog.is_none());
    assert!(root.path().join("alpha.rs").exists());
    assert!(!app.busy);
}

#[test]
fn keyboard_and_mouse_share_selection_and_modal_hit_regions() {
    let (_, mut app) = setup();
    app.action(Action::Grid);
    render(&mut app, 130, 35);
    let (rect, _) = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t, Target::Entry { pane: 0, index: 2 }))
        .unwrap()
        .clone();
    app.handle(Input::Click {
        x: rect.x,
        y: rect.y,
        double: false,
        toggle: true,
    });
    assert_eq!(app.tab().entry().unwrap().name, "beta.md");
    assert_eq!(app.tab().marked.len(), 1);
    app.action(Action::Rename);
    render(&mut app, 130, 35);
    assert!(
        app.hits
            .iter()
            .all(|(_, t)| matches!(t, Target::Confirm | Target::Cancel))
    );
}

#[test]
fn responsive_layouts_do_not_panic_and_hit_regions_stay_inside_screen() {
    let (_, mut app) = setup();
    for grid in [false, true] {
        for dual in [false, true] {
            for preview in [false, true] {
                app.settings.grid = grid;
                app.settings.dual = dual;
                app.settings.preview = preview;
                for (width, height) in [
                    (1, 1),
                    (24, 8),
                    (34, 14),
                    (50, 18),
                    (70, 26),
                    (100, 32),
                    (150, 44),
                    (190, 50),
                ] {
                    let buffer = render(&mut app, width, height);
                    assert!(
                        app.hits
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
fn empty_unreadable_and_removed_directories_leave_navigation_usable() {
    let (root, mut app) = setup();
    app.navigate(root.path().join("missing"), true);
    settle(&mut app);
    assert!(app.tab().error.is_some());
    assert!(app.tab().targets().is_empty());
    render(&mut app, 100, 30);
    app.action(Action::Back);
    settle(&mut app);
    assert_eq!(app.tab().visible.len(), 3);
}

#[test]
fn settings_roundtrip_and_corrupt_files_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let settings = Settings {
        grid: false,
        appearance: basalgrim::preferences::Appearance {
            font_size: 24,
            ..Default::default()
        },
        ..Settings::default()
    };
    settings.save(&path).unwrap();
    let loaded = Settings::load(&path).unwrap();
    assert!(!loaded.grid);
    assert_eq!(loaded.appearance.font_size, 24);
    fs::write(&path, "broken").unwrap();
    assert!(Settings::load(&path).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "broken");
}

#[test]
fn minimum_window_keeps_files_visible_and_help_can_scroll_to_the_end() {
    let (_, mut app) = setup();
    let buffer = render(&mut app, 34, 14);
    let text = buffer
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("folder"));
    app.action(Action::Help);
    render(&mut app, 50, 18);
    assert!(app.help_max > 0);
    key(&mut app, Key::End);
    let buffer = render(&mut app, 50, 18);
    let text = buffer
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("файловой системы"));
    key(&mut app, Key::Home);
    assert_eq!(app.help_offset, 0);
}

#[test]
fn typed_paths_are_resolved_before_parent_navigation() {
    let (root, mut app) = setup();
    app.navigate(root.path().join("folder/.."), true);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path().canonicalize().unwrap());
    app.action(Action::Parent);
    settle(&mut app);
    assert_eq!(
        app.tab().cwd,
        root.path().parent().unwrap().canonicalize().unwrap()
    );
}

#[test]
fn enlarging_the_window_reveals_all_entries_when_they_fit() {
    let (_root, mut app) = setup();
    for grid in [false, true] {
        app.settings.grid = grid;
        app.tab_mut().selected = app.tab().visible.len() - 1;
        render(&mut app, 34, 14);
        render(&mut app, 150, 50);
        assert_eq!(app.tab().offset, 0);
    }
}

#[test]
fn hovering_files_is_lighter_than_selection_and_does_not_change_browser_state() {
    use basalgrim::preferences::Theme;
    for theme in [Theme::MoltenZharr, Theme::Light] {
        for grid in [false, true] {
            let (_root, mut app) = setup();
            app.settings.grid = grid;
            app.settings.appearance.theme = theme;
            let baseline = render(&mut app, 150, 40);
            let area = app
                .hits
                .iter()
                .find_map(|(r, t)| matches!(t, Target::Entry { pane: 0, index: 1 }).then_some(*r))
                .unwrap();
            let selected = app
                .hits
                .iter()
                .find_map(|(r, t)| matches!(t, Target::Entry { pane: 0, index: 0 }).then_some(*r))
                .unwrap();
            let point = ratatui::layout::Position::new(area.x, area.y);
            let cwd = app.tab().cwd.clone();
            let preview = app.preview.as_ref().map(|p| p.title.clone());
            app.handle(Input::Pointer(Some(point)));
            let hover = render(&mut app, 150, 40);
            assert_ne!(
                hover[(point.x, point.y)].bg,
                baseline[(point.x, point.y)].bg
            );
            assert_ne!(
                hover[(point.x, point.y)].bg,
                baseline[(selected.x, selected.y)].bg
            );
            assert_eq!(
                hover[(point.x, point.y)].fg,
                baseline[(point.x, point.y)].fg
            );
            assert_eq!(
                hover[(selected.x, selected.y)],
                baseline[(selected.x, selected.y)]
            );
            assert_eq!(app.tab().selected, 0);
            assert!(app.tab().marked.is_empty());
            assert_eq!(app.active, 0);
            assert_eq!(app.tab().cwd, cwd);
            assert_eq!(app.preview.as_ref().map(|p| p.title.clone()), preview);
            assert!(!app.busy);
            assert_eq!(app.operation_count, 0);
            assert_eq!(render(&mut app, 150, 40), hover); // Idle frames must not accumulate tint.
            app.handle(Input::Pointer(None));
            assert_eq!(render(&mut app, 150, 40), baseline);
            app.handle(Input::Click {
                x: point.x,
                y: point.y,
                double: false,
                toggle: false,
            });
            let clicked = render(&mut app, 150, 40);
            assert_eq!(app.tab().selected, 1);
            assert_eq!(
                clicked[(point.x, point.y)].bg,
                baseline[(selected.x, selected.y)].bg
            );
        }
    }
}

#[test]
fn hover_uses_topmost_click_regions_and_cannot_reach_behind_a_dialog() {
    use basalgrim::app::{SettingsFocus, SettingsTab};
    let (_root, mut app) = setup();
    let baseline = render(&mut app, 150, 40);
    let toolbar = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Action(Action::Settings)).then_some(*r))
        .unwrap();
    let point = ratatui::layout::Position::new(toolbar.x, toolbar.y);
    app.handle(Input::Pointer(Some(point)));
    assert_ne!(
        render(&mut app, 150, 40)[(point.x, point.y)].bg,
        baseline[(point.x, point.y)].bg
    );
    app.action(Action::NewFolder);
    app.handle(Input::Pointer(None));
    let modal = render(&mut app, 150, 40);
    app.handle(Input::Pointer(Some(point)));
    assert_eq!(render(&mut app, 150, 40), modal);
    assert!(app.hovered_area().is_none());
    let confirm = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Confirm).then_some(*r))
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        confirm.x, confirm.y,
    ))));
    assert_ne!(
        render(&mut app, 150, 40)[(confirm.x, confirm.y)].bg,
        modal[(confirm.x, confirm.y)].bg
    );
    assert!(!app.busy);
    app.cancel_dialog();
    app.action(Action::Settings);
    app.settings_tab(SettingsTab::Typography);
    app.settings_view.focus = SettingsFocus::Menu;
    app.handle(Input::Pointer(None));
    let settings = render(&mut app, 150, 40);
    let arrow = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(
                t,
                Target::Setting {
                    index: 0,
                    activate: false,
                    direction: 1
                }
            )
            .then_some(*r)
        })
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        arrow.x, arrow.y,
    ))));
    let hover = render(&mut app, 150, 40);
    assert_eq!(app.hovered_area(), Some(arrow));
    assert_ne!(
        hover[(arrow.x, arrow.y)].bg,
        settings[(arrow.x, arrow.y)].bg
    );
    assert_eq!(app.settings_view.focus, SettingsFocus::Menu);
    app.handle(Input::Pointer(None));
    assert_eq!(render(&mut app, 150, 40), settings);
    app.dialog = Some(Dialog::CaptureBinding {
        index: 0,
        error: None,
    });
    let before = app.settings.bindings.clone();
    let baseline = render(&mut app, 150, 40);
    let key = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::BindingKey(Key::Char('j'))).then_some(*r))
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        key.x, key.y,
    ))));
    assert_ne!(
        render(&mut app, 150, 40)[(key.x, key.y)].bg,
        baseline[(key.x, key.y)].bg
    );
    assert_eq!(app.settings.bindings, before);
    assert_eq!(app.settings_view.modifiers, [false; 3]);
    assert!(!app.quit);
}

#[cfg(feature = "guitui")]
#[test]
fn leaving_a_hovered_item_restores_the_cached_gui_pixels() {
    use basalgrim::{
        preferences::{FontFace, Theme},
        raster,
    };
    let (_root, mut app) = setup();
    for theme in [Theme::MoltenZharr, Theme::Light] {
        for font in FontFace::ALL {
            app.settings.appearance.theme = theme;
            app.handle(Input::Pointer(None));
            let mut terminal = Terminal::new(raster::font_backend(font, 16, 150, 40)).unwrap();
            terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
            let baseline = terminal.backend().get_pixmap_data().to_vec();
            let area = app
                .hits
                .iter()
                .find_map(|(r, t)| matches!(t, Target::Entry { pane: 0, index: 1 }).then_some(*r))
                .unwrap();
            app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
                area.x, area.y,
            ))));
            terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
            assert_ne!(terminal.backend().get_pixmap_data(), baseline);
            app.handle(Input::Pointer(None));
            terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
            assert_eq!(terminal.backend().get_pixmap_data(), baseline);
        }
    }
}

use basalgrim::{
    app::{App, Dialog, FilterControl as C, FilterOwner, Target},
    filter::{Kind, Options},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
    view::ViewMode,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use std::{
    fs,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        app.tick();
        if !app.loading() {
            return;
        }
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= w && r.bottom() <= h)
    );
    t.backend().buffer().clone()
}
fn key(app: &mut App, key: Key) {
    app.handle(Input::Key(KeyPress::plain(key)));
}
fn control(app: &mut App, c: C) {
    let owner = FilterOwner::current(app, app.active);
    app.filter_control(owner, None, c);
}
fn names(app: &App) -> Vec<String> {
    app.tab()
        .visible
        .iter()
        .map(|&i| {
            app.tab().entries[i]
                .path
                .strip_prefix(&app.tab().cwd)
                .unwrap()
                .to_string_lossy()
                .into()
        })
        .collect()
}
fn query(app: &mut App, q: &str) {
    app.action(Action::Search);
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Text(q.into()));
    key(app, Key::Enter);
    settle(app);
}
fn scene() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let minecraft = root.path().join("minecraft");
    fs::create_dir_all(minecraft.join("config/weapon_master")).unwrap();
    fs::write(minecraft.join("root.toml"), "").unwrap();
    fs::write(minecraft.join("config/weapon_master.toml"), "").unwrap();
    fs::write(
        minecraft.join("config/weapon_master/weapon_master.toml"),
        "",
    )
    .unwrap();
    fs::write(minecraft.join("config/Weapon_Master.TOML"), "").unwrap();
    let mut app = App::new(
        minecraft,
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    (root, app)
}
#[test]
fn depth_zero_one_two_and_unlimited_match_the_minecraft_example() {
    let (_tmp, mut app) = scene();
    query(&mut app, "weapon_master.toml");
    assert!(names(&app).is_empty());
    control(&mut app, C::Depth(Some(1)));
    settle(&mut app);
    assert_eq!(names(&app).len(), 2);
    assert!(
        names(&app)
            .iter()
            .all(|n| n.starts_with("config/") && n.matches('/').count() == 1)
    );
    control(&mut app, C::Depth(Some(2)));
    settle(&mut app);
    assert_eq!(names(&app).len(), 3);
    control(&mut app, C::Depth(None));
    settle(&mut app);
    assert_eq!(names(&app).len(), 3);
    control(&mut app, C::Case);
    settle(&mut app);
    assert_eq!(
        names(&app)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "config/weapon_master.toml".to_owned(),
            "config/weapon_master/weapon_master.toml".to_owned()
        ]
        .into_iter()
        .collect()
    );
}
#[test]
fn kinds_extension_and_case_compose_without_pruning_the_traversal() {
    let (_tmp, mut app) = scene();
    control(&mut app, C::Depth(Some(2)));
    settle(&mut app);
    control(&mut app, C::Kind(Kind::Files));
    settle(&mut app);
    assert_eq!(names(&app).len(), 4);
    control(&mut app, C::Extension);
    app.handle(Input::Text(".tom".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert_eq!(names(&app).len(), 4);
    control(&mut app, C::Case);
    settle(&mut app);
    assert_eq!(names(&app).len(), 3);
    control(&mut app, C::Extension);
    key(&mut app, Key::Backspace);
    key(&mut app, Key::Enter);
    control(&mut app, C::Kind(Kind::Folders));
    settle(&mut app);
    assert_eq!(names(&app), ["config", "config/weapon_master"]);
    query(&mut app, "weapon");
    assert_eq!(names(&app), ["config/weapon_master"]);
}
#[test]
fn keeping_filters_on_navigation_is_explicit_and_clearing_restores_directory() {
    let (_tmp, mut app) = scene();
    query(&mut app, "weapon");
    control(&mut app, C::Depth(Some(2)));
    settle(&mut app);
    let dir = app.tab().cwd.join("config");
    app.navigate(dir, true);
    settle(&mut app);
    assert!(app.tab().filter.is_empty());
    assert_eq!(app.tab().filter_options, Options::default());
    assert!(app.tab().filter_run.is_none());
    query(&mut app, "weapon_master.toml");
    control(&mut app, C::Depth(Some(1)));
    settle(&mut app);
    control(&mut app, C::Keep);
    let dir = app.tab().cwd.join("weapon_master");
    app.navigate(dir, true);
    settle(&mut app);
    assert_eq!(app.tab().filter, "weapon_master.toml");
    assert_eq!(names(&app), ["weapon_master.toml"]);
    control(&mut app, C::Clear);
    settle(&mut app);
    assert!(app.tab().filter.is_empty());
    assert!(app.tab().filter_run.is_none());
    assert_eq!(names(&app), ["weapon_master.toml"]);
}
#[test]
fn all_depths_and_unlimited_have_no_silent_match_limit_and_sort_the_full_catalog() {
    let (tmp, mut app) = scene();
    let root = app.tab().cwd.clone();
    let mut dir = root.clone();
    for d in 0..9 {
        dir.push(format!("d{d}"));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(format!("needle-{d}.toml")), "").unwrap();
    }
    query(&mut app, "needle");
    for d in 0..=6 {
        control(&mut app, C::Depth(Some(d)));
        settle(&mut app);
        assert_eq!(names(&app).len(), usize::from(d));
    }
    control(&mut app, C::Depth(None));
    settle(&mut app);
    assert_eq!(names(&app).len(), 9);
    let directory = root.join("bulk");
    fs::create_dir(&directory).unwrap();
    for i in (0..310).rev() {
        fs::write(directory.join(format!("bulk-{i:03}.txt")), "").unwrap();
    }
    query(&mut app, "bulk-");
    let run = app.tab().filter_run.as_ref().unwrap();
    assert_eq!(run.worker.results.total, 310);
    assert_eq!(app.tab().entries.len(), 128);
    assert!(names(&app)[0].ends_with("bulk-000.txt"));
    control(&mut app, C::Page(1));
    settle(&mut app);
    assert!(names(&app)[0].ends_with("bulk-128.txt"));
    control(&mut app, C::Page(1));
    settle(&mut app);
    assert_eq!(app.tab().entries.len(), 54);
    assert!(names(&app).last().unwrap().ends_with("bulk-309.txt"));
    assert!(tmp.path().exists());
}
#[test]
fn lower_bar_and_locations_are_real_renderer_hits_in_every_view_at_minimum_size() {
    let (_tmp, mut app) = scene();
    query(&mut app, "weapon_master.toml");
    control(&mut app, C::Depth(Some(2)));
    settle(&mut app);
    for mode in ViewMode::ALL {
        app.select_view(mode);
        settle(&mut app);
        for (w, h) in [(34, 14), (150, 44)] {
            let b = render(&mut app, w, h);
            let text = b.content.iter().map(|c| c.symbol()).collect::<String>();
            assert!(
                app.hits.iter().any(|(_, t)| matches!(
                    t,
                    Target::Filter {
                        control: C::Name,
                        ..
                    }
                )),
                "{mode:?} {w} {text}"
            );
            assert!(app.hits.iter().any(|(_, t)| matches!(
                t,
                Target::Filter {
                    control: C::DepthMenu,
                    ..
                }
            )));
            assert!(
                text.contains("config"),
                "source folder missing {mode:?} {w}: {text}"
            );
        }
    }
}
#[test]
fn late_worker_and_stale_controls_cannot_restore_an_old_query_or_retarget_actions() {
    let (_tmp, mut app) = scene();
    render(&mut app, 100, 40);
    let old = FilterOwner::current(&app, 0);
    query(&mut app, "weapon_master.toml");
    control(&mut app, C::Depth(Some(2)));
    query(&mut app, "root");
    settle(&mut app);
    assert_eq!(names(&app), ["root.toml"]);
    app.filter_control(old, None, C::Clear);
    assert_eq!(app.tab().filter, "root");
    app.action(Action::Rename);
    app.handle(Input::Text("renamed".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert!(app.tab().cwd.join("renamed.toml").exists());
    assert!(!app.tab().cwd.join("root.toml").exists());
}
#[test]
fn text_selection_and_bottom_controls_do_not_dispatch_file_clipboard_actions() {
    let (_tmp, mut app) = scene();
    render(&mut app, 100, 40);
    control(&mut app, C::Name);
    app.handle(Input::Text("weapon".into()));
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Key(KeyPress::ctrl('c')));
    assert_eq!(app.take_text_copy().unwrap().text, "weapon");
    assert!(app.clipboard.is_none());
    key(&mut app, Key::Backspace);
    assert_eq!(app.tab().filter, "");
    key(&mut app, Key::Enter);
    render(&mut app, 34, 14);
    control(&mut app, C::DepthMenu);
    render(&mut app, 34, 14);
    assert!(matches!(app.dialog, Some(Dialog::FilterMenu(_))));
    let depths = app
        .hits
        .iter()
        .filter(|(_, t)| {
            matches!(
                t,
                Target::Filter {
                    control: C::Depth(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(depths, 8);
    let rect = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(
                t,
                Target::Filter {
                    control: C::Depth(Some(1)),
                    ..
                }
            )
            .then_some(*r)
        })
        .unwrap();
    click(&mut app, rect);
    settle(&mut app);
    assert_eq!(app.tab().filter_options.depth, Some(1));
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}

#[test]
fn preserved_filter_canonicalizes_navigation_and_never_expands_cached_tree_branches() {
    let (_tmp, mut app) = scene();
    let root = app.tab().cwd.clone();
    std::os::unix::fs::symlink(root.join("config"), root.join("link")).unwrap();
    query(&mut app, "weapon_master.toml");
    control(&mut app, C::Depth(Some(1)));
    settle(&mut app);
    control(&mut app, C::Keep);
    app.navigate(root.join("link"), true);
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.join("config"));
    assert_eq!(names(&app).len(), 3);
    control(&mut app, C::Clear);
    settle(&mut app);
    app.select_view(ViewMode::Tree);
    settle(&mut app);
    key(&mut app, Key::Right);
    settle(&mut app);
    query(&mut app, "weapon_master.toml");
    assert_eq!(
        names(&app).len(),
        2,
        "depth zero must not include an expanded cached child"
    );
    key(&mut app, Key::Right);
    settle(&mut app);
    assert_eq!(names(&app).len(), 2);
}

#[test]
fn operations_on_duplicate_names_use_the_selected_results_parent() {
    let (_tmp, mut app) = scene();
    query(&mut app, "weapon_master.toml");
    control(&mut app, C::Depth(Some(2)));
    settle(&mut app);
    let root = app.tab().cwd.clone();
    app.tab_mut().selected = app
        .tab()
        .visible
        .iter()
        .position(|&i| {
            app.tab().entries[i].path == root.join("config/weapon_master/weapon_master.toml")
        })
        .unwrap();
    app.action(Action::Copy);
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        [root.join("config/weapon_master/weapon_master.toml")]
    );
    app.action(Action::Rename);
    app.handle(Input::Text("renamed".into()));
    key(&mut app, Key::Enter);
    settle(&mut app);
    assert!(root.join("config/weapon_master/renamed.toml").exists());
    assert!(root.join("config/weapon_master.toml").exists());
    assert!(!root.join("renamed.toml").exists());
}

#[test]
fn grouped_filtered_views_keep_two_line_rows_and_wheel_preserves_focus() {
    let (_tmp, mut app) = scene();
    let root = app.tab().cwd.clone();
    for i in 0..40 {
        fs::write(root.join(format!("config/sample-{i:02}.txt")), "").unwrap();
    }
    query(&mut app, "sample");
    control(&mut app, C::Depth(Some(1)));
    settle(&mut app);
    app.settings.grouping.enabled = true;
    app.settings.grouping.field = basalgrim::columns::Column::Extension;
    for view in ViewMode::ALL {
        app.select_view(view);
        settle(&mut app);
        render(&mut app, 34, 14);
        let before = app.tab().entry().unwrap().path.clone();
        let r = app.file_areas[app.active];
        app.handle(Input::Scroll {
            x: r.x,
            y: r.y,
            delta: 3,
        });
        render(&mut app, 34, 14);
        assert_eq!(app.tab().entry().unwrap().path, before);
        key(&mut app, Key::End);
        let b = render(&mut app, 34, 14);
        let text = b.content.iter().map(|c| c.symbol()).collect::<String>();
        assert!(
            text.contains("config"),
            "missing grouped source path in {view:?}: {text}"
        );
        assert_eq!(app.tab().entry().unwrap().name, "sample-39.txt");
    }
}

#[test]
fn inline_filter_keeps_the_result_canvas_scrollable_without_losing_text_focus() {
    let (_tmp, mut app) = scene();
    for i in 0..30 {
        fs::write(app.tab().cwd.join(format!("scroll-{i:02}.txt")), "").unwrap();
    }
    app.refresh();
    settle(&mut app);
    app.select_view(ViewMode::List);
    app.action(Action::Search);
    app.handle(Input::Text("scroll".into()));
    render(&mut app, 34, 14);
    let selected = app.tab().entry().unwrap().path.clone();
    let r = app.file_areas[app.active];
    app.handle(Input::Scroll {
        x: r.x,
        y: r.y,
        delta: 1,
    });
    render(&mut app, 34, 14);
    assert!(app.tab().offset > 0);
    assert_eq!(app.tab().entry().unwrap().path, selected);
    assert!(app.inline_filter());
    assert_eq!(app.tab().filter, "scroll");
}

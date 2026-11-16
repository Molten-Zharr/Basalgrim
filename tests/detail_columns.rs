use basalgrim::{
    app::{App, Dialog, Target},
    columns::{Column, Layout, Spec, TOTAL},
    fs::Sort,
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
    time::{Duration, Instant, SystemTime},
};
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= w && r.bottom() <= h)
    );
    assert!(
        app.icon_slots
            .iter()
            .all(|s| s.area.right() <= w && s.area.bottom() <= h)
    );
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        app.tick();
        if !app.loading() {
            return;
        }
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn setup() -> (tempfile::TempDir, App) {
    let t = tempfile::tempdir().unwrap();
    for (name, size) in [("b.txt", 9), ("a.rs", 100), ("c.png", 20)] {
        fs::write(t.path().join(name), vec![b'x'; size]).unwrap();
    }
    let mut app = App::new(
        t.path().into(),
        Settings {
            view: Some(ViewMode::List),
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    (t, app)
}
fn hit(app: &App, p: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .find_map(|(r, t)| p(t).then_some(*r))
        .expect("missing hit")
}
fn click(app: &mut App, r: Rect, double: bool) {
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
fn header(app: &App, c: Column) -> Rect {
    hit(
        app,
        |t| matches!(t,Target::ColumnHeader{column,..} if *column==c),
    )
}
fn boundary(app: &App, c: Column) -> Rect {
    hit(
        app,
        |t| matches!(t,Target::ColumnBoundary{column,..} if *column==c),
    )
}
fn open(app: &mut App) {
    let r = header(app, Column::Name);
    app.handle(Input::SecondaryClick { x: r.x, y: r.y });
}
fn key(app: &mut App, k: Key) {
    app.handle(Input::Key(KeyPress::plain(k)));
}
fn drag(app: &mut App, c: Column, delta: u16) {
    let r = boundary(app, c);
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: r.x + delta,
        y: r.y,
    });
}
#[test]
fn equal_shares_custom_widths_and_double_click_follow_the_requested_example() {
    let mut l = Layout::default();
    assert_eq!(l.widths(100), [100]);
    l.toggle(Column::Modified);
    assert_eq!(l.widths(100), [50, 50]);
    l.toggle(Column::Created);
    assert_eq!(l.widths(99), [33, 33, 33]);
    l.columns = vec![
        Spec {
            column: Column::Name,
            share: 400_000,
            manual: true,
        },
        Spec {
            column: Column::Size,
            share: 200_000,
            manual: false,
        },
        Spec {
            column: Column::Modified,
            share: 300_000,
            manual: false,
        },
        Spec {
            column: Column::Created,
            share: 100_000,
            manual: true,
        },
    ];
    l.reset(Column::Created);
    assert_eq!(l.widths(100), [40, 20, 20, 20]);
    assert!(l.columns[0].manual && !l.columns[3].manual);
    l.toggle(Column::Owner);
    assert_eq!(l.widths(100), [40, 15, 15, 15, 15]);
    l.toggle(Column::Owner);
    assert_eq!(l.widths(100), [40, 20, 20, 20]);
    for w in [1, 9, 32, 101, 800, u16::MAX] {
        assert_eq!(
            l.widths(w).iter().map(|x| u32::from(*x)).sum::<u32>(),
            u32::from(w)
        );
    }
    let mut gesture = Layout::default();
    for c in [Column::Size, Column::Modified, Column::Created] {
        gesture.toggle(c);
    }
    gesture.resize(0, 100, 15);
    gesture.reset(Column::Size);
    gesture.resize(3, 100, -10);
    assert_eq!(gesture.widths(100), [40, 20, 30, 10]);
    gesture.reset(Column::Created);
    assert_eq!(gesture.widths(100), [40, 20, 20, 20]);
    l.normalize();
    assert_eq!(l.columns.iter().map(|s| s.share).sum::<u32>(), TOTAL);
}
#[test]
fn header_menu_checks_fields_at_minimum_size_without_click_through() {
    let (_t, mut app) = setup();
    for (w, h) in [(34, 14), (90, 30), (170, 53)] {
        render(&mut app, w, h);
        open(&mut app);
        render(&mut app, w, h);
        assert!(matches!(app.dialog, Some(Dialog::Columns(_))));
        assert_eq!(
            app.hits
                .iter()
                .filter(|(_, t)| matches!(t, Target::ColumnToggle { .. }))
                .count(),
            10
        );
        let fixed = hit(&app, |t| {
            matches!(
                t,
                Target::ColumnToggle {
                    column: Column::Name,
                    ..
                }
            )
        });
        click(&mut app, fixed, false);
        render(&mut app, w, h);
        assert!(
            app.settings
                .list_columns
                .columns
                .iter()
                .any(|s| s.column == Column::Name)
        );
        for c in [Column::Size, Column::Created, Column::Owner] {
            let before = app
                .settings
                .list_columns
                .columns
                .iter()
                .any(|s| s.column == c);
            let r = hit(
                &app,
                |t| matches!(t,Target::ColumnToggle{column,..} if *column==c),
            );
            click(&mut app, r, false);
            render(&mut app, w, h);
            assert_ne!(
                app.settings
                    .list_columns
                    .columns
                    .iter()
                    .any(|s| s.column == c),
                before
            );
        }
        let r = hit(&app, |t| matches!(t, Target::MenuBackground));
        app.handle(Input::Pointer(Some(Position::new(r.x, r.y))));
        assert!(app.hovered_area().is_none());
        key(&mut app, Key::Escape);
        assert!(app.dialog.is_none());
        render(&mut app, w, h);
    }
    for c in Column::ALL {
        if !app
            .settings
            .list_columns
            .columns
            .iter()
            .any(|s| s.column == c)
        {
            app.settings.list_columns.toggle(c);
        }
    }
    app.icon_backend = basalgrim::icons::IconBackend::Gui;
    for (w, h) in [(34, 14), (60, 20), (170, 53)] {
        render(&mut app, w, h);
    }
}
#[test]
fn mouse_resize_persists_relative_widths_cancels_and_rejects_stale_controls() {
    let (t, mut app) = setup();
    app.settings.list_columns.toggle(Column::Size);
    app.settings.list_columns.toggle(Column::Modified);
    render(&mut app, 170, 53);
    let original = app.settings.list_columns.clone();
    let selected = app.tab().entry().unwrap().path.clone();
    let old = app
        .hits
        .iter()
        .find(|(_, t)| {
            matches!(
                t,
                Target::ColumnBoundary {
                    column: Column::Name,
                    ..
                }
            )
        })
        .unwrap()
        .clone();
    drag(&mut app, Column::Name, 8);
    render(&mut app, 170, 53);
    assert_ne!(app.settings.list_columns, original);
    assert!(app.settings.list_columns.columns[0].manual);
    assert_eq!(app.tab().entry().unwrap().path, selected);
    assert!(!app.dragging_files());
    assert!(app.selection_area().is_none());
    let path = t.path().join("settings.json");
    app.persist_preferences(Some(&path));
    assert!(!path.exists());
    key(&mut app, Key::Escape);
    assert_eq!(app.settings.list_columns, original);
    render(&mut app, 170, 53);
    let edge = boundary(&app, Column::Name);
    drag(&mut app, Column::Name, 8);
    app.handle(Input::PointerUp {
        x: edge.x + 8,
        y: edge.y,
    });
    let saved = app.settings.list_columns.clone();
    app.persist_preferences(Some(&path));
    assert_eq!(Settings::load(&path).unwrap().list_columns, saved);
    render(&mut app, 34, 14);
    assert_eq!(app.settings.list_columns, saved);
    render(&mut app, 170, 53);
    drag(&mut app, Column::Size, 4);
    app.handle(Input::Pointer(None));
    assert_eq!(app.settings.list_columns, saved);
    render(&mut app, 170, 53);
    drag(&mut app, Column::Size, 4);
    render(&mut app, 90, 30);
    assert_eq!(app.settings.list_columns, saved);
    app.hits = vec![old.clone()];
    click(&mut app, old.0, true);
    assert_eq!(app.settings.list_columns, saved);
    render(&mut app, 170, 53);
    let r = boundary(&app, Column::Name);
    click(&mut app, r, true);
    assert!(!app.settings.list_columns.columns[0].manual);
    assert!(
        app.settings
            .list_columns
            .widths(120)
            .iter()
            .all(|w| *w == 40)
    );
}
#[test]
fn each_header_sorts_typed_values_both_ways_and_preserves_selected_paths() {
    let (_t, mut app) = setup();
    for c in Column::ALL {
        app.settings.list_columns.toggle(c);
    }
    let selected = app.tab().entry().unwrap().path.clone();
    app.tab_mut().marked.insert(selected.clone());
    app.tab_mut().explicit_selection = true;
    for c in Column::ALL {
        render(&mut app, 170, 53);
        let r = header(&app, c);
        click(&mut app, r, false);
        assert_eq!(app.tab().sort.field(), (c, c == Column::Name));
        render(&mut app, 170, 53);
        let r = header(&app, c);
        click(&mut app, r, false);
        assert_eq!(app.tab().sort.field(), (c, c != Column::Name));
        assert_eq!(app.tab().entry().unwrap().path, selected);
        assert!(app.tab().marked.contains(&selected));
    }
    for (c, descending) in [
        (Column::Size, false),
        (Column::Size, true),
        (Column::Created, true),
    ] {
        if c == Column::Created {
            app.tab_mut().entries[0].details.created = None;
            app.tab_mut().entries[1].details.created =
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(2));
            app.tab_mut().entries[2].details.created =
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(9));
        }
        app.tab_mut().sort = Sort::Field(c, descending);
        app.tab_mut().rebuild(false);
        let e = &app.tab().entries;
        if c == Column::Size {
            assert_eq!(
                e.iter().map(|e| e.size).collect::<Vec<_>>(),
                if descending {
                    vec![100, 20, 9]
                } else {
                    vec![9, 20, 100]
                }
            );
        } else {
            assert!(e.last().unwrap().details.created.is_none());
        }
    }
    app.action(Action::Refresh);
    settle(&mut app);
    assert_eq!(app.tab().sort.field(), (Column::Created, true));
}

#[test]
fn worker_metadata_uses_filesystem_birthtime_and_link_identity() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let (root, mut app) = setup();
    let file = root.path().join("a.rs");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    fs::File::open(&file)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100))
        .unwrap();
    let link = root.path().join("link");
    symlink(&file, &link).unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    for path in [&file, &link] {
        let e = app.tab().entries.iter().find(|e| e.path == *path).unwrap();
        let m = fs::symlink_metadata(path).unwrap();
        assert_eq!(e.details.created, m.created().ok());
        assert_eq!(e.details.mode, Some(m.mode()));
        assert_eq!(e.modified, m.modified().ok());
        assert!(e.details.owner.is_some());
    }
    let e = app.tab().entries.iter().find(|e| e.path == file).unwrap();
    assert_eq!(Column::Permissions.text(e), "0640");
    assert_eq!(Column::Extension.text(e), ".rs");
    assert_eq!(Column::Location.text(e), root.path().to_string_lossy());
    if e.details.created.is_some() {
        assert_ne!(e.details.created, e.modified);
    }
}
#[test]
fn opposite_pane_sort_is_independent_and_navigation_dismisses_stale_column_controls() {
    let (root, mut app) = setup();
    app.settings.dual = true;
    app.settings.list_columns.toggle(Column::Size);
    render(&mut app, 170, 53);
    let r = hit(
        &app,
        |t| matches!(t,Target::ColumnHeader{context,column:Column::Size} if context.owner.pane==1),
    );
    click(&mut app, r, false);
    assert_eq!(app.active, 1);
    assert_eq!(app.panes[1].tab().sort, Sort::Field(Column::Size, false));
    assert_eq!(app.panes[0].tab().sort, Sort::Name);
    render(&mut app, 170, 53);
    open(&mut app);
    render(&mut app, 170, 53);
    let stale = app
        .hits
        .iter()
        .find(|(_, t)| {
            matches!(
                t,
                Target::ColumnToggle {
                    column: Column::Created,
                    ..
                }
            )
        })
        .unwrap()
        .clone();
    let before = app.settings.list_columns.clone();
    let next = root.path().join("next");
    fs::create_dir(&next).unwrap();
    app.navigate(next, true);
    settle(&mut app);
    assert!(app.dialog.is_none());
    app.hits = vec![stale.clone()];
    click(&mut app, stale.0, false);
    assert_eq!(app.settings.list_columns, before);
}

#[test]
fn all_border_styles_keep_header_and_checkbox_controls_inside_compact_windows() {
    let (_t, mut app) = setup();
    for border in basalgrim::preferences::BorderWeight::ALL {
        app.settings.appearance.borders = border;
        for (w, h) in [(34, 14), (50, 22), (170, 53)] {
            render(&mut app, w, h);
            open(&mut app);
            render(&mut app, w, h);
            assert_eq!(
                app.hits
                    .iter()
                    .filter(|(_, t)| matches!(t, Target::ColumnToggle { .. }))
                    .count(),
                10
            );
            let r = hit(&app, |t| {
                matches!(
                    t,
                    Target::ColumnToggle {
                        column: Column::Accessed,
                        ..
                    }
                )
            });
            click(&mut app, r, false);
            render(&mut app, w, h);
            key(&mut app, Key::Escape);
            render(&mut app, w, h);
        }
    }
}

use basalgrim::{
    app::{App, Dialog, EditKind, PlaceControl, Target},
    input::{Action, Input, KeyPress},
    places::{Command, Config, Section},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    while app.loading() || app.library.pending > 0 {
        app.tick();
        assert!(
            Instant::now() < end,
            "{} / {:?}",
            app.notice,
            app.library.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    app.tick();
}
fn config(root: &Path) -> Config {
    Config {
        database: root.join("data/places.sqlite3"),
        bookmarks: root.join("gtk-bookmarks"),
        legacy_bookmarks: root.join("legacy"),
        recent: root.join("recent.xbel"),
    }
}
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= w && r.bottom() <= h)
    );
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn hit(app: &App, p: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .find_map(|(r, t)| p(t).then_some(*r))
        .expect("missing mouse control")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn control(app: &mut App, c: PlaceControl) {
    let expected = std::mem::discriminant(&c);
    let r = hit(
        app,
        |t| matches!(t,Target::PlacesControl{control,..} if std::mem::discriminant(control)==expected),
    );
    click(app, r);
}
#[test]
fn places_use_mouse_at_minimum_size_and_save_bookmark_labels_without_renaming_folders() {
    for grid in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("current");
        fs::create_dir(&root).unwrap();
        let mut app = App::new(
            root.clone(),
            Settings {
                grid,
                preview: false,
                ..Default::default()
            },
        );
        app.configure_places(config(temp.path()));
        settle(&mut app);
        app.action(Action::Places);
        settle(&mut app);
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Add);
        settle(&mut app);
        assert_eq!(app.library.snapshot.bookmarks[0].path, root);
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Rename);
        assert!(matches!(
            app.dialog,
            Some(Dialog::Edit {
                kind: EditKind::PlaceLabel { .. },
                ..
            })
        ));
        app.handle(Input::Text("Рабочие проекты".into()));
        app.confirm_dialog();
        settle(&mut app);
        assert_eq!(app.library.snapshot.bookmarks[0].label, "Рабочие проекты");
        assert!(root.exists());
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Filter);
        app.handle(Input::Text("нет совпадений".into()));
        app.confirm_dialog();
        assert!(app.places_view.rows.is_empty());
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Filter);
        app.handle(Input::Key(KeyPress::ctrl('u')));
        app.confirm_dialog();
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Remove);
        settle(&mut app);
        assert!(app.library.snapshot.bookmarks.is_empty());
        assert!(root.exists());
    }
}
#[test]
fn favorite_groups_and_recent_reveal_preserve_files_and_ignore_obsolete_hits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    fs::create_dir(&root).unwrap();
    for name in ["one.txt", "two.txt"] {
        fs::write(root.join(name), name).unwrap();
    }
    let mut app = App::new(
        root.clone(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.configure_places(config(temp.path()));
    settle(&mut app);
    app.action(Action::SelectAll);
    app.action(Action::Favorite);
    settle(&mut app);
    assert_eq!(app.library.snapshot.favorites.len(), 2);
    app.action(Action::Favorites);
    settle(&mut app);
    render(&mut app, 80, 24);
    let obsolete = hit(&app, |t| {
        matches!(
            t,
            Target::PlacesControl {
                control: PlaceControl::Remove,
                ..
            }
        )
    });
    app.open_places(Section::Bookmarks);
    click(&mut app, obsolete);
    settle(&mut app);
    assert_eq!(app.library.snapshot.favorites.len(), 2);
    app.library.send(Command::Recent(root.join("two.txt")));
    settle(&mut app);
    app.open_places(Section::Recent);
    settle(&mut app);
    render(&mut app, 34, 14);
    control(&mut app, PlaceControl::Reveal);
    settle(&mut app);
    assert!(app.dialog.is_none());
    assert_eq!(app.tab().entry().unwrap().path, root.join("two.txt"));
    app.open_places(Section::Favorites);
    settle(&mut app);
    render(&mut app, 34, 14);
    control(&mut app, PlaceControl::Remove);
    settle(&mut app);
    assert_eq!(app.library.snapshot.favorites.len(), 1);
    assert_eq!(fs::read_dir(root).unwrap().count(), 2);
}
#[test]
fn gtk_refresh_sidebar_navigation_missing_paths_and_cancelled_resolutions_are_guarded() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    let target = temp.path().join("target");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&target).unwrap();
    let c = config(temp.path());
    fs::write(
        &c.bookmarks,
        format!(
            "{} Work\n",
            basalgrim::clipboard::files::uri(&target).unwrap()
        ),
    )
    .unwrap();
    let mut app = App::new(
        root.clone(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.configure_places(c.clone());
    settle(&mut app);
    render(&mut app, 150, 40);
    let bookmark = hit(
        &app,
        |t| matches!(t,Target::SavedPlace(path) if path==&target),
    );
    click(&mut app, bookmark);
    settle(&mut app);
    assert_eq!(app.tab().cwd, target);
    app.place_open(root.clone(), false);
    app.navigate(temp.path().into(), true);
    settle(&mut app);
    assert_eq!(app.tab().cwd, temp.path());
    app.place_open(root.clone(), false);
    app.dialog = Some(Dialog::Help);
    settle(&mut app);
    assert_eq!(app.tab().cwd, temp.path());
    assert!(matches!(app.dialog, Some(Dialog::Help)));
    app.cancel_dialog();
    fs::write(&c.bookmarks, "file:///missing-basalgrim-place Missing\n").unwrap();
    app.library.send(Command::Refresh);
    settle(&mut app);
    app.open_places(Section::Bookmarks);
    settle(&mut app);
    render(&mut app, 34, 14);
    control(&mut app, PlaceControl::Open);
    settle(&mut app);
    assert!(app.notice.contains("missing-basalgrim-place"));
    assert!(matches!(app.dialog, Some(Dialog::Places)));
    render(&mut app, 34, 14);
    control(&mut app, PlaceControl::Remove);
    settle(&mut app);
    assert!(app.library.snapshot.bookmarks.is_empty());
    assert_eq!(
        fs::read_to_string(&c.bookmarks).unwrap(),
        "file:///missing-basalgrim-place Missing\n"
    );
}

#[test]
fn imported_bookmark_can_be_removed_with_mouse_and_stays_removed_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let videos = temp.path().join("Видео");
    fs::create_dir(&videos).unwrap();
    fs::write(videos.join("keep.txt"), "keep").unwrap();
    let c = config(temp.path());
    let gtk = format!(
        "{} Видео\n",
        basalgrim::clipboard::files::uri(&videos).unwrap()
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    for grid in [false, true] {
        let c = Config {
            database: temp.path().join(format!("places-{grid}.sqlite3")),
            ..c.clone()
        };
        let mut app = App::new(
            temp.path().into(),
            Settings {
                grid,
                ..Default::default()
            },
        );
        app.configure_places(c.clone());
        settle(&mut app);
        app.action(Action::Places);
        settle(&mut app);
        assert_eq!(app.places_view.rows[0].path, videos);
        assert!(app.places_view.rows[0].desktop);
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Remove);
        settle(&mut app);
        assert!(app.places_view.rows.is_empty());
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Refresh);
        settle(&mut app);
        assert!(app.places_view.rows.is_empty());
        app.cancel_dialog();
        render(&mut app, 150, 40);
        assert!(
            !app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::SavedPlace(p) if p == &videos))
        );
        drop(app);
        let mut restarted = App::new(videos.clone(), Settings::default());
        restarted.configure_places(c.clone());
        settle(&mut restarted);
        assert!(restarted.library.snapshot.bookmarks.is_empty());
        restarted.action(Action::Places);
        settle(&mut restarted);
        render(&mut restarted, 34, 14);
        control(&mut restarted, PlaceControl::Add);
        settle(&mut restarted);
        assert_eq!(restarted.places_view.rows.len(), 1);
        assert_eq!(restarted.places_view.rows[0].path, videos);
        assert_eq!(fs::read_to_string(videos.join("keep.txt")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(&c.bookmarks).unwrap(), gtk);
    }
}
#[test]
fn default_place_bindings_preserve_existing_shortcuts_and_app_new_never_attaches_storage() {
    let mut settings = Settings::default();
    settings.normalize().unwrap();
    assert_eq!(settings.shortcut(Action::TrashView), "Ctrl+Shift+B");
    assert_eq!(settings.shortcut(Action::RestoreTrashHere), "Ctrl+Alt+R");
    let root = tempfile::tempdir().unwrap();
    let app = App::new(root.path().into(), settings);
    assert!(!app.library.ready);
    assert_eq!(app.library.pending, 0);
}

#[test]
fn gtk_bookmarks_skip_quick_access_duplicates_and_recent_rows_hide_the_import_source() {
    use basalgrim::app::Place;
    let temp = tempfile::tempdir().unwrap();
    let built_in = temp.path().join("Documents");
    let custom = temp.path().join("Projects");
    fs::create_dir(&built_in).unwrap();
    fs::create_dir(&custom).unwrap();
    let c = config(temp.path());
    let uri = basalgrim::clipboard::files::uri;
    let gtk = format!(
        "{} Duplicate standard\n{} My projects\n",
        uri(&built_in).unwrap(),
        uri(&custom).unwrap()
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    fs::write(
        &c.recent,
        format!(
            "<xbel><bookmark href='{}'><title>Recent document</title></bookmark></xbel>",
            uri(&built_in.join("file.txt")).unwrap()
        ),
    )
    .unwrap();
    let mut app = App::new(built_in.clone(), Settings::default());
    app.places = vec![Place {
        label: "Documents",
        path: built_in.clone(),
    }];
    app.configure_places(c.clone());
    settle(&mut app);
    let frame = render(&mut app, 150, 40);
    assert!(!frame.contains("Duplicate standard"));
    assert_eq!(
        app.hits
            .iter()
            .filter(
                |(_, t)| matches!(t, Target::Place(p) | Target::SavedPlace(p) if p == &built_in)
            )
            .count(),
        1
    );
    assert!(
        app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::SavedPlace(p) if p == &custom))
    );
    app.action(Action::Places);
    settle(&mut app);
    assert_eq!(app.places_view.rows.len(), 1);
    assert_eq!(app.places_view.rows[0].path, custom);
    for (w, h) in [(150, 40), (34, 14)] {
        let frame = render(&mut app, w, h);
        assert!(!frame.contains("GTK") && !frame.contains("Duplicate standard"));
        assert!(frame.contains("My projects"));
    }
    // Adding a standard folder does not create an invisible duplicate in our store.
    control(&mut app, PlaceControl::Add);
    settle(&mut app);
    assert!(app.library.snapshot.bookmarks.iter().all(|i| i.desktop));
    assert!(app.notice.contains("быстром доступе"));
    app.open_places(Section::Recent);
    settle(&mut app);
    let frame = render(&mut app, 34, 14);
    assert!(frame.contains("Recent document") && !frame.contains("GTK"));
    // A user's previously saved label must remain editable in the manager.
    app.library.send(Command::Bookmark(built_in.clone()));
    settle(&mut app);
    app.open_places(Section::Bookmarks);
    settle(&mut app);
    assert!(
        app.places_view
            .rows
            .iter()
            .any(|i| i.path == built_in && !i.desktop)
    );
    assert_eq!(fs::read_to_string(&c.bookmarks).unwrap(), gtk);
}

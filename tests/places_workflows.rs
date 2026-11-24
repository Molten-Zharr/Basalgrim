use basalgrim::{
    app::{App, Dialog, EditKind, PlaceControl, Target},
    input::{Action, Input, Key, KeyPress},
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
fn select_place(app: &mut App, path: &Path) {
    let index = app
        .places_view
        .rows
        .iter()
        .position(|item| item.path == path)
        .unwrap();
    app.handle(Input::Key(KeyPress::plain(Key::Home)));
    for _ in 0..index {
        app.handle(Input::Key(KeyPress::plain(Key::Down)));
    }
    render(app, 34, 14);
    let row = hit(
        app,
        |t| matches!(t, Target::PlacesRow { index: i, .. } if *i == index),
    );
    click(app, row);
    render(app, 34, 14);
}
fn hover(app: &mut App, row: Rect, w: u16, h: u16) {
    app.handle(Input::Pointer(Some((row.x, row.y).into())));
    render(app, w, h);
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
fn folder_context_favorite_stays_clickable_during_places_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    let folder = parent.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let mut app = App::new(parent, Settings::default());
    app.configure_places(config(temp.path()));
    settle(&mut app);

    for action in [Action::Favorite, Action::Unfavorite] {
        render(&mut app, 100, 38);
        let entry = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
        app.handle(Input::SecondaryClick {
            x: entry.x + entry.width / 2,
            y: entry.y,
        });
        let before = render(&mut app, 100, 38);
        assert!(before.contains(action.name()));
        let index = app
            .command_items()
            .iter()
            .position(|i| i.action == action)
            .unwrap();
        let row = hit(&app, |t| matches!(t, Target::Command(i) if *i == index));
        assert!(app.library.send(Command::Refresh));
        assert_eq!(before, render(&mut app, 100, 38), "refresh moved the menu");
        assert_eq!(
            row,
            hit(&app, |t| matches!(t, Target::Command(i) if *i == index))
        );
        click(&mut app, row);
        assert!(
            !app.library.writable(),
            "a favorite edit is already pending"
        );
        settle(&mut app);
        let favorites = &app.library.snapshot.favorites;
        if action == Action::Favorite {
            assert_eq!(favorites.len(), 1);
            assert_eq!(favorites[0].path, folder);
        } else {
            assert!(favorites.is_empty());
        }
    }
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
        select_place(&mut app, &root);
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
        select_place(&mut app, &root);
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
    select_place(&mut app, Path::new("/missing-basalgrim-place"));
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
        select_place(&mut app, &videos);
        assert_eq!(
            app.places_view.rows[app.places_view.selected].source,
            basalgrim::places::PlaceSource::Desktop
        );
        control(&mut app, PlaceControl::Remove);
        settle(&mut app);
        assert!(app.places_view.rows.iter().all(|i| i.path != videos));
        render(&mut app, 34, 14);
        control(&mut app, PlaceControl::Refresh);
        settle(&mut app);
        assert!(app.places_view.rows.iter().all(|i| i.path != videos));
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
        assert_eq!(
            restarted
                .places_view
                .rows
                .iter()
                .filter(|i| i.path == videos)
                .count(),
            1
        );
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
    assert_eq!(app.places_view.rows.len(), 2);
    assert_eq!(app.places_view.rows[0].path, built_in);
    assert_eq!(app.places_view.rows[1].path, custom);
    for (w, h) in [(150, 40), (34, 14)] {
        let frame = render(&mut app, w, h);
        assert!(!frame.contains("GTK") && !frame.contains("Duplicate standard"));
        assert!(frame.contains("My projects"));
    }
    // Adding a standard folder does not create an invisible duplicate in our store.
    control(&mut app, PlaceControl::Add);
    settle(&mut app);
    assert!(
        app.library
            .snapshot
            .bookmarks
            .iter()
            .all(|i| i.source == basalgrim::places::PlaceSource::Desktop)
    );
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
            .any(|i| i.path == built_in && i.source == basalgrim::places::PlaceSource::Personal)
    );
    assert_eq!(fs::read_to_string(&c.bookmarks).unwrap(), gtk);
}

#[test]
fn dragging_folders_and_breadcrumbs_pins_links_across_restart() {
    use basalgrim::app::{BookmarkDrop, PanelKind};
    for (width, height) in [(150, 40), (34, 14)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        let folder = root.join("project");
        fs::create_dir_all(&folder).unwrap();
        let file = root.join("file.txt");
        fs::write(&file, "keep").unwrap();
        let c = config(temp.path());
        let mut app = App::new(
            root.clone(),
            Settings {
                grid: false,
                preview: false,
                ..Default::default()
            },
        );
        app.places.retain(|p| p.path == Path::new("/"));
        app.configure_places(c.clone());
        settle(&mut app);
        for (source, expected) in [
            (0, BookmarkDrop::Pin),
            (0, BookmarkDrop::Pin),
            (1, BookmarkDrop::FoldersOnly),
            (2, BookmarkDrop::Pin),
        ] {
            render(&mut app, width, height);
            let from = hit(&app, |t| match t {
                Target::Entry { pane: 0, index } => *index == source,
                Target::PathEdit(_) => source == 2,
                _ => false,
            });
            let to = hit(&app, |t| {
                matches!(t, Target::Place(_) | Target::PanelOpen(PanelKind::Places))
            });
            let x = from.x + from.width / 2;
            app.handle(Input::PointerDown {
                x,
                y: from.y,
                double: false,
                toggle: false,
            });
            app.handle(Input::Drag { x: to.x, y: to.y });
            app.handle(Input::DragModifiers { copy: true });
            let feedback = app.drag_feedback().expect("held drag");
            assert_eq!(feedback.bookmark, Some(expected));
            assert_eq!(feedback.area.is_some(), expected == BookmarkDrop::Pin);
            assert!(feedback.destination.is_none());
            app.handle(Input::PointerUp { x: to.x, y: to.y });
            settle(&mut app);
            assert!(app.dialog.is_none() && !app.busy);
            assert_eq!(app.tab().cwd, root);
            assert_eq!(
                app.library.snapshot.bookmarks.len(),
                if source == 2 { 2 } else { 1 }
            );
        }
        assert_eq!(fs::read_to_string(file).unwrap(), "keep");
        drop(app);
        let mut app = App::new(root.clone(), Settings::default());
        app.places.retain(|p| p.path == Path::new("/"));
        app.configure_places(c);
        settle(&mut app);
        render(&mut app, width, height);
        if width == 34 {
            let open = hit(&app, |t| matches!(t, Target::PanelOpen(PanelKind::Places)));
            click(&mut app, open);
            render(&mut app, width, height);
        }
        let link = hit(&app, |t| match t {
            Target::SavedPlace(p) => *p == folder,
            Target::PanelLink { target, .. } => {
                matches!(target.as_ref(), Target::SavedPlace(p) if *p == folder)
            }
            _ => false,
        });
        click(&mut app, link);
        settle(&mut app);
        assert_eq!(app.tab().cwd, folder);
    }
}

#[test]
fn pinning_a_marked_group_preserves_selection_and_rejects_mixed_or_cancelled_drops() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    let a = root.join("a");
    let b = root.join("b");
    let file = root.join("c.txt");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(&file, "contents").unwrap();
    let mut app = App::new(root, Settings::default());
    app.configure_places(config(temp.path()));
    settle(&mut app);
    for (include_file, cancel) in [(true, false), (false, true), (false, false)] {
        let mut marks = std::collections::BTreeSet::from([a.clone(), b.clone()]);
        if include_file {
            marks.insert(file.clone());
        }
        app.tab_mut().marked = marks.clone();
        app.tab_mut().explicit_selection = true;
        render(&mut app, 150, 40);
        let from = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
        let dest = hit(&app, |t| matches!(t, Target::Place(_)));
        app.handle(Input::PointerDown {
            x: from.x + from.width / 2,
            y: from.y,
            double: false,
            toggle: false,
        });
        app.handle(Input::Drag {
            x: dest.x,
            y: dest.y,
        });
        if cancel {
            app.handle(Input::CancelDrag);
        }
        app.handle(Input::PointerUp {
            x: dest.x,
            y: dest.y,
        });
        settle(&mut app);
        assert_eq!(app.tab().marked, marks);
        let paths: std::collections::BTreeSet<_> = app
            .library
            .snapshot
            .bookmarks
            .iter()
            .map(|i| i.path.clone())
            .collect();
        if cancel || include_file {
            assert!(paths.is_empty());
        } else {
            assert_eq!(paths, marks);
        }
    }
}

#[test]
fn first_pin_preserves_spacing_and_its_own_remove_button_survives_overflow() {
    use basalgrim::app::Place;
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path());
    let folder = temp.path().join("ZedMono");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("keep"), b"keep").unwrap();
    let mut app = App::new(temp.path().into(), Settings::default());
    app.places = [
        "Домой",
        "Загрузки",
        "Документы",
        "Музыка",
        "Изображения",
        "Проекты",
        "/",
    ]
    .map(|label| Place {
        label,
        path: temp.path().join(label),
    })
    .into();
    app.configure_places(config.clone());
    settle(&mut app);
    let positions = |app: &App| -> Vec<Rect> {
        app.hits
            .iter()
            .filter_map(|(r, t)| matches!(t, Target::Place(_)).then_some(*r))
            .collect()
    };
    render(&mut app, 150, 40);
    let before = positions(&app);
    assert_eq!(before.len(), 7);
    assert!(before.windows(2).all(|rows| rows[1].y == rows[0].y + 2));
    app.library.send(Command::Bookmark(folder.clone()));
    settle(&mut app);
    render(&mut app, 150, 40);
    assert_eq!(before, positions(&app));
    assert!(
        !app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::UnpinPlace { .. }))
    );
    let row = hit(&app, |t| matches!(t, Target::SavedPlace(p) if p == &folder));
    hover(&mut app, row, 150, 40);
    let link = hit(&app, |t| matches!(t, Target::SavedPlace(p) if p == &folder));
    let cross = hit(
        &app,
        |t| matches!(t, Target::UnpinPlace{path,..} if path == &folder),
    );
    assert_eq!(link.y, before.last().unwrap().y + 2);
    assert_eq!((link.y, link.right()), (cross.y, cross.x));
    let cwd = app.tab().cwd.clone();
    click(&mut app, cross);
    settle(&mut app);
    render(&mut app, 150, 40);
    assert_eq!(before, positions(&app));
    assert!(app.library.snapshot.bookmarks.is_empty());
    assert_eq!(app.tab().cwd, cwd);
    drop(app);

    let mut app = App::new(cwd, Settings::default());
    app.configure_places(config);
    settle(&mut app);
    assert!(app.library.snapshot.bookmarks.is_empty());
    assert_eq!(fs::read(folder.join("keep")).unwrap(), b"keep");

    let extras: Vec<_> = (0..24)
        .map(|i| temp.path().join(format!("folder-{i}")))
        .collect();
    for p in &extras {
        fs::create_dir(p).unwrap();
    }
    app.library.send(Command::BookmarkFolders {
        paths: extras.clone(),
        standard: vec![],
    });
    settle(&mut app);
    render(&mut app, 100, 24);
    let down = hit(&app, |t| matches!(t, Target::QuickAccessScroll(1)));
    for _ in 0..app.design.quick_access_max {
        click(&mut app, down);
        render(&mut app, 100, 24);
    }
    let last = extras.last().unwrap();
    let row = hit(&app, |t| matches!(t, Target::SavedPlace(p) if p == last));
    hover(&mut app, row, 100, 24);
    let cross = hit(
        &app,
        |t| matches!(t, Target::UnpinPlace{path,..} if path == last),
    );
    click(&mut app, cross);
    settle(&mut app);
    assert!(
        app.library
            .snapshot
            .bookmarks
            .iter()
            .all(|p| p.path != *last)
    );
    assert_eq!(app.library.snapshot.bookmarks.len(), extras.len() - 1);
}

#[test]
fn compact_places_delete_unpins_the_focused_standard_or_imported_row() {
    use basalgrim::{app::PanelKind, input::Key};
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("imported");
    fs::create_dir(&folder).unwrap();
    let c = config(temp.path());
    let gtk = format!(
        "{} Imported\n",
        basalgrim::clipboard::files::uri(&folder).unwrap()
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    let mut app = App::new(temp.path().into(), Settings::default());
    app.places.retain(|place| place.path == Path::new("/"));
    app.configure_places(c.clone());
    settle(&mut app);
    render(&mut app, 34, 14);
    let open = hit(&app, |t| matches!(t, Target::PanelOpen(PanelKind::Places)));
    click(&mut app, open);
    render(&mut app, 34, 14);
    app.handle(Input::Key(KeyPress::plain(Key::Delete)));
    assert_eq!(app.library.pending, 0);
    app.handle(Input::Key(KeyPress::plain(Key::Down)));
    render(&mut app, 34, 14);
    app.handle(Input::Key(KeyPress::plain(Key::Delete)));
    settle(&mut app);
    assert!(matches!(app.dialog, Some(Dialog::Panel(PanelKind::Places))));
    assert!(
        app.library
            .snapshot
            .hidden_bookmarks
            .contains(Path::new("/"))
    );
    assert_eq!(app.library.snapshot.bookmarks[0].path, folder);
    render(&mut app, 34, 14);
    app.handle(Input::Key(KeyPress::plain(Key::Delete)));
    settle(&mut app);
    assert!(app.library.snapshot.bookmarks.is_empty());
    assert!(folder.is_dir());
    assert_eq!(fs::read_to_string(c.bookmarks).unwrap(), gtk);
}

#[test]
fn all_standard_places_can_be_hidden_and_restored_from_the_empty_quick_access_list() {
    use basalgrim::app::Place;
    let temp = tempfile::tempdir().unwrap();
    let downloads = temp.path().join("Downloads");
    fs::create_dir(&downloads).unwrap();
    let c = config(temp.path());
    let gtk = format!(
        "{} GTK Downloads\n",
        basalgrim::clipboard::files::uri(&downloads).unwrap()
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    let launch = || {
        let mut app = App::new(downloads.clone(), Settings::default());
        app.places = vec![Place {
            label: "Загрузки",
            path: downloads.clone(),
        }];
        app.configure_places(c.clone());
        settle(&mut app);
        app
    };
    let mut app = launch();
    render(&mut app, 150, 40);
    let row = hit(&app, |t| matches!(t, Target::Place(p) if *p == downloads));
    hover(&mut app, row, 150, 40);
    assert_eq!(
        app.hits
            .iter()
            .filter(|(_, t)| matches!(t, Target::UnpinPlace { .. }))
            .count(),
        1
    );
    app.handle(Input::Pointer(None));
    render(&mut app, 150, 40);
    assert!(
        !app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::UnpinPlace { .. }))
    );
    hover(&mut app, row, 150, 40);
    let cross = hit(
        &app,
        |t| matches!(t, Target::UnpinPlace { path, .. } if *path == downloads),
    );
    click(&mut app, cross);
    settle(&mut app);
    assert_eq!(app.tab().cwd, downloads);
    drop(app);

    let mut app = launch();
    let frame = render(&mut app, 150, 40);
    assert!(!frame.contains("Загрузки") && !frame.contains("GTK Downloads"));
    let header = hit(&app, |t| {
        matches!(t, Target::PlacesSection(Section::Bookmarks))
    });
    click(&mut app, header);
    settle(&mut app);
    assert!(app.places_view.rows.is_empty());
    assert!(render(&mut app, 34, 14).contains("В быстром доступе пока пусто"));
    control(&mut app, PlaceControl::Add);
    settle(&mut app);
    assert_eq!(app.places_view.rows.len(), 1);
    assert_eq!(app.places_view.rows[0].label, "Загрузки");
    assert_eq!(app.places_view.rows[0].path, downloads);
    assert!(app.library.snapshot.hidden_bookmarks.is_empty());
    assert_eq!(fs::read_to_string(c.bookmarks).unwrap(), gtk);
}

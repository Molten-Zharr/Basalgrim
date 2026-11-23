use basalgrim::{
    app::{App, DetailControl, StyleControl, Target},
    folder_style::{Change, FolderId, FolderStyle},
    icons::IconBackend,
    input::{Action, Input, Key, KeyPress},
    places::{Command, Config, Store},
    settings::Settings,
    ui,
    view::ViewMode,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn config(base: &Path) -> Config {
    Config {
        database: base.join("places.sqlite3"),
        bookmarks: base.join("gtk"),
        legacy_bookmarks: base.join("legacy"),
        recent: base.join("recent"),
    }
}
fn id(path: &Path) -> FolderId {
    FolderId::capture(path, &fs::symlink_metadata(path).unwrap()).unwrap()
}

#[test]
fn imported_image_survives_restart_and_source_removal_and_stale_edits_are_rejected() {
    use std::os::unix::ffi::OsStrExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let c = config(root);
    let folder = root.join(std::ffi::OsStr::from_bytes(b"raw-\xff"));
    fs::create_dir(&folder).unwrap();
    let key = id(&folder);
    let icon = root.join("red.svg");
    fs::write(&icon,r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="64"><rect width="128" height="64" fill="#ff0000"/></svg>"##).unwrap();
    let style = FolderStyle {
        icon: icon.to_str().unwrap().into(),
        color: Some([1, 2, 3]),
        tag: "Мастерская".into(),
    };
    let change = |style, expected| Command::SetStyle {
        token: 1,
        change: Change {
            folder: key.clone(),
            path: folder.clone(),
            style,
            expected,
        },
    };
    let mut store = Store::open(&c.database).unwrap();
    store.apply(change(style.clone(), None)).unwrap();
    let mut second = Store::open(&c.database).unwrap();
    let saved = second.snapshot(&c).unwrap().styles[&key].clone();
    let frame = saved.image.unwrap();
    assert_eq!((frame.width, frame.height), (64, 32));
    assert_eq!(frame.pixels[..4], [255, 0, 0, 255]);
    assert_eq!(
        frame.id(),
        second.snapshot(&c).unwrap().styles[&key]
            .image
            .as_ref()
            .unwrap()
            .id()
    );
    fs::remove_file(icon).unwrap();
    assert!(second.apply(change(Default::default(), None)).is_err());
    let updated = FolderStyle {
        tag: "Новый".into(),
        ..style
    };
    second
        .apply(change(updated.clone(), Some(saved.revision)))
        .unwrap();
    let item = store.snapshot(&c).unwrap().styles[&key].clone();
    assert_eq!(item.style, updated);
    assert_eq!(&item.image.unwrap().pixels[..4], &[255, 0, 0, 255]);
    assert!(
        store
            .apply(change(Default::default(), Some(saved.revision)))
            .is_err()
    );
    let broken = root.join("bad.png");
    fs::write(&broken, b"invalid").unwrap();
    assert!(
        store
            .apply(change(
                FolderStyle {
                    icon: broken.to_str().unwrap().into(),
                    ..updated.clone()
                },
                Some(item.revision)
            ))
            .is_err()
    );
    assert_eq!(store.snapshot(&c).unwrap().styles[&key].style, updated);
    let renamed = root.join("kept");
    fs::rename(&folder, &renamed).unwrap();
    if fs::metadata(&renamed).unwrap().created().is_ok() {
        assert_eq!(id(&renamed), key);
    }
    fs::create_dir(&folder).unwrap();
    assert_ne!(id(&folder), key);
    assert!(
        second
            .apply(change(Default::default(), Some(item.revision)))
            .is_err()
    );
    fs::remove_dir(&folder).unwrap();
    fs::rename(renamed, &folder).unwrap();
    store
        .apply(change(Default::default(), Some(item.revision)))
        .unwrap();
    assert!(store.snapshot(&c).unwrap().styles.is_empty());
}

fn wait(app: &mut App, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready(app) {
        app.tick();
        assert!(Instant::now() < deadline, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn draw(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(app, f)).unwrap();
    term.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn click(app: &mut App, choose: impl Fn(StyleControl) -> bool) {
    draw(app, 34, 14);
    let (r, _) = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t,Target::Detail{control:DetailControl::Style(c),..} if choose(*c)))
        .unwrap();
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
#[test]
fn compact_property_editor_saves_only_its_folder_and_all_views_use_the_decoration() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("files");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("alpha")).unwrap();
    fs::write(root.join("alpha/sample"), [0; 73]).unwrap();
    fs::create_dir(root.join("beta")).unwrap();
    let mut app = App::new(
        root.clone(),
        Settings {
            view: Some(ViewMode::List),
            preview: false,
            ..Default::default()
        },
    );
    app.configure_places(config(t.path()));
    wait(&mut app, |a| !a.loading() && a.library.ready);
    app.action(Action::Properties);
    wait(&mut app, |a| {
        a.detail_view
            .as_ref()
            .is_some_and(|v| v.properties.is_some())
    });
    click(&mut app, |c| matches!(c, StyleControl::Open));
    click(&mut app, |c| matches!(c, StyleControl::Symbol(0)));
    key(&mut app, Key::Tab);
    app.handle(Input::Text("#00ABCD".into()));
    key(&mut app, Key::Tab);
    app.handle(Input::Paste("Дела".into()));
    click(&mut app, |c| matches!(c, StyleControl::Save));
    wait(&mut app, |a| a.dialog.is_none());
    let record = &app.library.snapshot.styles[&id(&root.join("alpha"))];
    assert_eq!(
        record.style,
        FolderStyle {
            icon: "♣".into(),
            color: Some([0, 171, 205]),
            tag: "Дела".into()
        }
    );
    for mode in [
        ViewMode::Icons,
        ViewMode::List,
        ViewMode::Columns,
        ViewMode::Tree,
    ] {
        app.settings.view = Some(mode);
        for backend in [IconBackend::Gui, IconBackend::Symbols] {
            app.icon_backend = backend;
            let screen = draw(&mut app, 100, 35);
            assert!(
                screen.contains("♣") && screen.contains("#Дела"),
                "{mode:?} {backend:?}"
            );
            assert_eq!(screen.matches("#Дела").count(), 1);
        }
    }
    app.settings.view = Some(ViewMode::Icons);
    app.settings.auto_folder_sizes = true;
    draw(&mut app, 100, 35);
    wait(&mut app, |a| {
        a.folder_sizes.cached(&root.join("alpha")).is_some()
    });
    let screen = draw(&mut app, 100, 35);
    assert!(screen.contains("#Дела") && screen.contains("≈73 B"));
    app.action(Action::Properties);
    wait(&mut app, |a| {
        a.detail_view
            .as_ref()
            .is_some_and(|v| v.properties.is_some())
    });
    key(&mut app, Key::F(2));
    click(&mut app, |c| matches!(c, StyleControl::Reset));
    key(&mut app, Key::Escape);
    assert!(draw(&mut app, 100, 35).contains("#Дела"));
}

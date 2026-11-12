use basalgrim::{
    app::{App, SettingsSection, Target},
    desktop::{Kind, Request, resolve},
    options::Options,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
};
#[test]
fn local_uri_requests_preserve_raw_names_links_groups_and_reject_invalid_batches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let raw = root.join(OsString::from_vec(b"hidden-\xff".to_vec()));
    fs::write(&raw, "bytes").unwrap();
    let link = root.join("link");
    symlink(&raw, &link).unwrap();
    let uris = [&raw, &link].map(|p| basalgrim::clipboard::files::uri(p).unwrap());
    let batch = resolve(Request::from_uris(Kind::Items, &uris, "test".into()).unwrap()).unwrap();
    assert_eq!(batch.locations.len(), 1);
    assert_eq!(batch.locations[0].selected, vec![raw, link]);
    for uri in [
        "file://remote/tmp/a",
        "file:///tmp/%00",
        "file:///tmp/%xy",
        "https://example.com/a",
        "file:///a/../b",
    ] {
        assert!(Request::from_uris(Kind::Items, &[uri.into()], String::new()).is_err());
    }
    assert!(
        Request::from_uris(Kind::Items, &vec!["file:///tmp".into(); 65], String::new()).is_err()
    );
    assert!(Request::from_uris(Kind::Open, &[], "a".repeat(2049)).is_err());
    let file = root.join("file");
    fs::write(&file, "data").unwrap();
    assert!(
        resolve(Request {
            kind: Kind::Folders,
            paths: vec![file.clone()],
            startup_id: String::new()
        })
        .is_err()
    );
    let properties = resolve(Request {
        kind: Kind::Properties,
        paths: vec![file.clone()],
        startup_id: String::new(),
    })
    .unwrap();
    assert!(properties.locations[0].properties);
    assert_eq!(properties.locations[0].selected, vec![file]);
}
#[test]
fn cli_supports_uri_paths_dash_names_file_reveal_and_explicit_session_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("-a% b");
    fs::write(&file, "test").unwrap();
    let uri = basalgrim::clipboard::files::uri(&file).unwrap();
    let opt = Options::from_args(["--select", "--no-session", "--", &uri].map(OsString::from))
        .unwrap()
        .unwrap();
    assert!(opt.explicit_directory && opt.no_session);
    assert_eq!(opt.request.unwrap().paths, vec![file]);
    let opt = Options::from_args([dir.path().as_os_str().to_owned()])
        .unwrap()
        .unwrap();
    assert_eq!(opt.directory, dir.path());
    assert!(opt.explicit_directory);
    assert!(opt.request.is_none());
    assert!(Options::from_args(["--select".into()]).is_err());
    assert!(Options::from_args(["--select".into(), "--properties".into()]).is_err());
    assert!(Options::from_args(["smb://host/share".into()]).is_err());
}
#[test]
fn desktop_settings_are_reachable_by_mouse_at_minimum_size_without_attaching_services() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path().into(), Settings::default());
    assert!(app.desktop.runtime.is_none());
    app.action(basalgrim::input::Action::Settings);
    let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
    terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
    let rect = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t, Target::SettingsSection(SettingsSection::Desktop)).then_some(*r)
        })
        .unwrap();
    app.handle(basalgrim::input::Input::PointerDown {
        x: rect.x,
        y: rect.y,
        double: false,
        toggle: false,
    });
    app.handle(basalgrim::input::Input::PointerUp {
        x: rect.x,
        y: rect.y,
    });
    assert_eq!(app.settings_view.section, SettingsSection::Desktop);
    terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
    assert_eq!(app.settings_rows().len(), 3);
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
}
fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !predicate(app) {
        app.tick();
        assert!(std::time::Instant::now() < end, "{}", app.notice);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn external_reveal_opens_fresh_tabs_defers_modals_and_keeps_hidden_raw_selection_on_refresh() {
    let root = tempfile::tempdir().unwrap();
    let hidden = root.path().join(".hidden");
    fs::write(&hidden, "hidden").unwrap();
    let second = root.path().join("visible");
    fs::write(&second, "other").unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    wait(&mut app, |a| !a.loading());
    app.action(basalgrim::input::Action::NewFolder);
    let request = Request {
        kind: Kind::Items,
        paths: vec![hidden.clone(), second.clone()],
        startup_id: String::new(),
    };
    app.attach_desktop(
        basalgrim::desktop::Runtime::start(false, false, false, Some(request))
            .unwrap()
            .unwrap(),
    );
    for _ in 0..20 {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(app.panes[0].tabs.len(), 1);
    assert!(app.dialog.is_some());
    app.cancel_dialog();
    wait(&mut app, |a| a.tab().marked.len() == 2);
    assert_eq!(app.panes[0].tabs.len(), 2);
    assert!(app.tab().marked.contains(&hidden));
    assert!(!app.settings.hidden);
    app.refresh();
    wait(&mut app, |a| !a.loading());
    assert!(app.tab().marked.contains(&hidden));
    assert_eq!(app.panes[0].tabs[0].marked.len(), 0);
}
#[test]
fn external_properties_waits_for_exact_scan_and_cancels_after_user_navigation() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "data").unwrap();
    let make = || {
        basalgrim::desktop::Runtime::start(
            false,
            false,
            false,
            Some(Request {
                kind: Kind::Properties,
                paths: vec![file.clone()],
                startup_id: String::new(),
            }),
        )
        .unwrap()
        .unwrap()
    };
    let mut app = App::new(root.path().into(), Settings::default());
    wait(&mut app, |a| !a.loading());
    app.attach_desktop(make());
    wait(&mut app, |a| a.details_dialog());
    assert_eq!(app.detail_view.as_ref().unwrap().source.path, file);
    app.cancel_dialog();
    app.attach_desktop(make());
    wait(&mut app, |a| a.panes[0].tabs.len() == 3);
    app.navigate(root.path().into(), true);
    for _ in 0..30 {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!app.details_dialog());
    assert!(app.notice.contains("отменён"));
}
struct Bus {
    child: std::process::Child,
    address: String,
}
impl Bus {
    fn new() -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            child,
            address: address.trim().into(),
        }
    }
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
fn private_dbus_methods_forwarding_backpressure_and_existing_owner_are_independent_of_host() {
    use basalgrim::desktop::{BUS, Event, FILE_MANAGER, OBJECT, Runtime};
    let bus = Bus::new();
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    fs::write(&file, "kept").unwrap();
    let peer = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap();
    peer.request_name(FILE_MANAGER).unwrap();
    let runtime = Runtime::start_at(Some(bus.address.clone()), true, false, true, None)
        .unwrap()
        .unwrap();
    assert!(runtime.status.contains("занят"));
    let uris = vec![basalgrim::clipboard::files::uri(&file).unwrap()];
    peer.call_method(
        Some(BUS),
        OBJECT,
        Some(FILE_MANAGER),
        "ShowItems",
        &(uris.clone(), "test"),
    )
    .unwrap();
    let Event::Batch(Ok(batch)) = runtime
        .events
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
    else {
        panic!("no batch")
    };
    assert_eq!(batch.locations[0].selected, vec![file.clone()]);
    assert!(
        Runtime::start_at(
            Some(bus.address.clone()),
            true,
            true,
            false,
            Some(Request {
                kind: Kind::Properties,
                paths: vec![file],
                startup_id: String::new()
            })
        )
        .unwrap()
        .is_none()
    );
    let Event::Batch(Ok(batch)) = runtime
        .events
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
    else {
        panic!("no forwarded batch")
    };
    assert!(batch.locations[0].properties);
    assert!(
        peer.call_method(
            Some(BUS),
            OBJECT,
            Some(FILE_MANAGER),
            "ShowItems",
            &(vec!["https://remote/file"], "")
        )
        .is_err()
    );
    assert!(
        peer.call_method(Some(BUS), OBJECT, Some(BUS), "ClaimFileManager", &())
            .is_err()
    );
    peer.release_name(FILE_MANAGER).unwrap();
    peer.call_method(Some(BUS), OBJECT, Some(BUS), "ClaimFileManager", &())
        .unwrap();
    let proxy = zbus::blocking::fdo::DBusProxy::new(&peer).unwrap();
    assert!(
        proxy
            .name_has_owner(FILE_MANAGER.try_into().unwrap())
            .unwrap()
    );
    peer.call_method(Some(BUS), OBJECT, Some(BUS), "ReleaseFileManager", &())
        .unwrap();
    assert!(
        !proxy
            .name_has_owner(FILE_MANAGER.try_into().unwrap())
            .unwrap()
    );
    let mut rejected = false;
    for _ in 0..40 {
        if peer
            .call_method(
                Some(BUS),
                OBJECT,
                Some(FILE_MANAGER),
                "ShowItems",
                &(uris.clone(), ""),
            )
            .is_err()
        {
            rejected = true;
            break;
        }
    }
    assert!(rejected, "bounded queues must reject overflow");
}
#[test]
fn installation_result_remains_visible_while_external_navigation_is_held_behind_settings() {
    use basalgrim::desktop::{Config, InstallAction, Runtime};
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    wait(&mut app, |a| !a.loading());
    app.action(basalgrim::input::Action::Settings);
    app.attach_desktop(
        Runtime::start(
            false,
            false,
            false,
            Some(Request {
                kind: Kind::Folders,
                paths: vec![root.path().into()],
                startup_id: String::new(),
            }),
        )
        .unwrap()
        .unwrap(),
    );
    for _ in 0..20 {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    app.desktop.working = true;
    assert!(
        !app.command_items()
            .iter()
            .find(|i| i.action == basalgrim::input::Action::Quit)
            .unwrap()
            .enabled
    );
    app.action(basalgrim::input::Action::Quit);
    assert!(!app.quit);
    assert!(app.desktop.runtime.as_ref().unwrap().install(
        Config {
            data: root.path().join("data"),
            state: root.path().join("state"),
            executable: root.path().join("missing-binary")
        },
        InstallAction::Install
    ));
    wait(&mut app, |a| !a.desktop.working);
    assert!(app.desktop.status.contains("Не найден"));
    assert_eq!(app.panes[0].tabs.len(), 1);
    assert!(matches!(app.dialog, Some(basalgrim::app::Dialog::Settings)));
}

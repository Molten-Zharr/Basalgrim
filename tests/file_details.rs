use basalgrim::{
    app::{App, DetailControl as C, Target},
    details::{Source, inspect},
    fs::{self, Operation},
    input::{Action, Input},
    journal::Database,
    operations::Cancellation,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs as disk,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::Path,
    time::{Duration, Instant},
};
fn source(path: &Path) -> Source {
    Source {
        path: path.into(),
        archive: None,
        readonly: false,
    }
}
fn mode(path: &Path) -> u32 {
    disk::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}
fn plan(path: &Path, bits: u32) -> Operation {
    Operation::Permissions(
        inspect(&source(path), &Cancellation::default())
            .unwrap()
            .change(bits)
            .unwrap(),
    )
}
fn set(path: &Path, bits: u32) {
    disk::set_permissions(path, disk::Permissions::from_mode(bits)).unwrap();
}
#[test]
fn properties_preserve_raw_paths_links_and_zero_mode_without_reading_special_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .join(std::ffi::OsString::from_vec(b"raw-\xff.txt".to_vec()));
    disk::write(&path, b"hello").unwrap();
    set(&path, 0);
    let p = inspect(&source(&path), &Default::default()).unwrap();
    assert_eq!(p.size, 5);
    assert_eq!(p.mode & 0o777, 0);
    assert_eq!(p.source.path, path);
    let report = fs::operate(Operation::Permissions(p.change(0o640).unwrap()));
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(mode(&path), 0o640);
    let link = root.path().join("link");
    symlink(&path, &link).unwrap();
    let p = inspect(&source(&link), &Default::default()).unwrap();
    assert_eq!(p.link, Some(path.clone()));
    assert!(!p.editable());
    assert!(p.change(0o777).is_err());
    let fifo = root.path().join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(
        !inspect(&source(&fifo), &Default::default())
            .unwrap()
            .editable()
    );
}
#[test]
fn permissions_are_nonrecursive_preserve_special_bits_and_reject_changed_objects() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("folder");
    disk::create_dir(&dir).unwrap();
    let file = dir.join("file");
    disk::write(&file, "same").unwrap();
    set(&file, 0o640);
    set(&dir, 0o1750);
    let report = fs::operate(plan(&dir, 0o700));
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(mode(&dir), 0o1700);
    assert_eq!(mode(&file), 0o640);
    let old = plan(&file, 0o777);
    disk::rename(&file, dir.join("previous")).unwrap();
    disk::write(&file, "same").unwrap();
    set(&file, 0o640);
    let report = fs::operate(old);
    assert!(report.completed.is_empty());
    assert!(report.error.unwrap().contains("измен"));
    assert_eq!(mode(&file), 0o640);
    let old = plan(&file, 0o777);
    set(&file, 0o600);
    let report = fs::operate(old);
    assert!(report.completed.is_empty());
    assert_eq!(mode(&file), 0o600);
    let old = plan(&file, 0o777);
    disk::rename(&dir, root.path().join("parked")).unwrap();
    let victim = root.path().join("victim");
    disk::create_dir(&victim).unwrap();
    disk::write(victim.join("file"), "victim").unwrap();
    set(&victim.join("file"), 0o600);
    symlink(&victim, &dir).unwrap();
    assert!(fs::operate(old).completed.is_empty());
    assert_eq!(mode(&victim.join("file")), 0o600);
    assert_eq!(mode(&root.path().join("parked/file")), 0o600);
}
fn journal_run(db: &mut Database, op: Operation, cancel: &Cancellation) -> fs::Report {
    let mut recorder = db.begin(&op, 1).unwrap();
    fs::operate_with_journal(
        op,
        cancel,
        &mut |_| {},
        &mut |_| basalgrim::conflicts::Decision::CANCEL,
        &mut recorder,
    )
}
#[test]
fn permission_intents_results_restart_and_recording_failures_preserve_exact_completion() {
    for failure in ["none", "before", "after"] {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        disk::write(&file, "kept").unwrap();
        set(&file, 0o600);
        let dbpath = root.path().join("journal.sqlite");
        let mut db = Database::open(&dbpath).unwrap();
        let sql = rusqlite::Connection::open(&dbpath).unwrap();
        if failure != "none" {
            sql.execute_batch(if failure=="before" {"CREATE TRIGGER fault BEFORE INSERT ON steps BEGIN SELECT RAISE(ABORT,'fault'); END;"}else{"CREATE TRIGGER fault BEFORE UPDATE ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(ABORT,'fault'); END;"}).unwrap();
        }
        let report = journal_run(&mut db, plan(&file, 0o640), &Default::default());
        assert_eq!(report.completed.len(), usize::from(failure != "before"));
        assert_eq!(mode(&file), if failure == "before" { 0o600 } else { 0o640 });
        assert_eq!(disk::read_to_string(&file).unwrap(), "kept");
        assert_eq!(report.error.is_some(), failure != "none");
        if failure == "none" {
            let receipt = report.journal.unwrap();
            assert!(receipt.history.blocked.is_some());
            assert_eq!(
                receipt.items[0].inverse.as_ref().unwrap().action,
                "restore_permissions"
            );
            let step: String = sql
                .query_row("SELECT kind FROM steps", [], |r| r.get(0))
                .unwrap();
            assert_eq!(step, "chmod:0640");
        }
        drop(db);
        let mut db = Database::open(&dbpath).unwrap();
        assert_eq!(db.history().unwrap().len(), 1);
    }
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file");
    disk::write(&file, "safe").unwrap();
    set(&file, 0o600);
    let op = plan(&file, 0o777);
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        fs::operate_with_control(op, &cancel, &mut |_| {})
            .completed
            .is_empty()
    );
    assert_eq!(mode(&file), 0o600);
}
fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !predicate(app) {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App) -> String {
    let mut t = Terminal::new(TestBackend::new(34, 14)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn click(app: &mut App, predicate: impl Fn(&C) -> bool) {
    render(app);
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| match t {
            Target::Detail { control, .. } if predicate(control) => Some(*r),
            _ => None,
        })
        .unwrap();
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
#[test]
fn compact_mouse_properties_apply_cancel_and_stale_controls_share_the_operation_engine() {
    for grid in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("Проверка.txt");
        disk::write(&file, "data").unwrap();
        set(&file, 0o640);
        let mut app = App::new(
            root.path().into(),
            Settings {
                grid,
                preview: false,
                ..Default::default()
            },
        );
        wait(&mut app, |a| !a.loading());
        app.action(Action::Properties);
        wait(&mut app, |a| {
            a.detail_view.as_ref().is_some_and(|v| !v.loading)
        });
        assert!(render(&mut app).contains("Сведения"));
        click(&mut app, |c| matches!(c, C::Permissions));
        render(&mut app);
        let old = app.hits.clone();
        click(&mut app, |c| matches!(c, C::Toggle(0o020)));
        assert_eq!(mode(&file), 0o640);
        let changed = app.detail_view.as_ref().unwrap().mode;
        app.hits = old;
        let r = app
            .hits
            .iter()
            .find_map(|(r, t)| {
                matches!(
                    t,
                    Target::Detail {
                        control: C::Toggle(0o040),
                        ..
                    }
                )
                .then_some(*r)
            })
            .unwrap();
        app.handle(Input::Click {
            x: r.x,
            y: r.y,
            double: false,
            toggle: false,
        });
        assert_eq!(app.detail_view.as_ref().unwrap().mode, changed);
        app.cancel_dialog();
        assert_eq!(mode(&file), 0o640);
        app.action(Action::Properties);
        wait(&mut app, |a| {
            a.detail_view.as_ref().is_some_and(|v| !v.loading)
        });
        click(&mut app, |c| matches!(c, C::Permissions));
        click(&mut app, |c| matches!(c, C::Toggle(0o020)));
        click(&mut app, |c| matches!(c, C::Apply));
        wait(&mut app, |a| !a.loading());
        assert_eq!(mode(&file), 0o660);
        assert!(app.dialog.is_none());
        assert_eq!(app.operations[0].completed_paths, vec![file.clone()]);
        app.action(Action::Properties);
        app.tab_mut().request += 1;
        app.tick();
        assert!(app.detail_view.is_none());
    }
}

#[test]
#[ignore = "requires scripts/details_fixture.py in a separate process with private HOME, XDG and bus"]
fn installed_applications_launch_exact_paths_and_reject_stale_choices() {
    use basalgrim::details::{Event, Service};
    fn event(service: &Service) -> Event {
        let end = Instant::now() + Duration::from_secs(8);
        loop {
            let mut events = service.poll();
            if !events.is_empty() {
                return events.remove(0);
            }
            assert!(Instant::now() < end, "application worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn list(
        service: &mut Service,
        id: u64,
        source: Source,
        all: bool,
    ) -> Vec<basalgrim::details::Application> {
        service
            .applications(id, source, all, Default::default())
            .unwrap();
        match event(service) {
            Event::Applications(token, result) => {
                assert_eq!(token, id);
                result.unwrap().1
            }
            _ => panic!("wrong worker event"),
        }
    }
    fn launched(service: &Service, expected: Option<&Path>, success: bool) {
        match event(service) {
            Event::Launched(_, path, result) => {
                assert_eq!(result.is_ok(), success, "{result:?}");
                if success {
                    assert_eq!(path.as_deref(), expected);
                }
            }
            _ => panic!("wrong worker event"),
        }
    }
    let root = std::path::PathBuf::from(
        std::env::var_os("BASALGRIM_DETAILS_FIXTURE").expect("explicit private fixture required"),
    );
    assert_eq!(std::env::var_os("HOME").unwrap(), root.as_os_str());
    let file = root
        .join("details-files")
        .join(std::ffi::OsString::from_vec(
            b"raw ' $(ignored) \xff.txt".to_vec(),
        ));
    disk::write(&file, "exact contents").unwrap();
    let mut service = Service::default();
    let apps = list(&mut service, 1, source(&file), false);
    assert_eq!(
        apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["Fixture Alpha", "Fixture Beta"]
    );
    assert!(apps[0].default);
    service.launch(1, 0, Default::default()).unwrap();
    launched(&service, Some(&file), true);
    let log = root.join("opened.json");
    let end = Instant::now() + Duration::from_secs(8);
    while !log.exists() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    let json: serde_json::Value = serde_json::from_slice(&disk::read(&log).unwrap()).unwrap();
    let hex = file
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(json["path"], hex);
    assert_eq!(json["app"], "Alpha");
    disk::remove_file(&log).unwrap();
    assert_eq!(list(&mut service, 2, source(&file), true).len(), 3);
    let desktop = root.join("data/applications/fixture-Alpha.desktop");
    let old = disk::read(&desktop).unwrap();
    disk::write(&desktop, [old.as_slice(), b"\n# changed\n"].concat()).unwrap();
    service.launch(2, 0, Default::default()).unwrap();
    launched(&service, None, false);
    disk::write(&desktop, old).unwrap();
    list(&mut service, 3, source(&file), false);
    disk::rename(&file, root.join("old.txt")).unwrap();
    disk::write(&file, "replacement").unwrap();
    service.launch(3, 0, Default::default()).unwrap();
    launched(&service, None, false);
    list(&mut service, 4, source(&file), false);
    let cancel = Cancellation::default();
    cancel.cancel();
    service.launch(4, 0, cancel).unwrap();
    launched(&service, None, false);
    assert!(!log.exists());
    let archive = root.join("files.tar");
    let report = fs::operate(Operation::ArchiveCreate {
        sources: vec![file.clone()],
        destination: archive.clone(),
        format: basalgrim::archive::Format::Tar,
    });
    assert!(report.error.is_none(), "{report:?}");
    let catalog = basalgrim::archive::Catalog::read(&archive, &Default::default()).unwrap();
    let archive_source = Source {
        path: archive.clone(),
        archive: Some(catalog.clone()),
        readonly: true,
    };
    let props = inspect(&archive_source, &Default::default()).unwrap();
    assert!(!props.editable());
    assert_eq!(props.mime, "inode/directory");
    service
        .applications(5, archive_source, true, Default::default())
        .unwrap();
    assert!(matches!(event(&service), Event::Applications(5, Err(_))));
    let member = Source {
        path: archive.join(file.file_name().unwrap()),
        archive: Some(catalog),
        readonly: true,
    };
    list(&mut service, 6, member, false);
    service.launch(6, 0, Default::default()).unwrap();
    launched(&service, None, true);
    let end = Instant::now() + Duration::from_secs(8);
    while !log.exists() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    let json: serde_json::Value = serde_json::from_slice(&disk::read(&log).unwrap()).unwrap();
    assert_eq!(json["data"], "7265706c6163656d656e74");
    assert_ne!(json["path"], hex);

    // Exercise renderer-owned rows too: neither a single click nor obsolete
    // geometry after filtering/reopening may start an application.
    disk::remove_file(&log).unwrap();
    let mut app = App::new(file.parent().unwrap().into(), Settings::default());
    wait(&mut app, |a| !a.loading());
    app.tab_mut().selected = app
        .tab()
        .entries
        .iter()
        .position(|e| e.path == file)
        .unwrap();
    app.action(Action::OpenWith);
    wait(&mut app, |a| {
        a.detail_view.as_ref().is_some_and(|v| !v.loading)
    });
    render(&mut app);
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(
                t,
                Target::Detail {
                    control: C::Row(1),
                    ..
                }
            )
            .then_some(*r)
        })
        .unwrap();
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
    assert_eq!(app.detail_view.as_ref().unwrap().selected, 1);
    assert!(!log.exists());
    render(&mut app);
    let old = app.hits.clone();
    app.handle(Input::Text("Alpha".into()));
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: true,
        toggle: false,
    });
    assert!(app.details_dialog());
    assert_eq!(app.detail_view.as_ref().unwrap().visible.len(), 1);
    assert!(!log.exists());
    app.cancel_dialog();
    app.action(Action::OpenWith);
    wait(&mut app, |a| {
        a.detail_view.as_ref().is_some_and(|v| !v.loading)
    });
    app.hits = old;
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: true,
        toggle: false,
    });
    assert!(app.details_dialog());
    assert!(!log.exists());
    render(&mut app);
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: true,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
    assert!(app.dialog.is_none());
    wait(&mut app, |_| log.exists());
    let json: serde_json::Value = serde_json::from_slice(&disk::read(&log).unwrap()).unwrap();
    assert_eq!(json["app"], "Beta");
    assert_eq!(json["path"], hex);
}

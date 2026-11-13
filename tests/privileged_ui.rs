//! Ordinary operations select authorization automatically; no separate administrator UI.
use basalgrim::{
    conflicts::Decision,
    fs::{Operation, Report},
    journal::{Database, Identity},
    operations::Cancellation,
    privileged::{Kind, execute_with_launcher, prepare},
};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};
fn mode(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}
fn create(p: &Path, name: &str) -> Operation {
    Operation::Create {
        parent: p.into(),
        name: name.into(),
        directory: false,
    }
}
fn run(op: Operation, db: &mut Database, calls: &mut Vec<Kind>) -> Report {
    execute_with_launcher(
        op,
        db,
        1,
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| Decision::CANCEL,
        &mut |plan, _, _, _| {
            calls.push(plan.kind);
            Report {
                total: plan.total(),
                cancelled: true,
                journal: Some(Default::default()),
                ..Default::default()
            }
        },
    )
}
#[test]
fn ordinary_create_requests_authorization_only_when_needed_without_mutation_or_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mode(root, 0o700);
    let folder = root.join("folder");
    fs::create_dir(&folder).unwrap();
    mode(&folder, 0o700);
    let mut db = Database::open(&root.join("user.sqlite3")).unwrap();
    let mut calls = vec![];
    let report = run(create(&folder, "Обычный.txt"), &mut db, &mut calls);
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert!(calls.is_empty());
    mode(&folder, 0o500);
    let report = run(create(&folder, "Защищённый.txt"), &mut db, &mut calls);
    mode(&folder, 0o700);
    assert!(report.cancelled);
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(calls, vec![Kind::File]);
    assert!(!folder.join("Защищённый.txt").exists());
    assert!(folder.join("Обычный.txt").exists());
    assert!(
        report
            .journal
            .unwrap()
            .items
            .iter()
            .all(|i| i.state != "uncertain")
    );
    let report = run(create(&folder, "Обычный.txt"), &mut db, &mut calls);
    assert!(report.error.is_some());
    assert_eq!(calls.len(), 1, "Name conflicts must not request root");
}
#[test]
fn all_supported_operations_share_permission_checks_and_keep_exact_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mode(root, 0o700);
    let source = root.join("sources");
    let dest = root.join("dest");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    mode(&source, 0o700);
    mode(&dest, 0o700);
    let a = source.join("a");
    let b = source.join("b");
    fs::write(&a, b"one").unwrap();
    fs::write(&b, b"two").unwrap();
    let cancel = Cancellation::default();
    let copy = Operation::Transfer {
        sources: vec![a.clone(), b.clone()],
        destination: dest.clone(),
        cut: false,
    };
    mode(&a, 0o400);
    assert!(
        prepare(&copy, &cancel).unwrap().is_none(),
        "read-only source is copyable"
    );
    mode(&b, 0o000);
    let plan = prepare(&copy, &cancel).unwrap().unwrap();
    assert_eq!(plan.sources.len(), 2);
    assert_eq!(plan.kind, Kind::Copy);
    assert!(!dest.join("a").exists());
    let mut db = Database::open(&root.join("user.sqlite3")).unwrap();
    let mut calls = vec![];
    let report = run(copy, &mut db, &mut calls);
    assert!(report.cancelled);
    assert!(
        !dest.join("a").exists(),
        "authorization must precede first item"
    );
    let movement = Operation::Transfer {
        sources: vec![a.clone(), b.clone()],
        destination: dest.clone(),
        cut: true,
    };
    assert!(
        prepare(&movement, &cancel).unwrap().is_none(),
        "same-filesystem move doesn't require reading file data"
    );
    mode(&source, 0o500);
    for op in [
        movement,
        Operation::Rename {
            source: a.clone(),
            name: "new".into(),
        },
        Operation::Duplicate {
            sources: vec![a.clone()],
            destination: source.clone(),
        },
    ] {
        assert!(prepare(&op, &cancel).unwrap().is_some());
    }
    mode(&source, 0o700);
    mode(&b, 0o600);
    let m = fs::symlink_metadata(&source).unwrap();
    let permission = Operation::Permissions(basalgrim::details::PermissionChange {
        path: a.clone(),
        before: Identity::read(&a).unwrap().unwrap(),
        parent: (m.dev(), m.ino()),
        mode: 0o600,
    });
    assert!(
        prepare(&permission, &cancel).unwrap().is_none(),
        "owner can chmod a readonly file"
    );
    // World-readable root-owned metadata is enough to detect a protected chmod;
    // no helper is launched and the host file is never changed.
    let host = Path::new("/etc/passwd");
    let parent = fs::metadata("/etc").unwrap();
    let permission = Operation::Permissions(basalgrim::details::PermissionChange {
        path: host.into(),
        before: Identity::read(host).unwrap().unwrap(),
        parent: (parent.dev(), parent.ino()),
        mode: 0o600,
    });
    assert!(prepare(&permission, &cancel).unwrap().is_some());
    let Operation::Permissions(mut changed_parent) = permission else {
        unreachable!()
    };
    changed_parent.parent.1 = changed_parent.parent.1.wrapping_add(1);
    assert!(prepare(&Operation::Permissions(changed_parent), &cancel).is_err());
}
#[test]
fn nested_unreadable_sources_and_protected_destination_components_are_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    mode(root, 0o700);
    let source = root.join("source");
    let dest = root.join("protected/dest");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(&dest).unwrap();
    mode(&source, 0o700);
    mode(&source.join("nested"), 0o700);
    mode(dest.parent().unwrap(), 0o700);
    mode(&dest, 0o700);
    let leaf = source.join("nested/leaf");
    fs::write(&leaf, b"contents").unwrap();
    mode(&leaf, 0o000);
    let op = Operation::Transfer {
        sources: vec![source.clone()],
        destination: dest.clone(),
        cut: false,
    };
    assert!(prepare(&op, &Cancellation::default()).unwrap().is_some());
    mode(&leaf, 0o600);
    symlink("/nonexistent", source.join("link")).unwrap();
    assert!(
        prepare(&op, &Cancellation::default()).unwrap().is_none(),
        "dangling links are copied, not followed"
    );
    mode(dest.parent().unwrap(), 0o000);
    let result = prepare(&op, &Cancellation::default());
    mode(dest.parent().unwrap(), 0o700);
    assert!(
        result.unwrap().is_some(),
        "protected destination must reach authorization"
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(prepare(&op, &cancelled).is_err());
}
#[test]
fn partial_failure_is_never_replayed_with_elevation() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    mode(r, 0o700);
    let a = r.join("a");
    let b = r.join("b");
    let dest = r.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&a, b"first").unwrap();
    fs::write(&b, b"second").unwrap();
    let mut db = Database::open(&r.join("user.sqlite3")).unwrap();
    let mut calls = 0;
    let report = execute_with_launcher(
        Operation::Transfer {
            sources: vec![a, b.clone()],
            destination: dest.clone(),
            cut: false,
        },
        &mut db,
        1,
        &Cancellation::default(),
        &mut |p| {
            if p.items_done == 1 {
                mode(&b, 0o000);
            }
        },
        &mut |_| Decision::CANCEL,
        &mut |_, _, _, _| {
            calls += 1;
            panic!("must never replay a started batch")
        },
    );
    mode(&b, 0o600);
    assert_eq!(calls, 0);
    assert_eq!(report.completed.len(), 1);
    assert!(report.error.is_some());
    assert_eq!(fs::read(dest.join("a")).unwrap(), b"first");
    assert!(!dest.join("b").exists());
}
#[test]
fn removed_binding_migrates_without_resetting_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("settings.json");
    let mut value = serde_json::to_value(basalgrim::settings::Settings::default()).unwrap();
    let bindings = value["bindings"].as_array_mut().unwrap();
    let mut old = bindings[0].clone();
    old["action"] = "Privileged".into();
    bindings.push(old);
    value["font_size"] = 22.into();
    fs::write(&p, serde_json::to_vec(&value).unwrap()).unwrap();
    let settings = basalgrim::settings::Settings::load(&p).unwrap();
    assert_eq!(settings.appearance.font_size, 22);
    assert!(
        !serde_json::to_string(&settings)
            .unwrap()
            .contains("Privileged")
    );
}

#[test]
fn protected_trash_routes_symlinks_without_reading_or_following_their_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    mode(r, 0o700);
    let folder = r.join("systemd");
    fs::create_dir(&folder).unwrap();
    let target = r.join("target");
    fs::write(&target, b"keep").unwrap();
    let link = folder.join("syslog.service");
    symlink(&target, &link).unwrap();
    let operation = Operation::Trash {
        sources: vec![link.clone()],
    };
    assert!(
        prepare(&operation, &Cancellation::default())
            .unwrap()
            .is_none()
    );
    mode(&folder, 0o500);
    let prepared = prepare(&operation, &Cancellation::default())
        .unwrap()
        .unwrap();
    assert_eq!(prepared.kind, Kind::Trash);
    assert_eq!(prepared.sources, vec![basalgrim::journal::bytes(&link)]);
    let mut db = Database::open(&r.join("user.sqlite3")).unwrap();
    let mut calls = vec![];
    let report = run(operation, &mut db, &mut calls);
    mode(&folder, 0o700);
    assert!(report.cancelled);
    assert_eq!(calls, vec![Kind::Trash]);
    assert_eq!(fs::read_link(link).unwrap(), target);
    assert_eq!(fs::read(target).unwrap(), b"keep");
}

#[test]
fn compact_error_notice_starts_with_cause_and_has_mouse_access_to_history() {
    use basalgrim::{
        app::{App, Target},
        input::Action,
        settings::Settings,
        ui,
    };
    use ratatui::{Terminal, backend::TestBackend};
    let t = tempfile::tempdir().unwrap();
    let mut app = App::new(t.path().into(), Settings::default());
    let error = "Корзина: gio: file:///etc/systemd/syslog.service: Отказано в доступе";
    app.notice = basalgrim::operations::error_summary(error);
    app.notice_error = true;
    app.operations
        .push_front(basalgrim::operations::OperationRecord::new(
            1,
            "Корзина".into(),
            1,
        ));
    let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("Отказано в доступе"), "{text}");
    assert!(
        app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::Action(Action::Operations)))
    );
}

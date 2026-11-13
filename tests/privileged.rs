use basalgrim::{
    conflicts::{Choice, Decision},
    fs::Operation,
    journal::{Database, bytes},
    operations::Cancellation,
    privileged::{Kind, Request, run_with_command},
};
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::Path,
    process::Command,
};

fn request(root: &Path, kind: Kind, sources: &[&Path], text: &str) -> Request {
    Request {
        kind,
        directory: bytes(root),
        sources: sources.iter().map(|p| bytes(p)).collect(),
        text: text.into(),
        destination_directory: None,
        trash: None,
    }
}
fn run(root: &Path, request: Request, choice: Choice) -> basalgrim::fs::Report {
    let exe =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/examples/privileged_fixture");
    assert!(
        exe.is_file(),
        "build --all-features --example privileged_fixture first"
    );
    let mut cmd = Command::new(exe);
    cmd.arg(root.join("helper.sqlite3"));
    let mut db = Database::open(&root.join("user.sqlite3")).unwrap();
    let mut recorder = db
        .begin(
            &Operation::Privileged(basalgrim::privileged::Plan::capture(request.clone()).unwrap()),
            1,
        )
        .unwrap();
    recorder.delegated_intent().unwrap();
    let mut report = run_with_command(
        request,
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice,
            remaining: false,
        },
        cmd,
    );
    recorder.finish_delegated(&mut report);
    report
}
#[test]
fn unprivileged_helper_copy_move_conflicts_raw_paths_and_both_journals() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let source = r.join(std::ffi::OsString::from_vec(b"source-\xff.txt".to_vec()));
    fs::write(&source, b"exact bytes").unwrap();
    let destination = r.join("dest");
    fs::create_dir(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
    for choice in [
        Choice::KeepBoth,
        Choice::KeepBoth,
        Choice::Skip,
        Choice::Replace,
    ] {
        let report = run(
            r,
            request(r, Kind::Copy, &[&source], destination.to_str().unwrap()),
            choice,
        );
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(report.completed.len(), usize::from(choice != Choice::Skip));
        assert_eq!(report.skipped.len(), usize::from(choice == Choice::Skip));
        assert_eq!(fs::read(&source).unwrap(), b"exact bytes");
        assert_eq!(
            fs::read(destination.join(source.file_name().unwrap())).unwrap(),
            b"exact bytes"
        );
        assert_eq!(report.journal.as_ref().unwrap().history.undo, 0);
        if choice == Choice::Replace {
            assert!(
                !report.journal.as_ref().unwrap().items[0]
                    .recovery_paths
                    .is_empty()
            );
        }
    }
    let report = run(
        r,
        request(r, Kind::Move, &[&source], destination.to_str().unwrap()),
        Choice::KeepBoth,
    );
    assert!(report.error.is_none(), "{report:?}");
    assert!(!source.exists());
    assert_eq!(report.completed, vec![source]);
    for name in ["helper.sqlite3", "user.sqlite3"] {
        let mut db = Database::open(&r.join(name)).unwrap();
        let history = db.history().unwrap();
        assert_eq!(history.len(), 5);
        assert!(history.iter().all(|h| h.history.undo == 0));
        assert!(history.iter().any(|h| h.completed == 1));
    }
}

#[test]
fn raw_destination_and_authentication_dismissal_are_not_retargeted_or_uncertain() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let source = r.join("source");
    fs::write(&source, b"raw destination").unwrap();
    let dest = r.join(std::ffi::OsString::from_vec(b"dest-\xff".to_vec()));
    fs::create_dir(&dest).unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o700)).unwrap();
    let mut req = request(r, Kind::Copy, &[&source], &dest.to_string_lossy());
    req.destination_directory = Some(bytes(&dest));
    let report = run(r, req, Choice::KeepBoth);
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(fs::read(dest.join("source")).unwrap(), b"raw destination");
    let req = request(r, Kind::File, &[], "must-not-exist");
    let mut db = Database::open(&r.join("cancel.sqlite3")).unwrap();
    let mut recorder = db
        .begin(
            &Operation::Privileged(basalgrim::privileged::Plan::capture(req.clone()).unwrap()),
            1,
        )
        .unwrap();
    recorder.delegated_intent().unwrap();
    let mut cmd = Command::new("/usr/bin/python3");
    cmd.args(["-c", "raise SystemExit(126)"]);
    let mut report = run_with_command(
        req,
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| Decision::CANCEL,
        cmd,
    );
    recorder.finish_delegated(&mut report);
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert_eq!(report.journal.as_ref().unwrap().items[0].state, "failed");
    assert!(!r.join("must-not-exist").exists());
}
#[test]
fn create_rename_permissions_and_no_implicit_replacement() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let a = run(r, request(r, Kind::File, &[], "new.txt"), Choice::KeepBoth);
    assert!(a.error.is_none(), "{a:?}");
    let a = run(r, request(r, Kind::File, &[], "new.txt"), Choice::KeepBoth);
    assert!(a.error.is_some());
    let source = r.join("new.txt");
    fs::write(&source, b"preserved").unwrap();
    let a = run(
        r,
        request(r, Kind::Permissions, &[&source], "640"),
        Choice::KeepBoth,
    );
    assert!(a.error.is_none(), "{a:?}");
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let a = run(
        r,
        request(r, Kind::Rename, &[&source], "новое имя.txt"),
        Choice::KeepBoth,
    );
    assert!(a.error.is_none(), "{a:?}");
    assert_eq!(a.renamed, Some((source.clone(), r.join("новое имя.txt"))));
    assert!(!source.exists());
    let a = run(r, request(r, Kind::Folder, &[], "folder"), Choice::KeepBoth);
    assert!(a.error.is_none(), "{a:?}");
    assert!(r.join("folder").is_dir());
}
#[test]
fn rejects_unsafe_inputs_and_cancels_before_spawning() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let source = r.join("source");
    fs::write(&source, b"unchanged").unwrap();
    let link = r.join("link");
    symlink(r, &link).unwrap();
    assert!(
        basalgrim::privileged::Plan::capture(request(
            &link,
            Kind::Copy,
            &[&link.join("source")],
            r.to_str().unwrap()
        ))
        .is_err()
    );
    assert!(
        request(r, Kind::Permissions, &[&source], "7777")
            .validate()
            .is_err()
    );
    assert!(request(r, Kind::File, &[], "../bad").validate().is_err());
    let c = Cancellation::default();
    c.cancel();
    let report = run_with_command(
        request(r, Kind::File, &[], "never"),
        &c,
        &mut |_| {},
        &mut |_| Decision::CANCEL,
        Command::new("/must/not/execute"),
    );
    assert!(report.cancelled);
    assert!(!r.join("never").exists());
    fs::set_permissions(r, fs::Permissions::from_mode(0o777)).unwrap();
    let report = run(r, request(r, Kind::File, &[], "unsafe"), Choice::KeepBoth);
    assert!(report.error.is_some(), "{report:?}");
    assert!(!r.join("unsafe").exists());
}

#[test]
fn cancellation_preserves_completed_items_and_removes_unpublished_copy() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let a = r.join("first");
    let b = r.join("large");
    let dest = r.join("dest");
    fs::write(&a, b"done").unwrap();
    fs::write(&b, vec![1; 8 * 1024 * 1024]).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o700)).unwrap();
    let c = Cancellation::default();
    let mut cmd = Command::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/examples/privileged_fixture"),
    );
    cmd.arg(r.join("helper.sqlite3"));
    let report = run_with_command(
        request(r, Kind::Copy, &[&a, &b], dest.to_str().unwrap()),
        &c,
        &mut |p| {
            if p.items_done == 1 {
                c.cancel()
            }
        },
        &mut |_| Decision::CANCEL,
        cmd,
    );
    assert!(report.cancelled, "{report:?}");
    assert_eq!(report.completed, vec![a.clone()]);
    assert_eq!(fs::read(dest.join("first")).unwrap(), b"done");
    assert!(!dest.join("large").exists());
    assert_eq!(fs::read_dir(&dest).unwrap().count(), 1);
    assert!(a.exists() && b.exists());
}

#[test]
fn user_journal_failure_after_mutation_preserves_exact_result() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let dbpath = r.join("user.sqlite3");
    drop(Database::open(&dbpath).unwrap());
    let connection = rusqlite::Connection::open(&dbpath).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_result BEFORE UPDATE OF inverse ON items BEGIN SELECT RAISE(FAIL,'fixture recording failure'); END;").unwrap();
    drop(connection);
    let report = run(
        r,
        request(r, Kind::File, &[], "published"),
        Choice::KeepBoth,
    );
    assert!(
        report
            .error
            .as_deref()
            .unwrap()
            .contains("fixture recording failure")
    );
    assert_eq!(report.completed, vec![r.join("published")]);
    assert!(r.join("published").is_file());
    let mut db = Database::open(&dbpath).unwrap();
    assert_eq!(
        db.history().unwrap()[0]
            .summary
            .as_ref()
            .unwrap()
            .progress
            .items_done,
        1
    );
}

#[test]
fn killed_helper_retains_confirmed_completion_and_does_not_replay() {
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let a = r.join("first");
    let b = r.join("large");
    let dest = r.join("dest");
    fs::write(&a, b"published").unwrap();
    fs::write(&b, vec![2; 8 * 1024 * 1024]).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::set_permissions(&dest, fs::Permissions::from_mode(0o700)).unwrap();
    let pidfile = r.join("pid");
    let mut cmd = Command::new("/usr/bin/python3");
    cmd.args(["-c","import os,sys;open(sys.argv[1],'w').write(str(os.getpid()));os.execv(sys.argv[2],sys.argv[2:])"]).arg(&pidfile).arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/examples/privileged_fixture")).arg(r.join("helper.sqlite3"));
    let mut process = None;
    let mut killed = false;
    let report = run_with_command(
        request(r, Kind::Copy, &[&a, &b], dest.to_str().unwrap()),
        &Cancellation::default(),
        &mut |p| {
            if process.is_none() {
                let id = fs::read_to_string(&pidfile).unwrap().parse().unwrap();
                process =
                    Some(pidfd_open(Pid::from_raw(id).unwrap(), PidfdFlags::empty()).unwrap());
            }
            if p.items_done == 1 && !killed {
                pidfd_send_signal(process.as_ref().unwrap(), Signal::KILL).unwrap();
                killed = true;
            }
        },
        &mut |_| Decision::CANCEL,
        cmd,
    );
    assert!(killed);
    assert!(
        report
            .error
            .as_deref()
            .unwrap()
            .contains("требует проверки")
    );
    assert_eq!(report.completed, vec![a]);
    assert_eq!(fs::read(dest.join("first")).unwrap(), b"published");
    let before = fs::read_dir(&dest).unwrap().count();
    let mut db = Database::open(&r.join("helper.sqlite3")).unwrap();
    let history = db.history().unwrap();
    assert_eq!(history[0].state, "interrupted");
    assert_eq!(history[0].history.undo, 0);
    assert_eq!(fs::read_dir(&dest).unwrap().count(), before);
}

#[test]
fn recursive_copy_preserves_links_and_rejects_untrusted_subdirectories() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let tree = r.join("tree");
    let sub = tree.join("nested");
    let dest = r.join("dest");
    let second = r.join("second");
    for dir in [&tree, &sub, &dest, &second] {
        fs::create_dir(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(sub.join("кириллица.txt"), b"nested data").unwrap();
    symlink(r, sub.join("outside")).unwrap();
    let report = run(
        r,
        request(r, Kind::Copy, &[&tree], dest.to_str().unwrap()),
        Choice::KeepBoth,
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(
        fs::read(dest.join("tree/nested/кириллица.txt")).unwrap(),
        b"nested data"
    );
    assert_eq!(fs::read_link(dest.join("tree/nested/outside")).unwrap(), r);
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o777)).unwrap();
    let report = run(
        r,
        request(r, Kind::Copy, &[&tree], second.to_str().unwrap()),
        Choice::KeepBoth,
    );
    assert!(report.error.is_some());
    assert!(report.completed.is_empty());
    assert_eq!(fs::read_dir(second).unwrap().count(), 0);
}

#[test]
fn helper_trash_uses_caller_catalog_and_preserves_symlink_target_and_raw_original() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let source = r.join("source");
    fs::create_dir(&source).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    let target = r.join("untouched");
    fs::write(&target, b"target bytes").unwrap();
    let link = source.join(std::ffi::OsString::from_vec(
        b"syslog-\xff.service".to_vec(),
    ));
    symlink(&target, &link).unwrap();
    let file = source.join("file.txt");
    fs::write(&file, b"contents").unwrap();
    let data = r.join("data");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let mut req = request(r, Kind::Trash, &[&link, &file], "");
    req.trash = Some(basalgrim::privileged::UserTrash {
        home: bytes(r),
        data: bytes(&data),
    });
    let report = run(r, req, Choice::KeepBoth);
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 2);
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(!file.exists());
    assert_eq!(fs::read(&target).unwrap(), b"target bytes");
    let catalog =
        basalgrim::trash::read(&[basalgrim::trash::Root::home(data)], &Default::default()).unwrap();
    assert_eq!(catalog.entries.len(), 2, "{catalog:?}");
    let entry = catalog
        .entries
        .iter()
        .find(|e| e.original.as_ref() == Some(&link))
        .unwrap();
    assert!(entry.symlink);
    assert_eq!(fs::read_link(&entry.data).unwrap(), target);
    assert_eq!(
        fs::read(
            &catalog
                .entries
                .iter()
                .find(|e| e.original.as_ref() == Some(&file))
                .unwrap()
                .data
        )
        .unwrap(),
        b"contents"
    );
    assert!(
        report
            .journal
            .as_ref()
            .unwrap()
            .items
            .iter()
            .all(|i| i.target.is_some())
    );
}

#[test]
fn helper_trash_cancellation_keeps_remaining_sources_and_bad_catalog_never_deletes() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    fs::set_permissions(r, fs::Permissions::from_mode(0o700)).unwrap();
    let data = r.join("data");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let first = r.join("first");
    let second = r.join("second");
    fs::write(&first, b"one").unwrap();
    fs::write(&second, b"two").unwrap();
    let mut req = request(r, Kind::Trash, &[&first, &second], "");
    req.trash = Some(basalgrim::privileged::UserTrash {
        home: bytes(r),
        data: bytes(&data),
    });
    let cancel = Cancellation::default();
    let notify_cancel = cancel.clone();
    let mut cmd = Command::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/examples/privileged_fixture"),
    );
    cmd.arg(r.join("helper.sqlite3"));
    let report = run_with_command(
        req,
        &cancel,
        &mut |p| {
            if p.items_done == 1 {
                notify_cancel.cancel();
            }
        },
        &mut |_| Decision::CANCEL,
        cmd,
    );
    // Cancellation is observed between atomic items; a raced second success is
    // also counted exactly. Never report an entry as completed while it remains.
    for p in [&first, &second] {
        assert_eq!(!p.exists(), report.completed.contains(p));
    }
    assert!(report.completed.contains(&first));
    let blocked = r.join("blocked");
    fs::write(&blocked, b"must remain").unwrap();
    let bad = r.join("bad-data");
    fs::create_dir(&bad).unwrap();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&data, bad.join("Trash")).unwrap();
    let mut req = request(r, Kind::Trash, &[&blocked], "");
    req.trash = Some(basalgrim::privileged::UserTrash {
        home: bytes(r),
        data: bytes(&bad),
    });
    assert!(basalgrim::privileged::Plan::capture(req).is_err());
    assert_eq!(fs::read(blocked).unwrap(), b"must remain");
}

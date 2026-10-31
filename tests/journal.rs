use basalgrim::{
    conflicts::{Choice, Decision},
    fs::{self, Operation},
    journal::{Database, Identity, path},
    operations::Cancellation,
};
use std::fs as disk;

fn run(database: &mut Database, operation: Operation, choice: Choice) -> fs::Report {
    let mut recorder = database.begin(&operation, 7).unwrap();
    fs::operate_with_journal(
        operation,
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice,
            remaining: true,
        },
        &mut recorder,
    )
}

#[test]
fn durable_history_records_actual_keep_both_paths_ids_and_inverse_plans() {
    let root = tempfile::tempdir().unwrap();
    let dbpath = root.path().join("operations.sqlite3");
    let source = root.path().join("source");
    let dest = root.path().join("dest");
    disk::create_dir(&source).unwrap();
    disk::create_dir(&dest).unwrap();
    disk::write(source.join("a.txt"), "incoming").unwrap();
    disk::write(dest.join("a.txt"), "original").unwrap();
    let mut db = Database::open(&dbpath).unwrap();
    let report = run(
        &mut db,
        Operation::Transfer {
            sources: vec![source.join("a.txt")],
            destination: dest.clone(),
            cut: false,
        },
        Choice::KeepBoth,
    );
    assert!(report.error.is_none(), "{report:?}");
    let receipt = report.journal.unwrap();
    assert!(receipt.id > 0);
    assert_eq!(receipt.items.len(), 1);
    let item = &receipt.items[0];
    assert_eq!(path(item.target.as_ref().unwrap()), dest.join("a (1).txt"));
    assert_eq!(item.before, Identity::read(&source.join("a.txt")).unwrap());
    assert_eq!(item.after, Identity::read(&dest.join("a (1).txt")).unwrap());
    assert_eq!(item.inverse.as_ref().unwrap().action, "remove_created");
    assert!(item.inverse.as_ref().unwrap().blocked.is_none());
    drop(db);
    let mut reopened = Database::open(&dbpath).unwrap();
    let history = reopened.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].state, "done");
    assert_eq!(
        disk::read_to_string(dest.join("a.txt")).unwrap(),
        "original"
    );
}

#[test]
fn create_move_rename_skip_and_replacement_have_distinct_inverse_records() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("operations.sqlite3")).unwrap();
    let create = run(
        &mut db,
        Operation::Create {
            parent: root.path().into(),
            name: "new".into(),
            directory: false,
        },
        Choice::KeepBoth,
    );
    assert!(create.error.is_none(), "{create:?}");
    assert_eq!(
        create.journal.unwrap().items[0]
            .inverse
            .as_ref()
            .unwrap()
            .action,
        "remove_created"
    );
    let rename = run(
        &mut db,
        Operation::Rename {
            source: root.path().join("new"),
            name: "renamed".into(),
        },
        Choice::KeepBoth,
    );
    assert!(rename.error.is_none(), "{rename:?}");
    assert_eq!(
        rename.journal.unwrap().items[0]
            .inverse
            .as_ref()
            .unwrap()
            .action,
        "move_back"
    );
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    disk::write(dest.join("renamed"), "old").unwrap();
    let skipped = run(
        &mut db,
        Operation::Transfer {
            sources: vec![root.path().join("renamed")],
            destination: dest.clone(),
            cut: true,
        },
        Choice::Skip,
    );
    assert!(skipped.error.is_none(), "{skipped:?}");
    assert_eq!(skipped.journal.unwrap().items[0].state, "skipped");
    let replaced = run(
        &mut db,
        Operation::Transfer {
            sources: vec![root.path().join("renamed")],
            destination: dest,
            cut: true,
        },
        Choice::Replace,
    );
    assert!(replaced.error.is_none(), "{replaced:?}");
    assert!(
        replaced.journal.unwrap().items[0]
            .inverse
            .as_ref()
            .unwrap()
            .blocked
            .is_some()
    );
}

#[test]
fn live_instances_are_not_interrupted_and_abandoned_work_is_recovered_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let dbpath = root.path().join("operations.sqlite3");
    let target = root.path().join("unconfirmed");
    let mut first = Database::open(&dbpath).unwrap();
    let operation = Operation::Create {
        parent: root.path().into(),
        name: "unconfirmed".into(),
        directory: false,
    };
    {
        let mut recorder = first.begin(&operation, 1).unwrap();
        recorder.begin_item(0).unwrap();
        recorder
            .before_step("create", None, Some(&target), false)
            .unwrap();
    }
    let mut second = Database::open(&dbpath).unwrap();
    assert_eq!(second.history().unwrap()[0].state, "running");
    assert!(!target.exists());
    drop(first);
    let history = second.history().unwrap();
    assert_eq!(history[0].state, "interrupted");
    assert_eq!(history[0].items[0].state, "uncertain");
    assert!(!target.exists());
}

#[test]
fn foreign_future_corrupt_and_symlink_databases_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("bad");
    disk::write(&bad, "not sqlite").unwrap();
    assert!(Database::open(&bad).is_err());
    assert_eq!(disk::read_to_string(&bad).unwrap(), "not sqlite");
    let future = root.path().join("future");
    drop(Database::open(&future).unwrap());
    {
        let db = rusqlite::Connection::open(&future).unwrap();
        db.pragma_update(None, "user_version", 99).unwrap();
    }
    let before = disk::read(&future).unwrap();
    assert!(Database::open(&future).is_err());
    assert_eq!(disk::read(&future).unwrap(), before);
    let symlink = root.path().join("link");
    std::os::unix::fs::symlink(&bad, &symlink).unwrap();
    assert!(Database::open(&symlink).is_err());
    assert!(disk::symlink_metadata(&symlink).unwrap().is_symlink());
}

#[test]
fn failure_before_publication_preserves_files_and_failure_after_it_keeps_completion_counts() {
    for after in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("operations.sqlite3");
        let mut db = Database::open(&path).unwrap();
        let source = root.path().join("input");
        let dest = root.path().join("dest");
        disk::write(&source, "data").unwrap();
        disk::create_dir(&dest).unwrap();
        let sql = if after {
            "CREATE TRIGGER reject_commit BEFORE UPDATE OF state ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(FAIL,'injected result failure'); END;"
        } else {
            "CREATE TRIGGER reject_intent BEFORE INSERT ON steps BEGIN SELECT RAISE(FAIL,'injected intent failure'); END;"
        };
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        let report = run(
            &mut db,
            Operation::Transfer {
                sources: vec![source.clone()],
                destination: dest.clone(),
                cut: true,
            },
            Choice::KeepBoth,
        );
        assert!(report.error.is_some(), "{report:?}");
        assert_eq!(report.completed.len(), usize::from(after));
        assert_eq!(source.exists(), !after);
        assert_eq!(dest.join("input").exists(), after);
        assert_eq!(
            db.history().unwrap()[0]
                .summary
                .as_ref()
                .unwrap()
                .progress
                .items_done,
            usize::from(after)
        );
    }
}

#[test]
fn a_journal_failure_after_claiming_a_replaced_target_restores_it_without_overwriting() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("operations.sqlite3");
    let mut db = Database::open(&path).unwrap();
    let source = root.path().join("file");
    let dest = root.path().join("dest");
    disk::write(&source, "incoming").unwrap();
    disk::create_dir(&dest).unwrap();
    disk::write(dest.join("file"), "original").unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_publish BEFORE INSERT ON steps WHEN NEW.kind='publish' BEGIN SELECT RAISE(FAIL,'publication intent failed'); END;").unwrap();
    let report = run(
        &mut db,
        Operation::Transfer {
            sources: vec![source.clone()],
            destination: dest.clone(),
            cut: false,
        },
        Choice::Replace,
    );
    assert!(
        report.error.is_some() && report.completed.is_empty(),
        "{report:?}"
    );
    assert_eq!(disk::read_to_string(&source).unwrap(), "incoming");
    assert_eq!(disk::read_to_string(dest.join("file")).unwrap(), "original");
    assert_eq!(disk::read_dir(&dest).unwrap().count(), 1);
    let sql = rusqlite::Connection::open(&path).unwrap();
    let restored: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM steps WHERE kind='restore_replaced' AND state='done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(restored, 1);
}

#[test]
fn cross_device_source_removal_requires_a_durable_intent_and_retains_both_copies_on_failure() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir_in("/dev/shm").unwrap();
    assert_ne!(
        root.path().metadata().unwrap().dev(),
        remote.path().metadata().unwrap().dev()
    );
    let path = root.path().join("operations.sqlite3");
    let mut db = Database::open(&path).unwrap();
    let source = root.path().join("file");
    disk::write(&source, "incoming").unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch("CREATE TRIGGER reject_removal BEFORE INSERT ON steps WHEN NEW.kind='remove_source' BEGIN SELECT RAISE(FAIL,'removal intent failed'); END;").unwrap();
    let report = run(
        &mut db,
        Operation::Transfer {
            sources: vec![source.clone()],
            destination: remote.path().into(),
            cut: true,
        },
        Choice::KeepBoth,
    );
    assert!(
        report.error.is_some() && report.completed.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.retained_copies.len(), 1);
    assert_eq!(disk::read_to_string(source).unwrap(), "incoming");
    assert_eq!(
        disk::read_to_string(remote.path().join("file")).unwrap(),
        "incoming"
    );
    assert_eq!(report.journal.unwrap().items[0].state, "partial");
}

#[test]
fn cancellation_is_persisted_and_a_long_copy_records_its_staging_location() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("operations.sqlite3");
    let mut db = Database::open(&path).unwrap();
    let file = root.path().join("big");
    disk::File::create(&file)
        .unwrap()
        .set_len(8 * 1024 * 1024)
        .unwrap();
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let op = Operation::Transfer {
        sources: vec![file.clone()],
        destination: dest.clone(),
        cut: false,
    };
    let cancellation = Cancellation::default();
    let mut recorder = db.begin(&op, 99).unwrap();
    let report = fs::operate_with_journal(
        op,
        &cancellation,
        &mut |p| {
            if p.bytes_done > 0 {
                cancellation.cancel();
            }
        },
        &mut |_| Decision::CANCEL,
        &mut recorder,
    );
    drop(recorder);
    assert!(
        report.cancelled && report.completed.is_empty() && report.error.is_none(),
        "{report:?}"
    );
    assert!(!dest.join("big").exists());
    let history = db.history().unwrap();
    assert_eq!(history[0].state, "cancelled");
    assert!(!history[0].items[0].recovery_paths.is_empty());
    assert!(file.exists());
    assert_eq!(disk::read_dir(dest).unwrap().count(), 0);
}

#[test]
fn non_utf8_files_and_symlink_identity_round_trip_in_blob_paths() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("operations.sqlite3")).unwrap();
    let name = std::ffi::OsString::from_vec(b"file-\xff".to_vec());
    let file = root.path().join(&name);
    disk::write(&file, "raw").unwrap();
    let report = run(
        &mut db,
        Operation::Duplicate {
            sources: vec![file.clone()],
            destination: root.path().into(),
        },
        Choice::KeepBoth,
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(
        report.journal.unwrap().items[0].source,
        file.as_os_str().as_bytes()
    );
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&name, &link).unwrap();
    let report = run(
        &mut db,
        Operation::Duplicate {
            sources: vec![link],
            destination: root.path().into(),
        },
        Choice::KeepBoth,
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(
        report.journal.unwrap().items[0]
            .after
            .as_ref()
            .unwrap()
            .link
            .as_deref(),
        Some(name.as_bytes())
    );
}

#[test]
fn history_is_bounded_without_losing_the_full_database_or_truthful_batch_counts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("operations.sqlite3");
    let mut db = Database::open(&path).unwrap();
    for i in 0..22 {
        let r = run(
            &mut db,
            Operation::Create {
                parent: root.path().into(),
                name: format!("file-{i}"),
                directory: false,
            },
            Choice::KeepBoth,
        );
        assert!(r.error.is_none(), "{r:?}");
    }
    assert_eq!(db.history().unwrap().len(), 20);
    let sql = rusqlite::Connection::open(&path).unwrap();
    let count: i64 = sql
        .query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 22);
    let files: Vec<_> = (0..205)
        .map(|i| {
            let p = root.path().join(format!("batch-{i}"));
            disk::write(&p, "").unwrap();
            p
        })
        .collect();
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let r = run(
        &mut db,
        Operation::Transfer {
            sources: files,
            destination: dest,
            cut: false,
        },
        Choice::KeepBoth,
    );
    assert!(r.error.is_none(), "{r:?}");
    assert_eq!(r.completed.len(), 205);
    let record = db.history().unwrap().remove(0).into_record(1);
    assert_eq!(record.progress.items_done, 205);
    assert_eq!(record.completed_paths.len(), 200);
}

#[test]
fn cancelling_while_the_intent_waits_for_a_database_lock_prevents_publication() {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("operations.sqlite3");
    let mut db = Database::open(&path).unwrap();
    let file = root.path().join("input");
    disk::write(&file, "x").unwrap();
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let op = Operation::Transfer {
        sources: vec![file.clone()],
        destination: dest.clone(),
        cut: false,
    };
    let cancellation = Cancellation::default();
    let locked = Arc::new(AtomicBool::new(false));
    let mut release = None;
    let mut recorder = db.begin(&op, 10).unwrap();
    let report = fs::operate_with_journal(
        op,
        &cancellation,
        &mut |p| {
            if p.bytes_done > 0 && !locked.swap(true, Ordering::SeqCst) {
                let lock = rusqlite::Connection::open(&path).unwrap();
                lock.execute_batch("BEGIN IMMEDIATE").unwrap();
                let token = cancellation.clone();
                release = Some(std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(100));
                    token.cancel();
                    std::thread::sleep(Duration::from_millis(20));
                    lock.execute_batch("ROLLBACK").unwrap();
                }));
            }
        },
        &mut |_| Decision::CANCEL,
        &mut recorder,
    );
    release.unwrap().join().unwrap();
    assert!(
        report.cancelled && report.error.is_none() && report.completed.is_empty(),
        "{report:?}"
    );
    assert!(file.exists() && !dest.join("input").exists());
    assert_eq!(disk::read_dir(dest).unwrap().count(), 0);
}

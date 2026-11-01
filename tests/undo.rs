use basalgrim::{
    conflicts::{Choice, Decision},
    fs::{self, Operation},
    journal::{Database, path},
    operations::Cancellation,
};
use std::{
    fs as disk,
    path::{Path, PathBuf},
};

fn run(db: &mut Database, operation: Operation, choice: Choice) -> fs::Report {
    let mut recorder = db.begin(&operation, 7).unwrap();
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
fn done(report: fs::Report) -> i64 {
    assert!(report.error.is_none(), "{report:?}");
    assert!(!report.cancelled);
    report.journal.unwrap().id
}
fn replay(db: &mut Database, id: i64, redo: bool) -> fs::Report {
    db.replay(id, redo, 9, &Cancellation::default(), &mut |_| {})
}
fn create(db: &mut Database, parent: &Path, name: &str, directory: bool) -> i64 {
    done(run(
        db,
        Operation::Create {
            parent: parent.into(),
            name: name.into(),
            directory,
        },
        Choice::KeepBoth,
    ))
}
fn copy(db: &mut Database, source: &Path, destination: &Path, cut: bool, choice: Choice) -> i64 {
    done(run(
        db,
        Operation::Transfer {
            sources: vec![source.into()],
            destination: destination.into(),
            cut,
        },
        choice,
    ))
}
fn tree(root: &Path, text: &str) {
    disk::create_dir_all(root.join("nested")).unwrap();
    disk::write(root.join("nested/data"), text).unwrap();
    std::os::unix::fs::symlink("missing", root.join("link")).unwrap();
}

#[test]
fn create_copy_duplicate_and_redo_survive_restart_without_original_source() {
    let root = tempfile::tempdir().unwrap();
    let dbpath = root.path().join("journal.sqlite3");
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let mut db = Database::open(&dbpath).unwrap();
    for directory in [false, true] {
        let name = if directory { "folder" } else { "file" };
        let id = create(&mut db, root.path(), name, directory);
        done(replay(&mut db, id, false));
        assert!(!root.path().join(name).exists());
        done(replay(&mut db, id, true));
        assert!(root.path().join(name).exists());
    }
    let source = root.path().join("original");
    tree(&source, "contents");
    let id = copy(&mut db, &source, &dest, false, Choice::KeepBoth);
    assert_eq!(db.history_state(id).unwrap().undo, 1);
    done(replay(&mut db, id, false));
    assert!(!dest.join("original").exists());
    disk::remove_dir_all(&source).unwrap();
    drop(db);
    let mut db = Database::open(&dbpath).unwrap();
    assert_eq!(db.history_state(id).unwrap().redo, 1);
    done(replay(&mut db, id, true));
    assert_eq!(
        disk::read_to_string(dest.join("original/nested/data")).unwrap(),
        "contents"
    );
    assert_eq!(
        disk::read_link(dest.join("original/link")).unwrap(),
        PathBuf::from("missing")
    );
    let duplicate = done(run(
        &mut db,
        Operation::Duplicate {
            sources: vec![dest.join("original")],
            destination: dest.clone(),
        },
        Choice::KeepBoth,
    ));
    assert!(dest.join("original (1)").exists());
    let undone = replay(&mut db, duplicate, false);
    assert_eq!(undone.completed, vec![dest.join("original (1)")]);
    assert_eq!(
        path(&undone.journal.as_ref().unwrap().items[0].source),
        dest.join("original (1)")
    );
    done(undone);
    assert!(!dest.join("original (1)").exists());
    done(replay(&mut db, duplicate, true));
    assert!(dest.join("original (1)").exists());
}

#[test]
fn rename_move_keep_both_and_chained_parent_creation_undo_in_order() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let parent = create(&mut db, root.path(), "parent", true);
    let file = create(&mut db, &root.path().join("parent"), "a", false);
    let rename = done(run(
        &mut db,
        Operation::Rename {
            source: root.path().join("parent/a"),
            name: "b".into(),
        },
        Choice::KeepBoth,
    ));
    done(replay(&mut db, rename, false));
    done(replay(&mut db, file, false));
    assert!(
        disk::read_dir(root.path().join("parent"))
            .unwrap()
            .next()
            .is_none()
    );
    done(replay(&mut db, parent, false));
    assert!(!root.path().join("parent").exists());
    done(replay(&mut db, parent, true));
    done(replay(&mut db, file, true));
    assert!(root.path().join("parent/a").exists());
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    disk::write(dest.join("a"), "old").unwrap();
    let moved = copy(
        &mut db,
        &root.path().join("parent/a"),
        &dest,
        true,
        Choice::KeepBoth,
    );
    assert!(dest.join("a (1)").exists());
    done(replay(&mut db, moved, false));
    assert!(root.path().join("parent/a").exists());
    assert_eq!(disk::read_to_string(dest.join("a")).unwrap(), "old");
    done(replay(&mut db, moved, true));
    assert!(dest.join("a (1)").exists());
}

#[test]
fn replacement_undo_and_redo_keep_both_complete_trees() {
    for cut in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut db = Database::open(&root.path().join("journal")).unwrap();
        let source = root.path().join("data");
        tree(&source, "incoming");
        let dest = root.path().join("dest");
        disk::create_dir(&dest).unwrap();
        tree(&dest.join("data"), "old");
        let id = copy(&mut db, &source, &dest, cut, Choice::Replace);
        assert_eq!(db.history_state(id).unwrap().undo, 1);
        for _ in 0..2 {
            done(replay(&mut db, id, false));
            assert_eq!(
                disk::read_to_string(dest.join("data/nested/data")).unwrap(),
                "old"
            );
            assert!(source.exists());
            done(replay(&mut db, id, true));
            assert_eq!(
                disk::read_to_string(dest.join("data/nested/data")).unwrap(),
                "incoming"
            );
            assert_eq!(source.exists(), !cut);
        }
    }
}

#[test]
fn changes_inside_trees_and_occupied_targets_are_rejected_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let source = root.path().join("data");
    tree(&source, "original");
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let id = copy(&mut db, &source, &dest, false, Choice::KeepBoth);
    let nested = dest.join("data/nested/data");
    let modified = disk::metadata(&nested).unwrap().modified().unwrap();
    disk::write(&nested, "modified").unwrap();
    disk::File::open(&nested)
        .unwrap()
        .set_times(disk::FileTimes::new().set_modified(modified))
        .unwrap();
    let rejected = replay(&mut db, id, false);
    assert!(rejected.error.as_ref().unwrap().contains("изменились"));
    assert!(rejected.completed.is_empty());
    assert_eq!(disk::read_to_string(&nested).unwrap(), "modified");
    let moved = copy(&mut db, &source, &dest, true, Choice::KeepBoth);
    tree(&source, "new owner");
    let rejected = replay(&mut db, moved, false);
    assert!(rejected.error.as_ref().unwrap().contains("существует"));
    assert_eq!(
        disk::read_to_string(source.join("nested/data")).unwrap(),
        "new owner"
    );
    assert!(dest.join("data (1)").exists());
}

#[test]
fn partial_cancelled_batches_track_only_atomic_completed_items() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let source = root.path().join("source");
    disk::create_dir(&source).unwrap();
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    for name in ["a", "b", "c"] {
        disk::write(source.join(name), name).unwrap();
    }
    let operation = Operation::Transfer {
        sources: ["a", "b", "c"].map(|n| source.join(n)).to_vec(),
        destination: dest.clone(),
        cut: false,
    };
    let id = done(run(&mut db, operation, Choice::KeepBoth));
    let cancel = Cancellation::default();
    let report = db.replay(id, false, 90, &cancel, &mut |p| {
        if p.items_done >= 1 {
            cancel.cancel();
        }
    });
    assert!(report.cancelled, "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert_eq!(db.history_state(id).unwrap().redo, 1);
    assert_eq!(db.history_state(id).unwrap().undo, 2);
    done(replay(&mut db, id, false));
    assert_eq!(db.history_state(id).unwrap().redo, 3);
    done(replay(&mut db, id, true));
    assert!(["a", "b", "c"].iter().all(|n| dest.join(n).exists()));
}

#[test]
fn journal_failures_before_and_after_undo_never_lose_completed_data() {
    for after in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let dbpath = root.path().join("journal");
        let mut db = Database::open(&dbpath).unwrap();
        let id = create(&mut db, root.path(), "new", false);
        let sql = rusqlite::Connection::open(&dbpath).unwrap();
        sql.execute_batch(if after {"CREATE TRIGGER fault BEFORE UPDATE ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(ABORT,'fault'); END;"}else{"CREATE TRIGGER fault BEFORE INSERT ON steps BEGIN SELECT RAISE(ABORT,'fault'); END;"}).unwrap();
        let report = replay(&mut db, id, false);
        assert!(report.error.is_some(), "{report:?}");
        assert_eq!(report.completed.len(), usize::from(after));
        assert_eq!(root.path().join("new").exists(), !after);
        sql.execute_batch("DROP TRIGGER fault;").unwrap();
        if after {
            done(replay(&mut db, id, true));
        } else {
            done(replay(&mut db, id, false));
        }
    }
}

#[test]
fn result_recording_failure_keeps_recovery_paths_and_blocks_automatic_retry() {
    let root = tempfile::tempdir().unwrap();
    let dbpath = root.path().join("journal");
    let mut db = Database::open(&dbpath).unwrap();
    let id = create(&mut db, root.path(), "new", false);
    let sql = rusqlite::Connection::open(&dbpath).unwrap();
    sql.execute_batch("CREATE TRIGGER fault BEFORE UPDATE ON undo_items WHEN NEW.state='undone' BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
    let report = replay(&mut db, id, false);
    assert!(report.error.is_some());
    assert_eq!(report.completed.len(), 1);
    let data: Vec<u8> = sql
        .query_row(
            "SELECT data FROM undo_items WHERE operation_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let retained = path(
        &plan["parking"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect::<Vec<_>>(),
    );
    assert!(retained.exists());
    sql.execute_batch("DROP TRIGGER fault;").unwrap();
    drop(db);
    let mut db = Database::open(&dbpath).unwrap();
    db.history().unwrap();
    assert!(db.history_state(id).unwrap().blocked.is_some());
    assert!(replay(&mut db, id, true).error.is_some());
    assert!(retained.exists());
}

#[test]
fn failure_in_the_second_replacement_step_restores_the_visible_result() {
    for after_first_step in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let dbpath = root.path().join("journal");
        let mut db = Database::open(&dbpath).unwrap();
        let source = root.path().join("data");
        disk::write(&source, "incoming").unwrap();
        let dest = root.path().join("dest");
        disk::create_dir(&dest).unwrap();
        disk::write(dest.join("data"), "old").unwrap();
        let id = copy(&mut db, &source, &dest, false, Choice::Replace);
        let sql = rusqlite::Connection::open(&dbpath).unwrap();
        sql.execute_batch(if after_first_step {"CREATE TRIGGER fault BEFORE UPDATE ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(ABORT,'fault'); END;"}else{"CREATE TRIGGER fault BEFORE INSERT ON steps WHEN NEW.kind='publish' AND CAST(NEW.source AS TEXT) LIKE '%/.basalgrim-replaced-%' BEGIN SELECT RAISE(ABORT,'fault'); END;"}).unwrap();
        let report = replay(&mut db, id, false);
        assert!(report.error.is_some());
        assert!(report.completed.is_empty());
        assert_eq!(disk::read_to_string(dest.join("data")).unwrap(), "incoming");
        sql.execute_batch("DROP TRIGGER fault;").unwrap();
        assert_eq!(db.history_state(id).unwrap().undo, 1);
        done(replay(&mut db, id, false));
        assert_eq!(disk::read_to_string(dest.join("data")).unwrap(), "old");
    }
}

#[test]
fn old_schema_is_migrated_without_executing_legacy_inverse_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let dbpath = root.path().join("journal");
    let mut db = Database::open(&dbpath).unwrap();
    let old = create(&mut db, root.path(), "old", false);
    drop(db);
    let sql = rusqlite::Connection::open(&dbpath).unwrap();
    sql.execute_batch("DROP TABLE replay_runs;DROP TABLE undo_items;PRAGMA user_version=1;")
        .unwrap();
    drop(sql);
    let mut db = Database::open(&dbpath).unwrap();
    assert!(db.history_state(old).unwrap().blocked.is_some());
    assert!(replay(&mut db, old, false).error.is_some());
    assert!(root.path().join("old").exists());
    let new = create(&mut db, root.path(), "new", false);
    done(replay(&mut db, new, false));
    assert!(!root.path().join("new").exists());
}

#[test]
fn raw_unix_names_and_symlink_targets_roundtrip_losslessly() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let source = root
        .path()
        .join(std::ffi::OsString::from_vec(b"raw-\xff\n%".to_vec()));
    let target = PathBuf::from(std::ffi::OsString::from_vec(b"target-\xfe".to_vec()));
    std::os::unix::fs::symlink(&target, &source).unwrap();
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let id = copy(&mut db, &source, &dest, false, Choice::KeepBoth);
    let result = dest.join(source.file_name().unwrap());
    done(replay(&mut db, id, false));
    assert!(disk::symlink_metadata(&result).is_err());
    done(replay(&mut db, id, true));
    assert_eq!(disk::read_link(result).unwrap(), target);
}

#[test]
fn blocked_intent_rechecks_source_changes_and_cancellation_before_mutation() {
    use std::{sync::mpsc, thread, time::Duration};
    for cancel in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let dbpath = root.path().join("journal");
        let mut db = Database::open(&dbpath).unwrap();
        let id = create(&mut db, root.path(), "new", false);
        let source = root.path().join("new");
        let (acquire, commands) = mpsc::channel();
        let (locked, ack) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let locker = thread::spawn(move || {
            commands.recv().unwrap();
            let sql = rusqlite::Connection::open(dbpath).unwrap();
            sql.execute_batch("BEGIN IMMEDIATE;").unwrap();
            locked.send(()).unwrap();
            released.recv().unwrap();
            sql.execute_batch("ROLLBACK;").unwrap();
        });
        let cancellation = Cancellation::default();
        let operation_cancel = cancellation.clone();
        let worker = thread::spawn(move || {
            let mut paused = false;
            db.replay(id, false, 50, &operation_cancel, &mut |p| {
                if !paused && p.phase == basalgrim::operations::Phase::Running {
                    paused = true;
                    acquire.send(()).unwrap();
                    ack.recv().unwrap();
                    ready.send(()).unwrap();
                }
            })
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        thread::sleep(Duration::from_millis(80));
        assert!(source.exists());
        if cancel {
            cancellation.cancel();
        } else {
            disk::write(&source, "external update").unwrap();
        }
        release.send(()).unwrap();
        let report = worker.join().unwrap();
        locker.join().unwrap();
        assert!(report.completed.is_empty());
        assert!(source.exists());
        if cancel {
            assert!(report.cancelled, "{report:?}");
        } else {
            assert!(report.error.as_ref().unwrap().contains("изменился"));
            assert_eq!(disk::read_to_string(source).unwrap(), "external update");
        }
    }
}

#[test]
fn cross_device_moves_can_be_undone_and_redone_with_full_verification() {
    if !Path::new("/dev/shm").is_dir() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir_in("/dev/shm").unwrap();
    use std::os::unix::fs::MetadataExt;
    if disk::metadata(root.path()).unwrap().dev() == disk::metadata(other.path()).unwrap().dev() {
        return;
    }
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let source = root.path().join("data");
    tree(&source, "cross-device contents");
    let id = copy(&mut db, &source, other.path(), true, Choice::KeepBoth);
    done(replay(&mut db, id, false));
    assert!(source.join("nested/data").exists());
    assert!(!other.path().join("data").exists());
    done(replay(&mut db, id, true));
    assert!(!source.exists());
    assert_eq!(
        disk::read_to_string(other.path().join("data/nested/data")).unwrap(),
        "cross-device contents"
    );
}

#[test]
fn cross_device_inverse_rebinds_prior_copy_evidence_without_adopting_external_inodes() {
    use std::os::unix::fs::MetadataExt;
    if !Path::new("/dev/shm").is_dir() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir_in("/dev/shm").unwrap();
    if disk::metadata(root.path()).unwrap().dev() == disk::metadata(other.path()).unwrap().dev() {
        return;
    }
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let source = root.path().join("tree");
    tree(&source, "original");
    let dest = root.path().join("dest");
    disk::create_dir(&dest).unwrap();
    let copied = copy(&mut db, &source, &dest, false, Choice::KeepBoth);
    let nested = dest.join("tree/nested/data");
    let moved = copy(&mut db, &nested, other.path(), true, Choice::KeepBoth);
    for _ in 0..2 {
        done(replay(&mut db, moved, false));
        done(replay(&mut db, moved, true));
    }
    done(replay(&mut db, moved, false));
    done(replay(&mut db, copied, false));
    assert!(!dest.join("tree").exists());
    done(replay(&mut db, copied, true));
    assert_eq!(disk::read_to_string(&nested).unwrap(), "original");
    done(replay(&mut db, moved, true));
    let target = other.path().join("data");
    let modified = disk::metadata(&target).unwrap().modified().unwrap();
    disk::write(other.path().join("replacement"), "original").unwrap();
    disk::rename(other.path().join("replacement"), &target).unwrap();
    disk::File::open(&target)
        .unwrap()
        .set_times(disk::FileTimes::new().set_modified(modified))
        .unwrap();
    let rejected = replay(&mut db, moved, false);
    assert!(rejected.error.is_some());
    assert!(rejected.completed.is_empty());
    assert!(target.exists());
    assert!(!nested.exists());
}

#[test]
fn trash_undo_redo_in_an_isolated_subprocess() {
    let root = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "isolated_trash_child", "--nocapture"])
        .env("BASALGRIM_UNDO_TRASH_TEST", root.path())
        .env("HOME", root.path())
        .env("XDG_DATA_HOME", root.path().join("data"))
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
fn isolated_trash_child() {
    let Some(root) = std::env::var_os("BASALGRIM_UNDO_TRASH_TEST").map(PathBuf::from) else {
        return;
    };
    let mut db = Database::open(&root.join("journal")).unwrap();
    let source = root.join("folder with % space");
    tree(&source, "trashed contents");
    let id = done(run(
        &mut db,
        Operation::Trash {
            sources: vec![source.clone()],
        },
        Choice::KeepBoth,
    ));
    assert_eq!(
        db.history_state(id).unwrap().undo,
        1,
        "{:?}",
        db.history_state(id).unwrap()
    );
    assert!(!source.exists());
    for _ in 0..2 {
        done(replay(&mut db, id, false));
        assert_eq!(
            disk::read_to_string(source.join("nested/data")).unwrap(),
            "trashed contents"
        );
        done(replay(&mut db, id, true));
        assert!(!source.exists());
    }
    done(replay(&mut db, id, false));
    assert!(
        root.join("data/Trash/files")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
}

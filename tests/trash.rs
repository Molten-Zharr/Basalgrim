use basalgrim::{
    conflicts::{Choice, Decision},
    fs::{self, Operation},
    journal::{Database, bytes},
    operations::Cancellation,
    trash::{self, Entry, Root},
};
use std::{
    fs as disk,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};

fn root_at(data: &Path) -> Root {
    for dir in [
        data.join("Trash"),
        data.join("Trash/files"),
        data.join("Trash/info"),
    ] {
        disk::create_dir_all(&dir).unwrap();
        disk::set_permissions(dir, disk::Permissions::from_mode(0o700)).unwrap();
    }
    Root::home(data.into())
}
fn metadata(root: &Root, id: &str, original: &Path) {
    let encoded = bytes(original)
        .into_iter()
        .map(|b| format!("%{b:02X}"))
        .collect::<String>();
    disk::write(
        root.directory.join("info").join(format!("{id}.trashinfo")),
        format!("[Trash Info]\nPath={encoded}\nDeletionDate=2026-11-01T12:30:45\n"),
    )
    .unwrap();
}
fn item(root: &Root, id: &str, original: &Path) -> Entry {
    disk::write(
        root.directory.join("files").join(id),
        format!("content-{id}"),
    )
    .unwrap();
    metadata(root, id, original);
    trash::read(std::slice::from_ref(root), &Default::default())
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.data.file_name().unwrap() == id)
        .unwrap()
}
fn restore(entries: Vec<Entry>, destination: Option<PathBuf>, choice: Choice) -> fs::Report {
    fs::operate_with_conflicts(
        Operation::RestoreTrash {
            entries,
            destination,
        },
        &Default::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice,
            remaining: true,
        },
    )
}
fn checked(report: &fs::Report) {
    assert!(report.error.is_none() && !report.cancelled, "{report:?}");
}
fn recorded(db: &mut Database, entries: Vec<Entry>, choice: Choice) -> fs::Report {
    let op = Operation::RestoreTrash {
        entries,
        destination: None,
    };
    let mut recorder = db.begin(&op, 1).unwrap();
    fs::operate_with_journal(
        op,
        &Default::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice,
            remaining: true,
        },
        &mut recorder,
    )
}

#[test]
fn catalog_decodes_raw_names_and_displays_broken_entries_without_guessing_destinations() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let raw = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"original-\xff".to_vec()));
    let entry = item(&root, "unrelated-storage-name", &raw);
    assert_eq!(entry.original.as_ref(), Some(&raw));
    assert!(entry.issue.is_none());
    assert!(!entry.name.contains("storage"));
    assert_eq!(entry.deleted, "2026-11-01 12:30:45");
    disk::write(root.directory.join("files/orphan"), "keep me").unwrap();
    disk::write(root.directory.join("files/invalid"), "keep too").unwrap();
    disk::write(
        root.directory.join("info/invalid.trashinfo"),
        "[Trash Info]\nPath=../escape\nDeletionDate=2026-11-01T12:30:45\n",
    )
    .unwrap();
    let catalog = trash::read(&[root], &Default::default()).unwrap();
    assert_eq!(catalog.entries.len(), 3);
    for entry in catalog
        .entries
        .iter()
        .filter(|e| e.data.file_name().unwrap() != "unrelated-storage-name")
    {
        assert!(entry.issue.is_some());
        assert!(entry.original.is_none());
    }
}

#[test]
fn restores_group_trees_symlinks_and_non_utf8_paths_without_following_targets() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let folder = temp.path().join("restored folder");
    disk::create_dir_all(root.directory.join("files/tree/nested")).unwrap();
    disk::write(root.directory.join("files/tree/nested/a"), "tree data").unwrap();
    metadata(&root, "tree", &folder);
    std::os::unix::fs::symlink("missing-target", root.directory.join("files/link")).unwrap();
    metadata(&root, "link", &temp.path().join("restored link"));
    let raw = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"raw-\xff".to_vec()));
    item(&root, "raw", &raw);
    let entries = trash::read(std::slice::from_ref(&root), &Default::default())
        .unwrap()
        .entries;
    let report = restore(entries, None, Choice::KeepBoth);
    checked(&report);
    assert_eq!(report.completed.len(), 3);
    assert_eq!(
        disk::read_to_string(folder.join("nested/a")).unwrap(),
        "tree data"
    );
    assert_eq!(
        disk::read_link(temp.path().join("restored link")).unwrap(),
        Path::new("missing-target")
    );
    assert_eq!(disk::read_to_string(raw).unwrap(), "content-raw");
    assert!(
        trash::read(&[root], &Default::default())
            .unwrap()
            .entries
            .is_empty()
    );
}

#[test]
fn conflicts_keep_both_replace_skip_and_cancel_preserve_the_right_objects() {
    for choice in [
        Choice::KeepBoth,
        Choice::Replace,
        Choice::Skip,
        Choice::Cancel,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = root_at(&temp.path().join("data"));
        let original = temp.path().join("same.txt");
        disk::write(&original, "existing").unwrap();
        let entry = item(&root, "stored", &original);
        let info = entry.info.clone();
        let data = entry.data.clone();
        let report = restore(vec![entry], None, choice);
        match choice {
            Choice::KeepBoth => {
                checked(&report);
                assert_eq!(disk::read_to_string(&original).unwrap(), "existing");
                assert_eq!(
                    disk::read_to_string(temp.path().join("same (1).txt")).unwrap(),
                    "content-stored"
                );
            }
            Choice::Replace => {
                checked(&report);
                assert_eq!(disk::read_to_string(&original).unwrap(), "content-stored");
            }
            Choice::Skip => {
                checked(&report);
                assert_eq!(report.skipped.as_slice(), std::slice::from_ref(&data));
                assert!(info.exists() && data.exists());
            }
            Choice::Cancel => {
                assert!(report.cancelled);
                assert!(info.exists() && data.exists());
            }
        }
        if matches!(choice, Choice::Skip | Choice::Cancel) {
            assert_eq!(disk::read_to_string(original).unwrap(), "existing");
        }
    }
}

#[test]
fn stale_data_metadata_and_directory_replacements_are_rejected_before_mutation() {
    for scenario in [0, 1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let root = root_at(&temp.path().join("data"));
        let original = temp.path().join("original");
        let entry = item(&root, "stored", &original);
        match scenario {
            0 => disk::write(&entry.data, "externally changed").unwrap(),
            1 => disk::write(&entry.info, "[Trash Info]\nPath=/different\n").unwrap(),
            _ => {
                disk::rename(
                    root.directory.join("files"),
                    root.directory.join("old-files"),
                )
                .unwrap();
                disk::create_dir(root.directory.join("files")).unwrap();
                disk::set_permissions(
                    root.directory.join("files"),
                    disk::Permissions::from_mode(0o700),
                )
                .unwrap();
                disk::write(&entry.data, "replacement").unwrap();
            }
        }
        let report = restore(vec![entry.clone()], None, Choice::Replace);
        assert!(report.error.is_some());
        assert!(report.completed.is_empty());
        assert!(!original.exists());
        assert!(entry.info.exists());
    }
}

#[test]
fn source_changes_while_waiting_for_a_conflict_do_not_replace_the_destination() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let original = temp.path().join("original");
    disk::write(&original, "existing").unwrap();
    let entry = item(&root, "stored", &original);
    let report = fs::operate_with_conflicts(
        Operation::RestoreTrash {
            entries: vec![entry.clone()],
            destination: None,
        },
        &Default::default(),
        &mut |_| {},
        &mut |_| {
            disk::write(&entry.data, "external edit during conflict").unwrap();
            Decision {
                choice: Choice::Replace,
                remaining: false,
            }
        },
    );
    assert!(report.error.is_some());
    assert_eq!(disk::read_to_string(original).unwrap(), "existing");
    assert!(entry.info.exists());
}

#[test]
fn unavailable_original_parent_can_be_redirected_to_a_chosen_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let original = temp.path().join("missing/file.txt");
    let entry = item(&root, "stored", &original);
    let report = restore(vec![entry.clone()], None, Choice::KeepBoth);
    assert!(report.error.is_some());
    assert!(entry.info.exists());
    let report = restore(vec![entry], Some(temp.path().into()), Choice::KeepBoth);
    checked(&report);
    assert_eq!(
        disk::read_to_string(temp.path().join("file.txt")).unwrap(),
        "content-stored"
    );
}

#[test]
fn unsafe_trash_roots_and_metadata_symlinks_are_never_used() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let entry = item(&root, "stored", &temp.path().join("original"));
    disk::remove_file(&entry.info).unwrap();
    std::os::unix::fs::symlink(temp.path().join("outside"), &entry.info).unwrap();
    let catalog = trash::read(std::slice::from_ref(&root), &Default::default()).unwrap();
    assert!(catalog.entries[0].issue.is_some());
    disk::set_permissions(&root.directory, disk::Permissions::from_mode(0o777)).unwrap();
    let catalog = trash::read(&[root], &Default::default()).unwrap();
    assert!(catalog.entries.is_empty());
    assert!(!catalog.warnings.is_empty());
}

#[test]
fn restoration_undo_redo_retains_exact_metadata_and_survives_restart() {
    for choice in [Choice::KeepBoth, Choice::Replace] {
        let temp = tempfile::tempdir().unwrap();
        let root = root_at(&temp.path().join("data"));
        let original = temp.path().join("same.txt");
        disk::write(&original, "existing").unwrap();
        let entry = item(&root, "stored", &original);
        let metadata = disk::read(&entry.info).unwrap();
        let dbpath = temp.path().join("journal.sqlite3");
        let mut db = Database::open(&dbpath).unwrap();
        let report = recorded(&mut db, vec![entry.clone()], choice);
        checked(&report);
        let id = report.journal.unwrap().id;
        assert_eq!(db.history_state(id).unwrap().undo, 1);
        assert!(!entry.info.exists());
        let target = if choice == Choice::KeepBoth {
            temp.path().join("same (1).txt")
        } else {
            original.clone()
        };
        assert_eq!(disk::read_to_string(&target).unwrap(), "content-stored");
        let report = db.replay(id, false, 2, &Default::default(), &mut |_| {});
        checked(&report);
        assert_eq!(disk::read(&entry.info).unwrap(), metadata);
        assert_eq!(disk::read_to_string(&original).unwrap(), "existing");
        assert_eq!(disk::read_to_string(&entry.data).unwrap(), "content-stored");
        drop(db);
        let mut db = Database::open(&dbpath).unwrap();
        let report = db.replay(id, true, 3, &Default::default(), &mut |_| {});
        checked(&report);
        assert_eq!(disk::read_to_string(target).unwrap(), "content-stored");
        assert!(!entry.info.exists());
    }
}

#[test]
fn a_journal_failure_after_claim_recovers_metadata_and_preserves_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let original = temp.path().join("original");
    let entry = item(&root, "stored", &original);
    let dbpath = temp.path().join("journal.sqlite3");
    let mut db = Database::open(&dbpath).unwrap();
    let connection = rusqlite::Connection::open(&dbpath).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_claim BEFORE UPDATE OF state ON steps WHEN NEW.kind='trash_info_claim' BEGIN SELECT RAISE(ABORT,'injected journal failure'); END;").unwrap();
    let report = recorded(&mut db, vec![entry.clone()], Choice::KeepBoth);
    assert!(report.error.is_some());
    assert!(report.completed.is_empty());
    assert!(entry.info.exists() && entry.data.exists());
    assert!(!original.exists());
}

#[test]
fn cancelled_listing_and_restore_leave_the_catalog_and_objects_intact() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let entry = item(&root, "stored", &temp.path().join("original"));
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(trash::read(&[root], &cancel).is_err());
    let report = fs::operate_with_control(
        Operation::RestoreTrash {
            entries: vec![entry.clone()],
            destination: None,
        },
        &cancel,
        &mut |_| {},
    );
    assert!(report.cancelled);
    assert!(entry.info.exists() && entry.data.exists());
}

#[test]
fn published_restore_is_counted_and_kept_when_journal_recording_fails() {
    let temp = tempfile::tempdir().unwrap();
    let root = root_at(&temp.path().join("data"));
    let original = temp.path().join("original");
    let entry = item(&root, "stored", &original);
    let dbpath = temp.path().join("journal.sqlite3");
    let mut db = Database::open(&dbpath).unwrap();
    let connection = rusqlite::Connection::open(&dbpath).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_publish BEFORE UPDATE OF state ON steps WHEN NEW.kind='publish' BEGIN SELECT RAISE(ABORT,'recording failed after publication'); END;").unwrap();
    let report = recorded(&mut db, vec![entry.clone()], Choice::KeepBoth);
    assert!(report.error.is_some());
    assert_eq!(
        report.completed.as_slice(),
        std::slice::from_ref(&entry.data)
    );
    assert_eq!(disk::read_to_string(&original).unwrap(), "content-stored");
    assert!(!entry.data.exists());
    assert!(!entry.info.exists());
    let retained: Vec<_> = disk::read_dir(&root.directory)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b".basalgrim-restore-")
        })
        .collect();
    assert_eq!(retained.len(), 1);
    assert!(retained[0].join("info").exists());
}

#[test]
fn cross_filesystem_restore_and_undo_use_verified_moves_and_keep_metadata() {
    let Ok(remote) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    use std::os::unix::fs::MetadataExt;
    if disk::metadata(remote.path()).unwrap().dev() == disk::metadata(temp.path()).unwrap().dev() {
        return;
    }
    let root = root_at(&remote.path().join("data"));
    let original = temp.path().join("original");
    let entry = item(&root, "stored", &original);
    let mut db = Database::open(&temp.path().join("journal.sqlite3")).unwrap();
    let report = recorded(&mut db, vec![entry.clone()], Choice::KeepBoth);
    checked(&report);
    let id = report.journal.unwrap().id;
    assert_eq!(disk::read_to_string(&original).unwrap(), "content-stored");
    checked(&db.replay(id, false, 2, &Default::default(), &mut |_| {}));
    assert!(!original.exists());
    assert!(entry.data.exists() && entry.info.exists());
    checked(&db.replay(id, true, 3, &Default::default(), &mut |_| {}));
    assert!(original.exists());
    assert!(!entry.data.exists() && !entry.info.exists());
}

#[test]
fn restore_undo_refuses_changed_results_reused_slots_and_changed_retained_metadata() {
    for scenario in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let root = root_at(&temp.path().join("data"));
        let original = temp.path().join("original");
        let entry = item(&root, "stored", &original);
        let mut db = Database::open(&temp.path().join("journal.sqlite3")).unwrap();
        let report = recorded(&mut db, vec![entry.clone()], Choice::KeepBoth);
        checked(&report);
        let id = report.journal.unwrap().id;
        let parked = disk::read_dir(&root.directory)
            .unwrap()
            .map(|p| p.unwrap().path().join("info"))
            .find(|p| p.is_file())
            .unwrap();
        match scenario {
            0 => disk::write(&original, "external edit").unwrap(),
            1 => disk::write(&entry.data, "another trashed file").unwrap(),
            _ => disk::write(&parked, "external metadata").unwrap(),
        }
        let report = db.replay(id, false, 2, &Default::default(), &mut |_| {});
        assert!(report.error.is_some());
        assert!(report.completed.is_empty());
        assert_eq!(
            disk::read_to_string(&original).unwrap(),
            if scenario == 0 {
                "external edit"
            } else {
                "content-stored"
            }
        );
        if scenario == 1 {
            assert_eq!(
                disk::read_to_string(&entry.data).unwrap(),
                "another trashed file"
            );
        }
        if scenario == 2 {
            assert_eq!(disk::read_to_string(&parked).unwrap(), "external metadata");
        }
    }
}

#[test]
fn both_volume_trash_layouts_resolve_relative_paths_and_accept_crlf_metadata() {
    for shared in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = Root::volume(temp.path().into(), shared);
        for dir in [
            root.directory.clone(),
            root.directory.join("files"),
            root.directory.join("info"),
        ] {
            disk::create_dir_all(&dir).unwrap();
            disk::set_permissions(dir, disk::Permissions::from_mode(0o700)).unwrap();
        }
        if shared {
            disk::set_permissions(
                temp.path().join(".Trash"),
                disk::Permissions::from_mode(0o1777),
            )
            .unwrap();
        }
        disk::create_dir(temp.path().join("original")).unwrap();
        disk::write(root.directory.join("files/stored"), "relative content").unwrap();
        disk::write(root.directory.join("info/stored.trashinfo"),"[Trash Info]\r\nPath=original/file.txt\r\nDeletionDate=2026-11-01T12:30:45\r\nUnknown=ignored\r\nPath=ignored\r\n").unwrap();
        let catalog = trash::read(&[root], &Default::default()).unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert!(catalog.entries[0].issue.is_none());
        checked(&restore(catalog.entries, None, Choice::KeepBoth));
        assert_eq!(
            disk::read_to_string(temp.path().join("original/file.txt")).unwrap(),
            "relative content"
        );
    }
}

fn deletions(root: &Root) -> Vec<trash::Deletion> {
    trash::read(std::slice::from_ref(root), &Default::default())
        .unwrap()
        .deletions
}
fn delete_operation(targets: Vec<trash::Deletion>) -> Operation {
    Operation::DeleteTrash {
        targets,
        empty: true,
    }
}

#[test]
fn permanent_delete_handles_trees_links_orphans_and_preserves_originals_and_history() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root_at(&tmp.path().join("data"));
    let external = tmp.path().join("untouched");
    disk::create_dir(&external).unwrap();
    disk::write(external.join("keep"), "original").unwrap();
    item(&root, "a", &external.join("a"));
    let tree = root.directory.join("files/tree");
    disk::create_dir(&tree).unwrap();
    disk::write(tree.join("nested"), "trash data").unwrap();
    std::os::unix::fs::symlink(&external, tree.join("outside")).unwrap();
    disk::write(root.directory.join("info/orphan.trashinfo"), "bad metadata").unwrap();
    let retained = root.directory.join(".basalgrim-restore-history");
    disk::create_dir(&retained).unwrap();
    disk::write(retained.join("keep"), "undo history").unwrap();
    let report = fs::operate(delete_operation(deletions(&root)));
    checked(&report);
    assert_eq!(report.completed.len(), 3);
    assert_eq!(
        disk::read_dir(root.directory.join("files"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        disk::read_dir(root.directory.join("info")).unwrap().count(),
        0
    );
    assert_eq!(
        disk::read_to_string(external.join("keep")).unwrap(),
        "original"
    );
    assert_eq!(
        disk::read_to_string(retained.join("keep")).unwrap(),
        "undo history"
    );
}

#[test]
fn stale_delete_targets_and_cancellation_do_not_delete_replacements() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root_at(&tmp.path().join("data"));
    let entry = item(&root, "a", &tmp.path().join("a"));
    let targets = deletions(&root);
    disk::rename(&entry.data, entry.data.with_extension("old")).unwrap();
    disk::write(&entry.data, "replacement").unwrap();
    let report = fs::operate(delete_operation(targets));
    assert!(report.error.is_some());
    assert!(report.completed.is_empty());
    assert_eq!(disk::read_to_string(&entry.data).unwrap(), "replacement");
    let cancel = Cancellation::default();
    cancel.cancel();
    let report = fs::operate_with_control(delete_operation(deletions(&root)), &cancel, &mut |_| {});
    assert!(report.cancelled);
    assert!(entry.data.exists() && entry.info.exists());
}

#[test]
fn delete_claim_journal_failure_recovers_data_and_permanent_delete_is_not_undoable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root_at(&tmp.path().join("data"));
    let entry = item(&root, "a", &tmp.path().join("a"));
    let db_path = tmp.path().join("journal.sqlite");
    let mut db = Database::open(&db_path).unwrap();
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_delete BEFORE UPDATE OF state ON steps WHEN NEW.kind='delete_claim' BEGIN SELECT RAISE(ABORT,'delete recording failed'); END;").unwrap();
    let op = delete_operation(deletions(&root));
    let mut recorder = db.begin(&op, 1).unwrap();
    let report = fs::operate_with_journal(
        op,
        &Default::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice: Choice::KeepBoth,
            remaining: false,
        },
        &mut recorder,
    );
    assert!(report.error.is_some());
    assert!(report.completed.is_empty());
    assert!(entry.data.exists() && entry.info.exists());
    drop(recorder);
    connection
        .execute_batch("DROP TRIGGER fail_delete")
        .unwrap();
    let op = delete_operation(deletions(&root));
    let mut recorder = db.begin(&op, 2).unwrap();
    let report = fs::operate_with_journal(
        op,
        &Default::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice: Choice::KeepBoth,
            remaining: false,
        },
        &mut recorder,
    );
    checked(&report);
    assert_eq!(report.completed.len(), 1);
    assert!(!entry.data.exists() && !entry.info.exists());
    assert_eq!(report.journal.as_ref().unwrap().history.undo, 0);
}

#[test]
fn cancellation_during_permanent_removal_finishes_the_current_item_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root_at(&tmp.path().join("data"));
    item(&root, "a", &tmp.path().join("a"));
    item(&root, "b", &tmp.path().join("b"));
    let targets = deletions(&root);
    let cancel = Cancellation::default();
    let report = fs::operate_with_control(delete_operation(targets), &cancel, &mut |progress| {
        if progress.entries_done > 0 {
            cancel.cancel();
        }
    });
    assert!(report.cancelled);
    assert_eq!(report.completed.len(), 1);
    assert_eq!(
        disk::read_dir(root.directory.join("files"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        disk::read_dir(root.directory.join("info")).unwrap().count(),
        1
    );
}

#[test]
fn permanent_deletion_stays_counted_when_recording_its_result_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root_at(&tmp.path().join("data"));
    let entry = item(&root, "a", &tmp.path().join("a"));
    let db_path = tmp.path().join("journal.sqlite");
    let mut db = Database::open(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_delete_result BEFORE UPDATE OF state ON steps WHEN NEW.kind='permanent_delete' BEGIN SELECT RAISE(ABORT,'result recording failed'); END;").unwrap();
    let op = delete_operation(deletions(&root));
    let mut recorder = db.begin(&op, 1).unwrap();
    let report = fs::operate_with_journal(
        op,
        &Default::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice: Choice::KeepBoth,
            remaining: false,
        },
        &mut recorder,
    );
    assert!(report.error.is_some());
    assert_eq!(report.completed.len(), 1);
    assert!(!entry.data.exists() && !entry.info.exists());
}

use basalgrim::{
    conflicts::{Choice, Decision},
    fs::{self, Operation},
    journal::{Database, storage::Retention},
    operations::Cancellation,
};
use rusqlite::{Connection, params};
use std::{
    fs as disk,
    path::{Path, PathBuf},
};

fn execute(db: &mut Database, operation: Operation, replace: bool) -> i64 {
    let mut recorder = db.begin(&operation, 1).unwrap();
    let report = fs::operate_with_journal(
        operation,
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| Decision {
            choice: if replace {
                Choice::Replace
            } else {
                Choice::KeepBoth
            },
            remaining: true,
        },
        &mut recorder,
    );
    assert!(report.error.is_none(), "{report:?}");
    report.journal.unwrap().id
}
fn copy(db: &mut Database, source: &Path, destination: &Path, replace: bool) -> i64 {
    execute(
        db,
        Operation::Transfer {
            sources: vec![source.into()],
            destination: destination.into(),
            cut: false,
        },
        replace,
    )
}
fn undo(db: &mut Database, id: i64) {
    let report = db.replay(id, false, 2, &Cancellation::default(), &mut |_| {});
    assert!(report.error.is_none(), "{report:?}");
}
fn held(dbpath: &Path, id: i64, field: &str) -> PathBuf {
    let data: Vec<u8> = Connection::open(dbpath)
        .unwrap()
        .query_row(
            "SELECT data FROM undo_items WHERE operation_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    let plan: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let mut node = &plan[field];
    if field == "backup" {
        node = &node["path"];
    }
    basalgrim::journal::path(&serde_json::from_value::<Vec<u8>>(node.clone()).unwrap())
}
fn prepare(root: &Path) -> (Database, PathBuf, PathBuf, PathBuf) {
    let path = root.join("journal");
    let source = root.join("source");
    let dest = root.join("dest");
    disk::create_dir_all(&dest).unwrap();
    disk::write(&source, "source contents").unwrap();
    (Database::open(&path).unwrap(), path, source, dest)
}
#[test]
fn cleanup_replaced_and_undone_data_keeps_live_files_and_audit_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, dest) = prepare(root.path());
    disk::write(dest.join("source"), "previous target").unwrap();
    let replacement = copy(&mut db, &source, &dest, true);
    let backup = held(&path, replacement, "backup");
    let folder = root.path().join("tree");
    disk::create_dir(&folder).unwrap();
    disk::write(folder.join("data"), "tree data").unwrap();
    std::os::unix::fs::symlink(&source, folder.join("link")).unwrap();
    let tree = copy(&mut db, &folder, &dest, false);
    undo(&mut db, tree);
    let parked = held(&path, tree, "parking");
    let catalog = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(catalog.protected, 0, "{:?}", catalog.issues);
    assert_eq!(catalog.candidates.len(), 2);
    assert_eq!(catalog.bytes, 15 + 9 + source.as_os_str().len() as u64);
    let report = db.cleanup_history(
        catalog.select(Retention::All),
        3,
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 2);
    assert!(!parked.exists() && !backup.exists());
    assert_eq!(disk::read_to_string(&source).unwrap(), "source contents");
    assert_eq!(
        disk::read_to_string(dest.join("source")).unwrap(),
        "source contents"
    );
    assert_eq!(
        disk::read_to_string(folder.join("data")).unwrap(),
        "tree data"
    );
    drop(db);
    let mut db = Database::open(&path).unwrap();
    assert_eq!(db.history_state(replacement).unwrap().undo, 0);
    assert_eq!(db.history_state(tree).unwrap().redo, 0);
    assert!(
        db.history_state(tree)
            .unwrap()
            .blocked
            .unwrap()
            .contains("очищены")
    );
    assert!(db.history().unwrap().iter().any(|r| r.id == replacement));
    assert_eq!(
        db.storage_catalog(&Cancellation::default()).unwrap().bytes,
        0
    );
}
#[test]
fn retention_uses_last_change_and_cleanup_of_zero_byte_plans_preserves_results() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, _, dest) = prepare(root.path());
    let create = execute(
        &mut db,
        Operation::Create {
            parent: dest.clone(),
            name: "empty".into(),
            directory: false,
        },
        false,
    );
    let first = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(first.select(Retention::Days7).entries.len(), 0);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE undo_items SET changed_ms=changed_ms-31*86400000 WHERE operation_id=?1",
            [create],
        )
        .unwrap();
    let catalog = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(catalog.select(Retention::Days90).entries.len(), 0);
    assert_eq!(catalog.select(Retention::Days30).entries.len(), 1);
    let report = db.cleanup_history(
        catalog.select(Retention::Days30),
        3,
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_none(), "{report:?}");
    assert!(dest.join("empty").exists());
    assert_eq!(db.history_state(create).unwrap().undo, 0);
}
#[test]
fn stale_confirmation_and_changed_or_symlinked_retention_are_never_deleted() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, dest) = prepare(root.path());
    let id = copy(&mut db, &source, &dest, false);
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    undo(&mut db, id);
    let parked = held(&path, id, "parking");
    let report = db.cleanup_history(selection, 3, &Cancellation::default(), &mut |_| {});
    assert!(report.error.is_some());
    assert!(parked.exists());
    assert_eq!(db.history_state(id).unwrap().redo, 1);
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    disk::write(&parked, "external edit").unwrap();
    let report = db.cleanup_history(selection, 4, &Cancellation::default(), &mut |_| {});
    assert!(report.error.is_some());
    assert_eq!(disk::read_to_string(&parked).unwrap(), "external edit");
    let catalog = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(catalog.protected, 1);
    assert!(catalog.candidates.is_empty());
    disk::remove_file(&parked).unwrap();
    std::os::unix::fs::symlink(&source, &parked).unwrap();
    assert!(
        db.storage_catalog(&Cancellation::default())
            .unwrap()
            .candidates
            .is_empty()
    );
    assert_eq!(disk::read_to_string(source).unwrap(), "source contents");
}
#[test]
fn cancellation_stops_between_records_and_finishes_the_current_tree() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, dest) = prepare(root.path());
    let first = copy(&mut db, &source, &dest, false);
    undo(&mut db, first);
    let second = copy(&mut db, &source, &dest, false);
    undo(&mut db, second);
    let a = held(&path, first, "parking");
    let b = held(&path, second, "parking");
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let cancel = Cancellation::default();
    let report = db.cleanup_history(selection, 3, &cancel, &mut |p| {
        if p.entries_done > 0 {
            cancel.cancel();
        }
    });
    assert!(report.cancelled, "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert!(!a.exists() && b.exists());
    assert_eq!(db.history_state(second).unwrap().redo, 1);
}
#[test]
fn sql_failures_before_and_after_removal_preserve_exact_completed_counts() {
    for after in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut db, path, source, dest) = prepare(root.path());
        let id = copy(&mut db, &source, &dest, false);
        undo(&mut db, id);
        let parked = held(&path, id, "parking");
        let selection = db
            .storage_catalog(&Cancellation::default())
            .unwrap()
            .select(Retention::All);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(if after {"CREATE TRIGGER fault BEFORE UPDATE ON steps WHEN OLD.kind='history_delete' BEGIN SELECT RAISE(FAIL,'injected after'); END;"}
            else {"CREATE TRIGGER fault BEFORE INSERT ON steps WHEN NEW.kind='history_delete' BEGIN SELECT RAISE(FAIL,'injected before'); END;"}).unwrap();
        let report = db.cleanup_history(selection, 3, &Cancellation::default(), &mut |_| {});
        assert!(report.error.is_some(), "{report:?}");
        assert_eq!(report.completed.len(), usize::from(after));
        assert_eq!(parked.exists(), !after);
        assert!(source.exists());
        if !after {
            assert_eq!(db.history_state(id).unwrap().redo, 1);
        }
    }
}
#[test]
fn live_other_window_and_interrupted_cleanup_keep_data_and_never_replay() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, dest) = prepare(root.path());
    let id = copy(&mut db, &source, &dest, false);
    undo(&mut db, id);
    let parked = held(&path, id, "parking");
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let mut other = Database::open(&path).unwrap();
    let pending = other
        .begin(
            &Operation::Create {
                parent: dest,
                name: "pending".into(),
                directory: false,
            },
            99,
        )
        .unwrap();
    let report = db.cleanup_history(selection, 3, &Cancellation::default(), &mut |_| {});
    assert!(report.error.is_some(), "{report:?}");
    assert!(parked.exists());
    assert_eq!(db.history_state(id).unwrap().redo, 1);
    drop(pending);
    drop(other);
    let connection = Connection::open(&path).unwrap();
    let run: i64 = connection
        .query_row("SELECT MAX(id) FROM operations", [], |r| r.get(0))
        .unwrap();
    connection
        .execute(
            "UPDATE undo_items SET state='discarding',last_run=?2 WHERE operation_id=?1",
            params![id, run],
        )
        .unwrap();
    drop(db);
    let mut db = Database::open(&path).unwrap();
    let catalog = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(catalog.protected, 1);
    assert!(parked.exists());
    assert_eq!(db.history_state(id).unwrap().redo, 0);
    assert!(catalog.candidates.is_empty());
}

#[test]
fn compact_mouse_confirmation_policy_and_restart_share_the_same_worker() {
    use basalgrim::{
        app::{App, Dialog, Target},
        input::{Action, Input},
        settings::Settings,
        ui,
    };
    use ratatui::{Terminal, backend::TestBackend};
    fn settle(app: &mut App) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(6);
        while app.loading() || app.journal_loading {
            app.tick();
            assert!(std::time::Instant::now() < end, "{}", app.notice);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    fn click(app: &mut App, which: usize) {
        Terminal::new(TestBackend::new(34, 14))
            .unwrap()
            .draw(|f| ui::render(app, f))
            .unwrap();
        let rect = app
            .hits
            .iter()
            .find_map(|(r, t)| match (which, t) {
                (0, Target::Action(Action::HistoryStorage))
                | (1, Target::StoragePolicy)
                | (2, Target::StorageClear)
                | (3, Target::Confirm)
                | (4, Target::Cancel) => Some(*r),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing mouse target {which}"));
        app.handle(Input::Click {
            x: rect.x,
            y: rect.y,
            double: false,
            toggle: false,
        });
        settle(app);
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("journal");
    let mut app = App::new(root.path().into(), Settings::default());
    app.attach_journal(path.clone());
    settle(&mut app);
    app.start_operation(Operation::Create {
        parent: root.path().into(),
        name: "keep".into(),
        directory: false,
    });
    settle(&mut app);
    app.action(Action::Undo);
    settle(&mut app);
    app.action(Action::Operations);
    settle(&mut app);
    click(&mut app, 0);
    assert!(matches!(app.dialog, Some(Dialog::HistoryStorage)));
    assert_eq!(app.storage_selection().entries.len(), 1);
    click(&mut app, 1);
    assert_eq!(app.settings.history_retention, Retention::Days7);
    assert!(app.storage_selection().entries.is_empty());
    for _ in 0..3 {
        click(&mut app, 1);
    }
    click(&mut app, 2);
    assert!(matches!(app.dialog, Some(Dialog::CleanupHistory(_))));
    click(&mut app, 4);
    assert!(app.storage_selection().entries.len() == 1);
    click(&mut app, 2);
    click(&mut app, 3);
    assert!(matches!(app.dialog, Some(Dialog::Operations)));
    assert!(
        app.operations.front().unwrap().error.is_none(),
        "{}",
        app.notice
    );
    app.cancel_dialog();
    assert!(app.replay_candidate(true).is_none());
    assert!(!root.path().join("keep").exists());
    drop(app);
    let mut reopened = App::new(root.path().into(), Settings::default());
    reopened.attach_journal(path);
    settle(&mut reopened);
    assert!(reopened.replay_candidate(true).is_none());
}

#[test]
fn persisted_policy_defaults_and_new_binding_preserve_existing_shortcuts() {
    use basalgrim::{
        input::{Action, Key, KeyPress},
        settings::Settings,
    };
    let mut settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.history_retention, Retention::All);
    settings.history_retention = Retention::Days30;
    let roundtrip: Settings =
        serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
    assert_eq!(roundtrip.history_retention, Retention::Days30);
    let key = KeyPress {
        ctrl: true,
        shift: true,
        ..KeyPress::plain(Key::F(6))
    };
    settings
        .bindings
        .retain(|b| b.action != Action::HistoryStorage);
    settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Copy)
        .unwrap()
        .key = key;
    settings.normalize().unwrap();
    assert_eq!(settings.action_for(key), Some(Action::Copy));
    assert_ne!(settings.shortcut(Action::HistoryStorage), key.label());
}

#[test]
fn raw_names_and_directory_changes_never_follow_a_replaced_storage_parent() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, dest) = prepare(root.path());
    let tree = root.path().join("tree");
    disk::create_dir(&tree).unwrap();
    let raw = std::ffi::OsString::from_vec(b"raw-\xff".to_vec());
    disk::write(tree.join(&raw), "raw file").unwrap();
    let id = copy(&mut db, &tree, &dest, false);
    undo(&mut db, id);
    let parked = held(&path, id, "parking");
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(
        db.cleanup_history(selection.clone(), 3, &cancelled, &mut |_| {})
            .cancelled
    );
    assert_eq!(db.history_state(id).unwrap().redo, 1);
    disk::write(parked.join("arrival"), "new data").unwrap();
    assert!(
        db.cleanup_history(selection, 4, &Cancellation::default(), &mut |_| {})
            .error
            .is_some()
    );
    assert_eq!(
        disk::read_to_string(parked.join("arrival")).unwrap(),
        "new data"
    );
    assert!(parked.join(&raw).exists());
    disk::remove_file(parked.join("arrival")).unwrap();
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let parent = parked.parent().unwrap();
    let relocated = root.path().join("relocated");
    disk::rename(parent, &relocated).unwrap();
    std::os::unix::fs::symlink(&relocated, parent).unwrap();
    assert!(
        db.cleanup_history(selection, 5, &Cancellation::default(), &mut |_| {})
            .error
            .is_some()
    );
    assert!(relocated.join("item").join(raw).exists());
    assert!(source.exists());
}

#[test]
fn cleanup_of_restore_metadata_preserves_restored_data_and_other_trash_items() {
    use basalgrim::trash::{self, Root};
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, source, _) = prepare(root.path());
    let trash = Root::home(root.path().join("data"));
    for dir in [
        &trash.directory,
        &trash.directory.join("files"),
        &trash.directory.join("info"),
    ] {
        disk::create_dir_all(dir).unwrap();
        disk::set_permissions(dir, disk::Permissions::from_mode(0o700)).unwrap();
    }
    let original = root.path().join("restored");
    disk::write(trash.directory.join("files/item"), "restore content").unwrap();
    disk::write(
        trash.directory.join("info/item.trashinfo"),
        format!(
            "[Trash Info]\nPath={}\nDeletionDate=2026-11-01T12:30:45\n",
            original.display()
        ),
    )
    .unwrap();
    disk::write(trash.directory.join("files/other"), "keep trash").unwrap();
    let entry = trash::read(std::slice::from_ref(&trash), &Cancellation::default())
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.data.ends_with("item"))
        .unwrap();
    let id = execute(
        &mut db,
        Operation::RestoreTrash {
            entries: vec![entry],
            destination: None,
        },
        false,
    );
    let parked = held(&path, id, "info_parking");
    assert!(parked.exists());
    let catalog = db.storage_catalog(&Cancellation::default()).unwrap();
    assert_eq!(catalog.protected, 0, "{:?}", catalog.issues);
    assert_eq!(catalog.bytes, parked.metadata().unwrap().len());
    let report = db.cleanup_history(
        catalog.select(Retention::All),
        3,
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_none(), "{report:?}");
    assert!(!parked.exists());
    assert_eq!(disk::read_to_string(original).unwrap(), "restore content");
    assert_eq!(
        disk::read_to_string(trash.directory.join("files/other")).unwrap(),
        "keep trash"
    );
    assert!(source.exists());
}

#[test]
fn changed_file_during_tree_disposal_is_retained_even_if_mtime_is_restored() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, _, dest) = prepare(root.path());
    let tree = root.path().join("tree");
    disk::create_dir(&tree).unwrap();
    for name in ["a", "b"] {
        disk::write(tree.join(name), "original").unwrap();
    }
    let id = copy(&mut db, &tree, &dest, false);
    undo(&mut db, id);
    let parked = held(&path, id, "parking");
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let mut edited = None;
    let report = db.cleanup_history(selection, 3, &Cancellation::default(), &mut |progress| {
        if progress.entries_done == 1 && edited.is_none() {
            let file = disk::read_dir(&parked)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let time = file.metadata().unwrap().modified().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1));
            disk::write(&file, "modified").unwrap();
            disk::File::open(&file)
                .unwrap()
                .set_times(disk::FileTimes::new().set_modified(time))
                .unwrap();
            edited = Some(file);
        }
    });
    assert!(report.error.is_some(), "{report:?}");
    assert!(report.completed.is_empty());
    assert_eq!(disk::read_to_string(edited.unwrap()).unwrap(), "modified");
    assert_eq!(db.history_state(id).unwrap().redo, 0);
    assert_eq!(
        db.storage_catalog(&Cancellation::default())
            .unwrap()
            .protected,
        1
    );
}

#[test]
fn removing_our_own_hard_links_authorizes_only_the_resulting_ctime_changes() {
    let root = tempfile::tempdir().unwrap();
    let (mut db, path, _, dest) = prepare(root.path());
    let tree = root.path().join("tree");
    disk::create_dir(&tree).unwrap();
    disk::write(tree.join("new"), "new data").unwrap();
    let old = dest.join("tree");
    disk::create_dir(&old).unwrap();
    disk::write(old.join("a"), "old data").unwrap();
    disk::hard_link(old.join("a"), old.join("b")).unwrap();
    let id = copy(&mut db, &tree, &dest, true);
    let backup = held(&path, id, "backup");
    let selection = db
        .storage_catalog(&Cancellation::default())
        .unwrap()
        .select(Retention::All);
    let report = db.cleanup_history(selection, 3, &Cancellation::default(), &mut |_| {});
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert!(!backup.exists());
    assert_eq!(disk::read_to_string(old.join("new")).unwrap(), "new data");
}

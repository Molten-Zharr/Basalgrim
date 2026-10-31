use basalgrim::{
    app::{App, Clipboard, Dialog, Target},
    fs::Operation,
    input::{Action, Input},
    journal::Database,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        app.tick();
        if predicate(app) {
            return;
        }
        assert!(Instant::now() < end, "journal timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn ready(app: &mut App) {
    wait(app, |a| !a.loading() && !a.journal_loading);
}
fn fixture() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let db = root.path().join("state/operations.sqlite3");
    let mut app = App::new(
        files.clone(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.attach_journal(db.clone());
    ready(&mut app);
    (root, app, files, db)
}
fn create(app: &mut App, parent: &Path, name: &str) {
    app.start_operation(Operation::Create {
        parent: parent.into(),
        name: name.into(),
        directory: false,
    });
    ready(app);
}

#[test]
fn restarted_history_is_loaded_and_shared_mouse_refresh_updates_it_without_duplicates() {
    let (_root, mut app, files, dbpath) = fixture();
    create(&mut app, &files, "one");
    create(&mut app, &files, "two");
    let ids: Vec<_> = app
        .operations
        .iter()
        .map(|r| r.journal.as_ref().unwrap().id)
        .collect();
    drop(app);
    let mut app = App::new(
        files.clone(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.attach_journal(dbpath.clone());
    ready(&mut app);
    assert_eq!(app.operations.len(), 2);
    assert!(app.operations.iter().all(|r| r.from_journal));
    assert_eq!(app.operations[0].journal.as_ref().unwrap().id, ids[0]);
    create(&mut app, &files, "three");
    app.action(Action::Operations);
    ready(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let row = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::JournalRefresh).then_some(*r))
        .unwrap();
    app.handle(Input::Click {
        x: row.x,
        y: row.y,
        double: false,
        toggle: false,
    });
    ready(&mut app);
    assert!(matches!(app.dialog, Some(Dialog::Operations)));
    assert_eq!(app.operations.len(), 3);
    assert_eq!(app.operation_count, 1);
    let mut db = Database::open(&dbpath).unwrap();
    assert_eq!(db.history().unwrap().len(), 3);
}

#[test]
fn bad_journal_blocks_mutation_preserves_the_file_and_can_be_retried_from_the_same_screen() {
    let (_root, mut app, files, dbpath) = fixture();
    drop(app);
    fs::write(&dbpath, "broken").unwrap();
    app = App::new(
        files.clone(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.attach_journal(dbpath.clone());
    ready(&mut app);
    assert!(app.journal_error.is_some());
    create(&mut app, &files, "blocked");
    assert!(!files.join("blocked").exists());
    assert!(app.operations[0].error.as_ref().unwrap().contains("Журнал"));
    assert_eq!(fs::read_to_string(&dbpath).unwrap(), "broken");
    fs::remove_file(&dbpath).unwrap();
    app.action(Action::Operations);
    ready(&mut app);
    assert!(app.journal_error.is_none());
    app.cancel_dialog();
    create(&mut app, &files, "works");
    assert!(files.join("works").exists());
}

#[test]
fn post_move_journal_failure_removes_only_actually_moved_cut_paths() {
    let (_root, mut app, files, dbpath) = fixture();
    let dest = files.join("dest");
    fs::create_dir(&dest).unwrap();
    let a = files.join("a");
    let b = files.join("b");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    app.clipboard = Some(Clipboard {
        paths: vec![a.clone(), b.clone()],
        cut: true,
    });
    rusqlite::Connection::open(&dbpath).unwrap().execute_batch("CREATE TRIGGER fail_after BEFORE UPDATE OF state ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(FAIL,'after mutation'); END;").unwrap();
    app.navigate(dest.clone(), true);
    ready(&mut app);
    app.action(Action::Paste);
    ready(&mut app);
    assert!(!a.exists() && b.exists() && dest.join("a").exists());
    assert_eq!(app.clipboard.as_ref().unwrap().paths, [b]);
    assert_eq!(app.operations[0].progress.items_done, 1);
    assert!(app.operations[0].error.is_some());
}

#[test]
fn switching_journal_requests_rejects_stale_history_and_missing_state_storage_is_explicit() {
    let (_root, mut app, files, dbpath) = fixture();
    create(&mut app, &files, "a");
    let other = dbpath.with_file_name("other.sqlite3");
    app.attach_journal(dbpath);
    app.attach_journal(other);
    ready(&mut app);
    assert!(app.operations.iter().all(|r| !r.from_journal));
    app.configure_journal(None);
    ready(&mut app);
    assert!(app.journal_error.is_some());
    let mut fresh = App::new(files.clone(), Settings::default());
    fresh.configure_journal(None);
    ready(&mut fresh);
    assert!(fresh.journal_error.is_some());
    create(&mut fresh, &files, "blocked");
    assert!(!files.join("blocked").exists());
}

#[test]
fn dropping_a_standalone_worker_cancels_an_unanswered_conflict_and_drains_its_queue() {
    use basalgrim::{
        operations::Cancellation,
        worker::{Event, Worker},
    };
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let dest = root.path().join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(&source, "incoming").unwrap();
    fs::write(dest.join("source"), "existing").unwrap();
    let worker = Worker::new();
    let cancellation = Cancellation::default();
    assert!(worker.operate(
        1,
        Operation::Transfer {
            sources: vec![source.clone()],
            destination: dest.clone(),
            cut: false
        },
        cancellation.clone()
    ));
    let end = Instant::now() + Duration::from_secs(5);
    let reply = loop {
        assert!(Instant::now() < end);
        if let Ok(Event::Conflict { reply, .. }) =
            worker.events.recv_timeout(Duration::from_millis(10))
        {
            break reply;
        }
    };
    let (done, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        drop(worker);
        done.send(()).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        cancellation.cancel();
    }
    thread.join().unwrap();
    assert!(result.is_ok());
    drop(reply);
    assert_eq!(fs::read_to_string(source).unwrap(), "incoming");
    assert_eq!(fs::read_to_string(dest.join("source")).unwrap(), "existing");
}

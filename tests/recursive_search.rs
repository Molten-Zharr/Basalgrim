use basalgrim::{
    app::{App, Dialog, Target},
    input::{Action, Input, Key, KeyPress},
    operations::Cancellation,
    search,
    settings::Settings,
    ui,
};
use std::{
    fs,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if !app.loading() {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn scan_matches_names_and_does_not_follow_directory_symlinks() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/NEEDLE.txt"), "data").unwrap();
    fs::create_dir(root.path().join(".hidden")).unwrap();
    fs::write(root.path().join(".hidden/needle"), "hidden").unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("sub/cycle")).unwrap();
    let r = search::scan(
        root.path(),
        "needle",
        false,
        &Cancellation::default(),
        |_| {},
    );
    assert_eq!(r.paths.len(), 1);
    assert_eq!(r.status(), "Завершён");
    let r = search::scan(
        root.path(),
        "needle",
        true,
        &Cancellation::default(),
        |_| {},
    );
    assert_eq!(r.paths.len(), 2);
    let c = Cancellation::default();
    c.cancel();
    assert_eq!(search::scan(root.path(), "", true, &c, |_| {}).scanned, 0);
}
#[test]
fn recursive_dialog_mouse_navigation_focus_cancellation_and_stale_results() {
    for (w, h) in [(34, 14), (150, 44)] {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/needle.txt"), "data").unwrap();
        let mut app = App::new(root.path().to_path_buf(), Settings::default());
        settle(&mut app);
        app.action(Action::Search);
        app.handle(Input::Key(KeyPress::plain(Key::F(3))));
        app.handle(Input::Text("needle".into()));
        settle(&mut app);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        let (area, _) = app
            .hits
            .iter()
            .find(|(_, t)| matches!(t, Target::SearchOpen(_, 0)))
            .unwrap();
        let (x, y) = (area.x, area.y);
        app.handle(Input::Click {
            x,
            y,
            double: false,
            toggle: false,
        });
        settle(&mut app);
        assert!(app.dialog.is_none());
        assert_eq!(app.tab().cwd, root.path().join("sub"));
        assert_eq!(
            app.tab().entry().unwrap().path,
            root.path().join("sub/needle.txt")
        );
        app.action(Action::Search);
        app.begin_recursive_search();
        app.handle(Input::Text("needle".into()));
        app.handle(Input::Key(KeyPress::ctrl('a')));
        app.handle(Input::Text("absent".into()));
        settle(&mut app);
        assert!(app.search.as_ref().unwrap().worker.results.paths.is_empty());
        app.cancel_dialog();
        assert!(app.search.is_none());
        assert!(!matches!(app.dialog, Some(Dialog::Edit { .. })));
    }
}

fn wait_search(search: &mut search::Search, ready: impl Fn(&search::Results) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        search.poll();
        if ready(&search.results) {
            assert!(
                search.results.failure.is_none(),
                "{:?}",
                search.results.failure
            );
            return;
        }
        assert!(Instant::now() < deadline, "search/page timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn finds_a_file_beyond_250001_entries() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("tree");
    fs::create_dir(&root).unwrap();
    // Small groups of hard links avoid both file data and per-inode link limits.
    let mut seed = fixture.path().join("seed");
    for index in 0..250_010 {
        if index % 10_000 == 0 {
            seed = fixture.path().join(format!("seed-{index}"));
            fs::write(&seed, b"").unwrap();
        }
        fs::hard_link(&seed, root.join(format!("entry-{index:06}"))).unwrap();
    }
    // Use actual directory order, not an assumption about alphabetical traversal.
    let inventory: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(inventory.len(), 250_010);
    let target = inventory.last().unwrap();
    let query = target.file_name().unwrap().to_str().unwrap();
    let result = search::scan(&root, query, true, &Cancellation::default(), |_| {});
    assert_eq!(result.scanned, inventory.len());
    assert_eq!(result.total, 1);
    assert_eq!(result.paths.as_slice(), std::slice::from_ref(target));
    assert_eq!(result.status(), "Завершён");
}

#[test]
fn pages_all_matches_beyond_2000_and_eight_mib_of_paths() {
    use std::collections::HashSet;
    use std::os::unix::ffi::OsStringExt;
    let fixture = tempfile::tempdir().unwrap();
    let mut leaf = fixture.path().to_path_buf();
    for _ in 0..30 {
        leaf.push("d".repeat(120));
        fs::create_dir(&leaf).unwrap();
    }
    let mut expected = HashSet::new();
    for index in 0..2407 {
        // Include names that cannot roundtrip through Unicode.
        let name = std::ffi::OsString::from_vec(
            format!("needle-{index:04}-")
                .bytes()
                .chain([0xff])
                .collect(),
        );
        let path = leaf.join(name);
        fs::write(&path, b"").unwrap();
        expected.insert(path);
    }
    assert!(expected.iter().map(|p| p.as_os_str().len()).sum::<usize>() > 8 * 1024 * 1024);
    let mut worker = search::Search::default();
    worker.start(fixture.path().to_path_buf(), "NEEDLE".into(), true);
    wait_search(&mut worker, |r| r.done);
    assert_eq!(worker.results.total, expected.len());
    let mut found = HashSet::new();
    for offset in (0..expected.len()).step_by(search::PAGE_SIZE) {
        worker.request_page(offset);
        wait_search(&mut worker, |r| r.offset == offset);
        assert!(worker.results.paths.len() <= search::PAGE_SIZE);
        for (local, path) in worker.results.paths.iter().enumerate() {
            assert_eq!(worker.results.path(offset + local), Some(path));
            assert!(found.insert(path.clone()), "duplicate result page");
        }
    }
    assert_eq!(found, expected);
    worker.request_page(0);
    wait_search(&mut worker, |r| r.offset == 0);
    assert_eq!(worker.results.paths.len(), search::PAGE_SIZE);
}

#[test]
fn deep_directories_errors_stop_and_obsolete_queries() {
    let root = tempfile::tempdir().unwrap();
    let mut leaf = root.path().to_path_buf();
    for _ in 0..96 {
        leaf.push("d");
        fs::create_dir(&leaf).unwrap();
    }
    let target = leaf.join("needle");
    fs::write(&target, b"").unwrap();
    let result = search::scan(
        root.path(),
        "needle",
        true,
        &Cancellation::default(),
        |_| {},
    );
    assert_eq!(result.paths, [target]);
    assert_eq!(result.scanned, 97);
    let result = search::scan(
        &root.path().join("missing"),
        "needle",
        true,
        &Cancellation::default(),
        |_| {},
    );
    assert_eq!(result.status(), "С пропусками");
    assert_eq!(result.errors, 1);
    for i in 0..5000 {
        fs::write(root.path().join(format!("match-{i}")), b"").unwrap();
    }
    let cancel = Cancellation::default();
    let result = search::scan(root.path(), "match", true, &cancel, |r| {
        if r.scanned >= 1024 {
            cancel.cancel();
        }
    });
    assert_eq!(result.status(), "Остановлен");
    assert!(result.scanned < 5000);
    let mut worker = search::Search::default();
    worker.start(root.path().to_path_buf(), "match".into(), true);
    wait_search(&mut worker, |r| r.total >= 128);
    worker.stop();
    worker.request_page(32);
    wait_search(&mut worker, |r| r.offset == 32);
    assert_eq!(worker.results.status(), "Остановлен");
    assert!(!worker.results.paths.is_empty());
    worker.start(root.path().to_path_buf(), "absent".into(), true);
    wait_search(&mut worker, |r| r.done);
    assert_eq!(worker.results.total, 0);
    assert_eq!(worker.results.status(), "Завершён");
    worker.start(root.path().to_path_buf(), "match".into(), true);
    worker.start(root.path().to_path_buf(), "".into(), true);
    std::thread::sleep(Duration::from_millis(30));
    worker.poll();
    assert_eq!(worker.results.total, 0);
    assert!(worker.results.done && !worker.results.stopped);
    worker.start(root.path().to_path_buf(), "x".repeat(4097), true);
    std::thread::sleep(Duration::from_millis(30));
    worker.poll();
    assert_eq!(worker.results.status(), "Прерван");
}

#[test]
fn compact_and_large_dialog_open_result_after_2000() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("sub");
    fs::create_dir(&sub).unwrap();
    for i in 0..2105 {
        fs::write(sub.join(format!("needle-{i:04}")), b"").unwrap();
    }
    for (width, height) in [(34, 14), (150, 44)] {
        let mut app = App::new(root.path().to_path_buf(), Settings::default());
        settle(&mut app);
        app.begin_recursive_search();
        app.handle(Input::Text("needle".into()));
        settle(&mut app);
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        let &(area, _) = app
            .hits
            .iter()
            .find(|(_, target)| matches!(target, Target::SearchMove(isize::MAX)))
            .unwrap();
        app.handle(Input::Click {
            x: area.x,
            y: area.y,
            double: false,
            toggle: false,
        });
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        let worker = &mut app.search.as_mut().unwrap().worker;
        wait_search(worker, |r| r.path(2104).is_some());
        let expected = worker.results.path(2104).unwrap().clone();
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        let &(area, _) = app
            .hits
            .iter()
            .find(|(_, target)| matches!(target, Target::SearchOpen(_, 2104)))
            .unwrap();
        app.handle(Input::Click {
            x: area.x,
            y: area.y,
            double: false,
            toggle: false,
        });
        settle(&mut app);
        assert!(app.dialog.is_none());
        assert_eq!(app.tab().entry().unwrap().path, expected);
    }
}

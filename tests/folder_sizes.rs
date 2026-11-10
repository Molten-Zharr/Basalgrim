use basalgrim::{
    app::{App, Target},
    folder_size::{FolderSizes, measure},
    input::{Action, Input},
    operations::Cancellation,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, symlink},
    },
    time::{Duration, Instant},
};

fn wait(worker: &mut FolderSizes) {
    let until = Instant::now() + Duration::from_secs(10);
    while !worker.result.done {
        worker.poll();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn settle(app: &mut App) {
    let until = Instant::now() + Duration::from_secs(10);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn hit(app: &App, p: impl Fn(&Target) -> bool) -> Rect {
    app.hits.iter().find(|(_, t)| p(t)).unwrap().0
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}

#[test]
fn sizes_include_hidden_sparse_and_raw_paths_without_following_links_or_counting_hardlinks_twice() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("excluded"), [1; 10000]).unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    fs::write(root.path().join("nested/data"), [1; 123]).unwrap();
    fs::write(root.path().join(".hidden"), [1; 17]).unwrap();
    fs::write(
        root.path()
            .join(std::ffi::OsString::from_vec(b"raw\xff".to_vec())),
        [1; 9],
    )
    .unwrap();
    fs::hard_link(root.path().join("nested/data"), root.path().join("alias")).unwrap();
    let sparse = root.path().join("sparse");
    fs::File::create(&sparse).unwrap().set_len(1 << 30).unwrap();
    symlink(outside.path(), root.path().join("outside")).unwrap();
    symlink(".", root.path().join("cycle")).unwrap();
    symlink("missing", root.path().join("dangling")).unwrap();
    let result = measure(root.path(), &Cancellation::default(), |_| {});
    assert!(result.complete(), "{result:?}");
    assert_eq!(result.bytes, (1 << 30) + 123 + 17 + 9);
    assert_eq!(result.files, 5);
    assert_eq!(result.links, 3);
    assert_eq!(result.folders, 1);
    assert_eq!(result.duplicates, 1);
    let disk = [
        root.path().to_path_buf(),
        root.path().join("nested"),
        root.path().join("nested/data"),
        root.path().join(".hidden"),
        root.path()
            .join(std::ffi::OsString::from_vec(b"raw\xff".to_vec())),
        sparse,
        root.path().join("outside"),
        root.path().join("cycle"),
        root.path().join("dangling"),
    ]
    .iter()
    .map(|p| fs::symlink_metadata(p).unwrap().blocks() * 512)
    .sum::<u64>();
    assert_eq!(result.allocated, disk);
    assert!(result.allocated < result.bytes);
}

#[test]
fn roots_must_be_real_directories_and_cancellation_is_partial() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file"), b"abc").unwrap();
    symlink(root.path(), root.path().join("link")).unwrap();
    for name in ["file", "link", "missing"] {
        let r = measure(&root.path().join(name), &Cancellation::default(), |_| {});
        assert!(!r.complete());
        assert!(r.failure.is_some());
    }
    let cancel = Cancellation::default();
    cancel.cancel();
    let r = measure(root.path(), &cancel, |_| {});
    assert!(r.stopped);
    assert!(!r.complete());
}

#[test]
fn all_entries_and_deep_subdirectories_are_counted_without_old_search_limits() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("seed");
    fs::write(&source, b"size").unwrap();
    // Cheap hard links exercise the full traversal/identity set beyond 250001.
    for group in 0..251 {
        let dir = root.path().join(format!("group{group}"));
        fs::create_dir(&dir).unwrap();
        for i in 0..1000 {
            if i == 0 {
                fs::write(dir.join("0"), b"size").unwrap();
            } else {
                fs::hard_link(dir.join("0"), dir.join(i.to_string())).unwrap();
            }
        }
    }
    let mut deep = root.path().to_path_buf();
    for _ in 0..96 {
        deep.push("deep");
        fs::create_dir(&deep).unwrap();
    }
    fs::write(deep.join("last"), b"deep").unwrap();
    let result = measure(root.path(), &Cancellation::default(), |_| {});
    assert!(result.complete(), "{result:?}");
    assert_eq!(result.files, 251_002);
    assert_eq!(result.bytes, 1012);
    assert_eq!(result.folders, 347);
    assert_eq!(result.duplicates, 250_749);
}

#[test]
fn cached_results_are_reused_repeatedly_but_force_change_and_invalidation_recompute() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), [0; 17]).unwrap();
    let mut sizes = FolderSizes::default();
    sizes.start(root.path().into(), false);
    wait(&mut sizes);
    assert!(sizes.result.complete());
    assert!(!sizes.result.cached);
    let measured = sizes.result.measured;
    for _ in 0..2 {
        sizes.start(root.path().into(), false);
        wait(&mut sizes);
        assert!(sizes.result.cached);
        assert_eq!(sizes.result.measured, measured);
        assert!(sizes.cached(root.path()).is_some());
    }
    sizes.start(root.path().into(), true);
    wait(&mut sizes);
    assert!(!sizes.result.cached);
    fs::write(root.path().join("new"), [0; 33]).unwrap();
    sizes.start(root.path().into(), false);
    wait(&mut sizes);
    assert!(!sizes.result.cached);
    assert_eq!(sizes.result.bytes, 50);
    fs::write(root.path().join("a"), [0; 70]).unwrap();
    sizes.invalidate(&root.path().join("a"));
    assert!(sizes.cached(root.path()).is_none());
    sizes.start(root.path().into(), false);
    wait(&mut sizes);
    assert_eq!(sizes.result.bytes, 103);
    assert!(!sizes.result.cached);
    fs::remove_file(root.path().join("a")).unwrap();
    fs::remove_file(root.path().join("new")).unwrap();
    fs::remove_dir(root.path()).unwrap();
    sizes.start(root.path().into(), false);
    wait(&mut sizes);
    assert!(sizes.result.failure.is_some());
    assert!(sizes.cached(root.path()).is_none());
}

#[test]
fn obsolete_stopped_or_invalidated_results_cannot_seed_the_cache() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::write(first.path().join("a"), [0; 17]).unwrap();
    fs::write(second.path().join("b"), [0; 33]).unwrap();
    let mut sizes = FolderSizes::default();
    sizes.start(first.path().into(), false);
    sizes.start(second.path().into(), false);
    wait(&mut sizes);
    assert_eq!(sizes.result.bytes, 33);
    assert!(sizes.cached(first.path()).is_none());
    sizes.start(first.path().into(), true);
    sizes.stop();
    for _ in 0..30 {
        sizes.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(sizes.result.stopped);
    assert!(sizes.cached(first.path()).is_none());
    sizes.start(first.path().into(), true);
    sizes.invalidate(first.path());
    wait(&mut sizes);
    assert!(sizes.result.stale);
    assert!(sizes.cached(first.path()).is_none());
}

#[test]
fn mouse_contexts_preview_cache_and_compact_stop_retry_close_share_the_captured_folder() {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("folder");
    fs::create_dir(&child).unwrap();
    fs::write(child.join("data"), [0; 1234]).unwrap();
    fs::write(root.path().join("outside"), [0; 55]).unwrap();
    for (w, h) in [(150, 40), (34, 14)] {
        for grid in [false, true] {
            let mut app = App::new(
                root.path().into(),
                Settings {
                    grid,
                    ..Default::default()
                },
            );
            settle(&mut app);
            render(&mut app, w, h);
            let entry = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
            app.handle(Input::SecondaryClick {
                x: entry.x,
                y: entry.y,
            });
            render(&mut app, w, h);
            let index = app
                .command_items()
                .iter()
                .position(|i| i.action == Action::FolderSize)
                .unwrap();
            // Folder command remains reachable even if a compact popup needs scrolling.
            while !app
                .hits
                .iter()
                .any(|(_, t)| matches!(t,Target::Command(i) if *i==index))
            {
                app.handle(Input::Scroll {
                    x: 1,
                    y: 1,
                    delta: 1,
                });
                render(&mut app, w, h);
            }
            let r = hit(&app, |t| matches!(t,Target::Command(i) if *i==index));
            click(&mut app, r);
            render(&mut app, w, h);
            let stop = hit(&app, |t| matches!(t, Target::FolderStop(_)));
            click(&mut app, stop);
            assert!(app.folder_sizes.result.stopped);
            render(&mut app, w, h);
            let retry = hit(&app, |t| matches!(t, Target::FolderRetry(_)));
            click(&mut app, retry);
            settle(&mut app);
            assert_eq!(app.folder_sizes.path.as_ref(), Some(&child));
            assert_eq!(app.folder_sizes.result.bytes, 1234);
            let screen = render(&mut app, w, h);
            assert!(screen.contains("Подсчёт завершён"));
            let close = hit(&app, |t| matches!(t, Target::Cancel));
            click(&mut app, close);
            assert!(app.dialog.is_none());
            if w == 150 {
                let text = render(&mut app, w, h);
                assert!(text.contains("Содержимое ~"));
                let button = hit(&app, |t| matches!(t, Target::FolderMeasure { .. }));
                click(&mut app, button);
                settle(&mut app);
                assert!(app.folder_sizes.result.cached);
                app.cancel_dialog();
            }
            render(&mut app, w, h);
            let file_area = app.file_areas[app.active];
            app.handle(Input::SecondaryClick {
                x: file_area.right() - 1,
                y: file_area.bottom() - 1,
            });
            render(&mut app, w, h);
            let index = app
                .command_items()
                .iter()
                .position(|i| i.action == Action::FolderSize)
                .unwrap();
            let button = hit(&app, |t| matches!(t,Target::Command(i) if *i==index));
            click(&mut app, button);
            settle(&mut app);
            assert_eq!(app.folder_sizes.path.as_deref(), Some(root.path()));
            assert_eq!(app.folder_sizes.result.bytes, 1289);
        }
    }
}

#[test]
fn settings_migration_preserves_custom_bindings_and_adds_folder_size() {
    let mut settings = Settings::default();
    settings.bindings.retain(|b| b.action != Action::FolderSize);
    let original = settings.bindings.clone();
    settings.normalize().unwrap();
    assert!(
        settings
            .bindings
            .iter()
            .any(|b| b.action == Action::FolderSize)
    );
    assert!(original.iter().all(|b| settings.bindings.contains(b)));
}

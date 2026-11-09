use basalgrim::{
    batch_rename::{self, Plan},
    fs::{self, Operation},
    journal::Database,
    operations::Cancellation,
};
use std::{
    fs as disk,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn files(root: &Path) -> Vec<PathBuf> {
    ["a.txt", "b.txt", "c.txt"]
        .into_iter()
        .map(|n| {
            let p = root.join(n);
            disk::write(&p, n).unwrap();
            p
        })
        .collect()
}
fn plan(paths: &[PathBuf], pattern: &str) -> Plan {
    batch_rename::preview(paths, pattern, &Cancellation::default())
        .unwrap()
        .plan()
        .unwrap()
}
fn run(
    db: &mut Database,
    plan: Plan,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&basalgrim::operations::Progress),
) -> fs::Report {
    let op = Operation::BatchRename(plan);
    let mut recorder = db.begin(&op, 1).unwrap();
    fs::operate_with_journal(
        op,
        cancel,
        notify,
        &mut |_| panic!("checked batch must not improvise a new name"),
        &mut recorder,
    )
}

#[test]
fn preview_is_read_only_numbers_in_captured_order_and_preserves_extensions() {
    let root = tempfile::tempdir().unwrap();
    let paths = files(root.path());
    let preview = batch_rename::preview(
        &[paths[2].clone(), paths[0].clone()],
        "Фото_{n:03}_{name}{ext}",
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        preview
            .rows
            .iter()
            .map(|r| r.proposed.as_str())
            .collect::<Vec<_>>(),
        ["Фото_001_c.txt", "Фото_002_a.txt"]
    );
    assert_eq!(preview.changed, 2);
    assert!(preview.ready());
    assert!(paths.iter().all(|p| p.exists()));
    assert_eq!(disk::read_dir(root.path()).unwrap().count(), 3);
    let no_op = batch_rename::preview(&paths, "{full}", &Cancellation::default()).unwrap();
    assert_eq!(no_op.changed, 0);
    assert!(!no_op.ready());
    let hidden = root.path().join(".env");
    disk::write(&hidden, "").unwrap();
    let folder = root.path().join("folder.with.dot");
    disk::create_dir(&folder).unwrap();
    let preview = batch_rename::preview(
        &[hidden, folder],
        "{{{name}}}_{n}{ext}",
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(preview.rows[0].proposed, "{.env}_1");
    assert_eq!(preview.rows[1].proposed, "{folder.with.dot}_2");
}

#[test]
fn advanced_patterns_preserve_extensions_by_default_and_change_them_explicitly() {
    let root = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for name in ["one.png", "two.jpg", "README", ".env", "archive.tar.gz"] {
        let path = root.path().join(name);
        disk::write(&path, name).unwrap();
        paths.push(path);
    }
    let folder = root.path().join("folder.with.dot");
    disk::create_dir(&folder).unwrap();
    paths.push(folder);
    let cancel = Cancellation::default();
    let names = |pattern: &str| {
        let preview = batch_rename::preview(&paths, pattern, &cancel).unwrap();
        assert!(preview.ready(), "{pattern}");
        preview
            .rows
            .into_iter()
            .map(|row| row.proposed)
            .collect::<Vec<_>>()
    };
    let preserved = names("Photo_{n:03}");
    assert_eq!(
        preserved,
        [
            "Photo_001.png",
            "Photo_002.jpg",
            "Photo_003",
            "Photo_004",
            "Photo_005.gz",
            "Photo_006"
        ]
    );
    assert_eq!(names("Photo_{n:03}{ext}"), preserved); // Previously valid explicit templates remain valid.
    assert_eq!(names("{name}_{n:03}")[4], "archive.tar_005.gz");
    assert_eq!(names("prefix_{full}")[0], "prefix_one.png");
    for pattern in [
        "Photo_{n:03}{ext:jpeg}",
        "Photo_{n:03}{ext:.jpeg}",
        "Photo_{n:03}.jpeg",
    ] {
        assert_eq!(
            names(pattern),
            (1..=6)
                .map(|i| format!("Photo_{i:03}.jpeg"))
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(
        names("Photo_{n:03}{ext:}"),
        (1..=6).map(|i| format!("Photo_{i:03}")).collect::<Vec<_>>()
    );
    assert_eq!(names("Photo_{n:03}{ext:tar.gz}")[0], "Photo_001.tar.gz");
    assert_eq!(names("{name}{ext:jpeg}")[4], "archive.tar.jpeg");
    assert_eq!(names("{full}{ext:jpeg}")[4], "archive.tar.jpeg");
    assert_eq!(names("{full}{ext:jpeg}")[5], "folder.with.dot.jpeg");
    assert_eq!(names("{full}{ext:}")[4], "archive.tar");
    assert_eq!(names("Photo_{n}.jpeg{ext:jpg}")[0], "Photo_1.jpg");
    assert_eq!(names(".hidden_{n}")[0], ".hidden_1.png");
    assert_eq!(names("Photo_{n}.{{jpg}}")[0], "Photo_1.{jpg}");
    assert_eq!(names("Photo_{n}.")[0], "Photo_1.");
    for pattern in ["", ".", "..", &"x".repeat(254)] {
        assert!(
            !batch_rename::preview(&paths, pattern, &cancel)
                .unwrap()
                .ready(),
            "{pattern}"
        );
    }
    for text in ["Фото", "Фото.v2", ".hidden", "", "Фото_{name}"] {
        for numbered in [false, true] {
            let outputs = |pattern: String| {
                batch_rename::preview(&paths[..2], &pattern, &cancel)
                    .unwrap()
                    .rows
                    .into_iter()
                    .map(|row| row.proposed)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                outputs(batch_rename::advanced_pattern(text, numbered)),
                outputs(batch_rename::simple_pattern(text, numbered)),
                "mode switch: {text}, numbered={numbered}"
            );
        }
    }
    assert!(paths.iter().all(|path| path.exists())); // Previews leave the fixture untouched.
}

#[test]
fn changing_or_removing_an_extension_rechecks_duplicate_names() {
    let root = tempfile::tempdir().unwrap();
    let paths = ["photo.png", "photo.jpg"].map(|name| {
        let path = root.path().join(name);
        disk::write(&path, name).unwrap();
        path
    });
    let cancel = Cancellation::default();
    let unchanged = batch_rename::preview(&paths, "{name}", &cancel).unwrap();
    assert_eq!(unchanged.changed, 0);
    assert_eq!(unchanged.conflicts, 0);
    for pattern in ["{name}{ext:jpeg}", "{name}{ext:}"] {
        let preview = batch_rename::preview(&paths, pattern, &cancel).unwrap();
        assert_eq!(preview.conflicts, 2);
        assert!(!preview.ready());
    }
    for pattern in [
        "Photo_{ext:jpg}_extra",
        "Photo_{ext:../jpg}",
        "Photo_{ext:.}",
    ] {
        let error = batch_rename::preview(&paths, pattern, &cancel)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("{ext:") && error.contains("Символ 7"),
            "{error}"
        );
    }
}

#[test]
fn duplicate_occupied_invalid_and_non_utf8_names_disable_the_entire_plan() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let paths = files(root.path());
    let cancel = Cancellation::default();
    for pattern in ["same.txt", "../{name}", "{name}/bad", "", &"x".repeat(256)] {
        let result = batch_rename::preview(&paths, pattern, &cancel).unwrap();
        assert!(!result.ready());
        assert!(result.conflicts > 0, "{pattern}");
    }
    for pattern in ["{missing}", "{n:999}", "{n:0}", "{name", "}"] {
        assert!(batch_rename::preview(&paths, pattern, &cancel).is_err());
    }
    disk::write(root.path().join("new_a.txt"), "do not overwrite").unwrap();
    let blocked = batch_rename::preview(&paths, "new_{full}", &cancel).unwrap();
    assert_eq!(blocked.conflicts, 1);
    assert!(!blocked.ready());
    assert!(
        !batch_rename::preview(&[paths[0].clone(), paths[0].clone()], "{n}", &cancel)
            .unwrap()
            .ready()
    );
    let raw = root
        .path()
        .join(std::ffi::OsString::from_vec(b"raw\xff.txt".to_vec()));
    disk::write(&raw, "raw").unwrap();
    assert!(
        !batch_rename::preview(std::slice::from_ref(&raw), "{name}_{n}{ext}", &cancel)
            .unwrap()
            .ready()
    );
    assert_eq!(disk::read(&raw).unwrap(), b"raw");
    assert_eq!(
        disk::read_to_string(root.path().join("new_a.txt")).unwrap(),
        "do not overwrite"
    );
}

#[test]
fn template_errors_identify_the_bad_part_and_show_actual_corrected_patterns() {
    let root = tempfile::tempdir().unwrap();
    let sources = files(root.path());
    let cancel = Cancellation::default();
    let error = batch_rename::preview(&sources, "Снимок_{n:03}{jpg}", &cancel)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Неизвестная переменная: {jpg}"));
    assert!(error.contains("Символ 14"));
    assert!(error.contains("Задать расширение .jpg"));
    assert!(error.contains("Снимок_{n:03}{ext:jpg}"));
    assert!(error.lines().any(|line| line == "Снимок_{n:03}"));
    for (input, part, advice) in [
        ("Снимок_{number}", "{number}", "{n:03}"),
        ("Снимок_{n:bad}{ext}", "{n:bad}", "от 1 до 8"),
        ("Снимок_{n:0}{ext}", "{n:0}", "от 1 до 8"),
        ("Снимок_{n:03", "Не закрыта скобка {", "Добавьте }"),
        ("Снимок_}", "Лишняя скобка }", "Уберите её"),
    ] {
        let error = batch_rename::preview(&sources, input, &cancel)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(part) && error.contains(advice),
            "{input}: {error}"
        );
        assert!(error.contains("Символ 8"), "{error}");
    }
    let fixed = batch_rename::preview(&sources, "Снимок_{n:03}{ext:jpg}", &cancel).unwrap();
    assert!(fixed.ready());
    assert!(fixed.rows.iter().all(|row| row.proposed.ends_with(".jpg")));
    let preserved = batch_rename::preview(&sources, "Снимок_{n:03}", &cancel).unwrap();
    assert!(preserved.ready());
    assert!(sources.iter().all(|path| path.exists()));
}

#[test]
fn stale_sources_parent_and_late_collisions_are_rejected_before_any_rename() {
    for mode in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        disk::create_dir(&folder).unwrap();
        let paths = files(&folder);
        let planned = plan(&paths, "new_{full}");
        match mode {
            0 => disk::write(&paths[1], "changed contents").unwrap(),
            1 => {
                disk::rename(&folder, root.path().join("held")).unwrap();
                disk::create_dir(&folder).unwrap();
                files(&folder);
            }
            _ => disk::write(folder.join("new_c.txt"), "later occupant").unwrap(),
        }
        let report = fs::operate(Operation::BatchRename(planned));
        assert!(report.error.is_some());
        assert!(report.completed.is_empty());
        assert!(paths.iter().all(|p| p.exists()));
        assert!(!folder.join("new_a.txt").exists());
    }
}

#[test]
fn cancellation_and_a_mid_batch_conflict_preserve_exact_completed_counts() {
    for cancel_now in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = files(root.path());
        let planned = plan(&paths, "new_{full}");
        let cancel = Cancellation::default();
        let mut changed = false;
        let mut db = Database::open(&root.path().join("journal")).unwrap();
        let report = run(&mut db, planned, &cancel, &mut |p| {
            if p.items_done == 1 && !changed {
                changed = true;
                if cancel_now {
                    cancel.cancel();
                } else {
                    disk::write(root.path().join("new_b.txt"), "later arrival").unwrap();
                }
            }
        });
        assert_eq!(report.completed, [paths[0].clone()]);
        assert_eq!(report.batch_renamed.len(), 1);
        assert_eq!(report.cancelled, cancel_now);
        assert!(paths[1].exists() && paths[2].exists());
        assert_eq!(
            disk::read_to_string(root.path().join("new_a.txt")).unwrap(),
            "a.txt"
        );
        let id = report.journal.unwrap().id;
        let undo = db.replay(id, false, 2, &Cancellation::default(), &mut |_| {});
        assert!(undo.error.is_none(), "{undo:?}");
        assert!(paths[0].exists());
        if !cancel_now {
            assert_eq!(
                disk::read_to_string(root.path().join("new_b.txt")).unwrap(),
                "later arrival"
            );
        }
    }
}

#[test]
fn one_durable_batch_undoes_and_redoes_after_restart_including_directories_and_links() {
    let root = tempfile::tempdir().unwrap();
    let mut paths = files(root.path());
    let folder = root.path().join("folder");
    disk::create_dir(&folder).unwrap();
    disk::write(folder.join("inside"), "keep").unwrap();
    paths.push(folder);
    let link = root.path().join("link");
    std::os::unix::fs::symlink("missing", &link).unwrap();
    paths.push(link);
    let dbpath = root.path().join("journal");
    let mut db = Database::open(&dbpath).unwrap();
    let report = run(
        &mut db,
        plan(&paths, "new_{full}"),
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 5);
    let id = report.journal.unwrap().id;
    assert_eq!(db.history_state(id).unwrap().undo, 5);
    drop(db);
    let mut db = Database::open(&dbpath).unwrap();
    let undo = db.replay(id, false, 2, &Cancellation::default(), &mut |_| {});
    assert!(undo.error.is_none(), "{undo:?}");
    assert_eq!(undo.completed.len(), 5);
    assert_eq!(
        disk::read_to_string(root.path().join("folder/inside")).unwrap(),
        "keep"
    );
    let redo = db.replay(id, true, 3, &Cancellation::default(), &mut |_| {});
    assert!(redo.error.is_none(), "{redo:?}");
    assert_eq!(redo.completed.len(), 5);
    assert_eq!(
        disk::read_link(root.path().join("new_link")).unwrap(),
        PathBuf::from("missing")
    );
    assert_eq!(
        disk::read_to_string(root.path().join("new_folder/inside")).unwrap(),
        "keep"
    );
}

#[test]
fn journal_failures_before_and_after_publication_never_lose_or_overcount_data() {
    for after in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = files(root.path());
        let planned = plan(&paths, "new_{full}");
        let dbpath = root.path().join("journal");
        let mut db = Database::open(&dbpath).unwrap();
        rusqlite::Connection::open(&dbpath).unwrap().execute_batch(if after {
            "CREATE TRIGGER fault BEFORE UPDATE ON steps WHEN NEW.state='done' BEGIN SELECT RAISE(ABORT,'recording fault'); END;"
        }else{
            "CREATE TRIGGER fault BEFORE INSERT ON steps WHEN NEW.kind='publish' BEGIN SELECT RAISE(ABORT,'intent fault'); END;"
        }).unwrap();
        let report = run(&mut db, planned, &Cancellation::default(), &mut |_| {});
        assert!(report.error.is_some());
        assert_eq!(report.completed.len(), usize::from(after));
        assert_eq!(root.path().join("new_a.txt").exists(), after);
        assert_eq!(paths[0].exists(), !after);
        assert!(paths[1].exists() && paths[2].exists());
    }
}

#[test]
fn batch_rename_crash_child() {
    let Some(root) = std::env::var_os("BASALGRIM_BATCH_CRASH_TEST") else {
        return;
    };
    let root = PathBuf::from(root);
    let paths = files(&root);
    let planned = plan(&paths, "new_{full}");
    let mut db = Database::open(&root.join("journal")).unwrap();
    run(
        &mut db,
        planned,
        &Cancellation::default(),
        &mut |progress| {
            if progress.items_done == 1 {
                disk::write(root.join("published"), "ready").unwrap();
                loop {
                    std::thread::park_timeout(Duration::from_secs(1));
                }
            }
        },
    );
    panic!("parent must SIGKILL the child at publication");
}

#[test]
fn source_changes_and_cancellation_during_a_blocked_intent_are_rechecked() {
    for cancelled in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let paths = files(root.path());
        let operation = Operation::BatchRename(plan(&paths, "new_{full}"));
        let dbpath = root.path().join("journal");
        let mut db = Database::open(&dbpath).unwrap();
        let mut recorder = db.begin(&operation, 1).unwrap();
        let barrier = rusqlite::Connection::open(&dbpath).unwrap();
        barrier.execute_batch("BEGIN IMMEDIATE").unwrap();
        let cancel = Cancellation::default();
        let (send, recv) = std::sync::mpsc::channel();
        let report = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                fs::operate_with_journal(
                    operation,
                    &cancel,
                    &mut |p| {
                        if p.current.as_ref() == Some(&paths[0]) && p.items_done == 0 {
                            let _ = send.send(());
                        }
                    },
                    &mut |_| panic!("unexpected conflict"),
                    &mut recorder,
                )
            });
            recv.recv_timeout(Duration::from_secs(3)).unwrap();
            if cancelled {
                cancel.cancel();
            } else {
                disk::write(&paths[0], "changed while journal is busy").unwrap();
            }
            barrier.execute_batch("COMMIT").unwrap();
            worker.join().unwrap()
        });
        assert!(report.completed.is_empty());
        assert_eq!(report.cancelled, cancelled);
        assert!(paths.iter().all(|p| p.exists()));
        assert!(!root.path().join("new_a.txt").exists());
    }
}

#[test]
fn killed_batch_keeps_published_data_and_is_never_resumed_automatically() {
    let root = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "batch_rename_crash_child", "--nocapture"])
        .env("BASALGRIM_BATCH_CRASH_TEST", root.path())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    while !root.path().join("published").exists() {
        if Instant::now() >= end || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not reach publication");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let mut db = Database::open(&root.path().join("journal")).unwrap();
    let history = db.history().unwrap();
    assert_eq!(history[0].state, "interrupted");
    assert_eq!(
        disk::read_to_string(root.path().join("new_a.txt")).unwrap(),
        "a.txt"
    );
    assert!(root.path().join("b.txt").exists() && root.path().join("c.txt").exists());
    assert!(!root.path().join("a.txt").exists() && !root.path().join("new_b.txt").exists());
    let replay = db.replay(
        history[0].id,
        false,
        3,
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(replay.error.is_some());
    assert!(root.path().join("new_a.txt").exists());
}

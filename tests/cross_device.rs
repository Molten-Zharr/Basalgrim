#![cfg(target_os = "linux")]
use basalgrim::{
    fs::{self, Operation, Report},
    operations::{Cancellation, Phase, Progress},
};
use std::{
    fs as disk,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

fn pair() -> Option<(tempfile::TempDir, tempfile::TempDir)> {
    let source = tempfile::tempdir().unwrap();
    let destination = match tempfile::Builder::new()
        .prefix("basalgrim-cross-")
        .tempdir_in("/dev/shm")
    {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("No writable second filesystem: {error}");
            return None;
        }
    };
    if disk::metadata(source.path()).unwrap().dev()
        == disk::metadata(destination.path()).unwrap().dev()
    {
        eprintln!("Cross-device test needs a second filesystem");
        return None;
    }
    Some((source, destination))
}
fn operation(sources: Vec<PathBuf>, destination: &Path) -> Operation {
    Operation::Transfer {
        sources,
        destination: destination.into(),
        cut: true,
    }
}
fn run(
    source: &Path,
    destination: &Path,
    cancellation: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
) -> Report {
    fs::operate_with_control(
        operation(vec![source.into()], destination),
        cancellation,
        notify,
    )
}
fn no_staging(path: &Path) {
    assert!(disk::read_dir(path).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        !name.to_string_lossy().starts_with(".basalgrim-")
    }));
}
fn claimed(parent: &Path, name: &str) -> Option<PathBuf> {
    disk::read_dir(parent)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".basalgrim-move-")
        })
        .map(|e| e.path().join(name))
}

#[test]
fn real_cross_device_move_preserves_bytes_modes_mtime_and_nonfollowing_links() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("tree");
    disk::create_dir_all(source.join("nested")).unwrap();
    disk::write(source.join("nested/data"), vec![17; 600_001]).unwrap();
    let time = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    disk::File::open(source.join("nested/data"))
        .unwrap()
        .set_times(disk::FileTimes::new().set_modified(time))
        .unwrap();
    disk::set_permissions(
        source.join("nested/data"),
        disk::Permissions::from_mode(0o440),
    )
    .unwrap();
    symlink(".", source.join("loop")).unwrap();
    symlink("missing", source.join("broken")).unwrap();
    let raw = std::ffi::OsString::from_vec(b"raw-\xff".to_vec());
    disk::write(source.join(&raw), "raw name").unwrap();
    let mut phases = Vec::new();
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| phases.push(p.phase),
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.as_slice(), std::slice::from_ref(&source));
    assert!(!source.exists());
    assert!(report.retained_copies.is_empty());
    let target = destination.path().join("tree");
    assert_eq!(
        disk::read(target.join("nested/data")).unwrap(),
        vec![17; 600_001]
    );
    assert_eq!(
        disk::metadata(target.join("nested/data")).unwrap().mode() & 0o777,
        0o440
    );
    assert_eq!(
        disk::metadata(target.join("nested/data"))
            .unwrap()
            .modified()
            .unwrap(),
        time
    );
    assert_eq!(disk::read(target.join(raw)).unwrap(), b"raw name");
    assert_eq!(
        disk::read_link(target.join("loop")).unwrap(),
        Path::new(".")
    );
    assert_eq!(
        disk::read_link(target.join("broken")).unwrap(),
        Path::new("missing")
    );
    assert!(phases.contains(&Phase::Verifying) && phases.contains(&Phase::RemovingSource));
    assert_eq!(report.progress.bytes_done, 600_009);
    assert_eq!(report.progress.bytes_total, Some(600_009));
    assert_eq!(report.progress.fraction(), 1.0);
    no_staging(root.path());
    no_staging(destination.path());
}

#[test]
fn cancellation_during_copy_or_verification_keeps_source_and_publishes_nothing() {
    for phase in [Phase::Running, Phase::Verifying] {
        let Some((root, destination)) = pair() else {
            return;
        };
        let source = root.path().join("large");
        disk::write(&source, vec![5; 900_000]).unwrap();
        let cancellation = Cancellation::default();
        let report = run(&source, destination.path(), &cancellation, &mut |p| {
            if p.phase == phase && p.bytes_done > 0 {
                cancellation.cancel();
            }
        });
        assert!(report.cancelled && report.error.is_none(), "{report:?}");
        assert!(report.completed.is_empty() && report.retained_copies.is_empty());
        assert_eq!(disk::read(&source).unwrap(), vec![5; 900_000]);
        assert!(!destination.path().join("large").exists());
        no_staging(root.path());
        no_staging(destination.path());
    }
}

#[test]
fn changed_source_even_with_restored_mtime_is_not_deleted_or_published() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("changing");
    disk::write(&source, vec![3; 900_000]).unwrap();
    let original_time = disk::metadata(&source).unwrap().modified().unwrap();
    let mut changed = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.bytes_done > 0 && !changed {
                changed = true;
                disk::write(&source, vec![7; 900_000]).unwrap();
                disk::File::open(&source)
                    .unwrap()
                    .set_times(disk::FileTimes::new().set_modified(original_time))
                    .unwrap();
            }
        },
    );
    assert!(changed && report.error.is_some(), "{report:?}");
    assert!(report.completed.is_empty() && report.retained_copies.is_empty());
    assert_eq!(disk::read(&source).unwrap(), vec![7; 900_000]);
    assert!(!destination.path().join("changing").exists());
    no_staging(destination.path());
}

#[test]
fn a_source_replaced_with_a_symlink_never_deletes_or_changes_its_target() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    let outside = root.path().join("outside");
    disk::write(&source, vec![1; 600_000]).unwrap();
    disk::write(&outside, "precious").unwrap();
    let mut changed = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.bytes_done > 0 && !changed {
                changed = true;
                disk::rename(&source, root.path().join("original")).unwrap();
                symlink(&outside, &source).unwrap();
            }
        },
    );
    assert!(report.error.is_some() && report.completed.is_empty());
    assert!(source.is_symlink());
    assert_eq!(disk::read_to_string(outside).unwrap(), "precious");
    assert!(!destination.path().join("source").exists());
    no_staging(destination.path());
}

#[test]
fn a_raced_destination_is_never_replaced_and_source_stays_intact() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "original").unwrap();
    let target = destination.path().join("source");
    let mut raced = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.phase == Phase::Verifying && !raced {
                raced = true;
                disk::write(&target, "precious").unwrap();
            }
        },
    );
    assert!(raced && report.error.is_some() && report.completed.is_empty());
    assert_eq!(disk::read_to_string(source).unwrap(), "original");
    assert_eq!(disk::read_to_string(target).unwrap(), "precious");
    no_staging(destination.path());
}

#[test]
fn cancellation_after_publication_reports_a_retained_copy_and_keeps_the_source() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "payload").unwrap();
    let cancellation = Cancellation::default();
    let report = run(&source, destination.path(), &cancellation, &mut |p| {
        if p.phase == Phase::RemovingSource {
            cancellation.cancel();
        }
    });
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert!(report.completed.is_empty());
    assert_eq!(report.retained_copies.len(), 1);
    assert_eq!(report.retained_copies[0].source, source);
    assert_eq!(
        report.retained_copies[0].destination,
        destination.path().join("source")
    );
    assert_eq!(disk::read_to_string(&source).unwrap(), "payload");
    assert_eq!(
        disk::read_to_string(destination.path().join("source")).unwrap(),
        "payload"
    );
    no_staging(root.path());
    no_staging(destination.path());
}

#[test]
fn a_modified_published_copy_prevents_source_deletion() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "payload").unwrap();
    let mut changed = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.phase == Phase::RemovingSource && !changed {
                changed = true;
                disk::write(destination.path().join("source"), "changed").unwrap();
            }
        },
    );
    assert!(report.error.is_some() && report.completed.is_empty());
    assert_eq!(report.retained_copies.len(), 1);
    assert_eq!(disk::read_to_string(&source).unwrap(), "payload");
    assert_eq!(
        disk::read_to_string(destination.path().join("source")).unwrap(),
        "changed"
    );
    no_staging(root.path());
    no_staging(destination.path());
}

#[test]
fn failed_source_cleanup_preserves_verified_copy_and_restores_remaining_source() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("tree");
    disk::create_dir(&source).unwrap();
    for name in ["a", "b"] {
        disk::write(source.join(name), name).unwrap();
    }
    let mut inserted = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.phase == Phase::RemovingSource
                && let Some(path) = claimed(root.path(), "tree")
                && !path.join("a").exists()
                && !inserted
            {
                inserted = true;
                disk::write(path.join("new-unverified"), "keep me").unwrap();
            }
        },
    );
    assert!(
        inserted && report.error.is_some() && report.completed.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.retained_copies.len(), 1);
    assert!(report.retained_copies[0].recovery.is_none());
    assert_eq!(
        disk::read_to_string(source.join("new-unverified")).unwrap(),
        "keep me"
    );
    for name in ["a", "b"] {
        assert_eq!(
            disk::read_to_string(destination.path().join("tree").join(name)).unwrap(),
            name
        );
    }
    no_staging(root.path());
}

#[test]
fn a_blocked_source_restore_exposes_recovery_path_without_overwriting_or_raii_deletion() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "payload").unwrap();
    let mut changed = false;
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |p| {
            if p.phase == Phase::RemovingSource
                && !changed
                && let Some(path) = claimed(root.path(), "source")
            {
                changed = true;
                disk::write(&source, "new occupant").unwrap();
                disk::write(path, "changed source").unwrap();
            }
        },
    );
    assert!(
        changed && report.error.is_some() && report.completed.is_empty(),
        "{report:?}"
    );
    let recovery = report.retained_copies[0].recovery.as_ref().unwrap();
    assert_eq!(disk::read_to_string(recovery).unwrap(), "changed source");
    assert_eq!(disk::read_to_string(source).unwrap(), "new occupant");
    assert_eq!(
        disk::read_to_string(destination.path().join("source")).unwrap(),
        "payload"
    );
    assert!(
        report
            .error
            .as_ref()
            .unwrap()
            .contains(&recovery.to_string_lossy().to_string())
    );
}

#[test]
fn cancellation_during_committed_cleanup_counts_the_move_before_stopping_the_batch() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let first = root.path().join("tree");
    disk::create_dir(&first).unwrap();
    for name in ["a", "b"] {
        disk::write(first.join(name), name).unwrap();
    }
    let second = root.path().join("second");
    disk::write(&second, "second").unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        operation(vec![first.clone(), second.clone()], destination.path()),
        &cancellation,
        &mut |p| {
            if p.phase == Phase::RemovingSource
                && let Some(path) = claimed(root.path(), "tree")
                && !path.join("a").exists()
            {
                cancellation.cancel();
            }
        },
    );
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.as_slice(), std::slice::from_ref(&first));
    assert!(report.retained_copies.is_empty());
    assert!(!first.exists() && second.exists());
    assert_eq!(
        disk::read_to_string(destination.path().join("tree/b")).unwrap(),
        "b"
    );
    assert!(!destination.path().join("second").exists());
    no_staging(root.path());
}

#[test]
fn a_later_collision_keeps_earlier_moves_completed_and_same_device_moves_keep_the_inode() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let first = root.path().join("a");
    let second = root.path().join("b");
    disk::write(&first, "a").unwrap();
    disk::write(&second, "b").unwrap();
    disk::write(destination.path().join("b"), "keep").unwrap();
    let report = fs::operate(operation(
        vec![first.clone(), second.clone()],
        destination.path(),
    ));
    assert_eq!(report.completed.as_slice(), std::slice::from_ref(&first));
    assert!(report.error.is_some());
    assert!(!first.exists() && second.exists());
    assert_eq!(
        disk::read_to_string(destination.path().join("b")).unwrap(),
        "keep"
    );
    let inode = disk::symlink_metadata(&second).unwrap().ino();
    fs::transfer_one(&second, &root.path().join("renamed"), true).unwrap();
    assert_eq!(
        disk::symlink_metadata(root.path().join("renamed"))
            .unwrap()
            .ino(),
        inode
    );
}

#[test]
fn growing_sources_and_new_directory_entries_abort_before_publication() {
    for growing in [false, true] {
        let Some((root, destination)) = pair() else {
            return;
        };
        let source = root.path().join("tree");
        disk::create_dir(&source).unwrap();
        let file = source.join("data");
        disk::write(&file, vec![1; 600_000]).unwrap();
        let mut changed = false;
        let report = run(
            &source,
            destination.path(),
            &Cancellation::default(),
            &mut |p| {
                if p.bytes_done > 0 && !changed {
                    changed = true;
                    if growing {
                        disk::OpenOptions::new()
                            .write(true)
                            .open(&file)
                            .unwrap()
                            .set_len(1 << 40)
                            .unwrap();
                    } else {
                        disk::write(source.join("added"), "new data").unwrap();
                    }
                }
            },
        );
        assert!(
            changed && report.error.is_some() && report.completed.is_empty(),
            "{report:?}"
        );
        assert!(!destination.path().join("tree").exists());
        if growing {
            assert_eq!(disk::metadata(file).unwrap().len(), 1 << 40);
        } else {
            assert_eq!(
                disk::read_to_string(source.join("added")).unwrap(),
                "new data"
            );
        }
        no_staging(destination.path());
    }
}

#[test]
fn hard_linked_sources_are_verified_and_removed_without_false_change_errors() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("tree");
    disk::create_dir(&source).unwrap();
    disk::write(source.join("a"), "hardlinked").unwrap();
    disk::hard_link(source.join("a"), source.join("b")).unwrap();
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_none(), "{report:?}");
    assert!(!source.exists());
    for name in ["a", "b"] {
        assert_eq!(
            disk::read_to_string(destination.path().join("tree").join(name)).unwrap(),
            "hardlinked"
        );
    }
    no_staging(root.path());
}

#[test]
fn read_only_source_cleanup_reports_the_copy_without_changing_source_permissions() {
    let Some((root, destination)) = pair() else {
        return;
    };
    if disk::metadata(root.path()).unwrap().uid() == 0 {
        return;
    }
    let source = root.path().join("tree");
    disk::create_dir(&source).unwrap();
    disk::write(source.join("data"), "keep").unwrap();
    disk::set_permissions(&source, disk::Permissions::from_mode(0o555)).unwrap();
    let report = run(
        &source,
        destination.path(),
        &Cancellation::default(),
        &mut |_| {},
    );
    let mode = disk::metadata(&source).unwrap().mode() & 0o777;
    disk::set_permissions(&source, disk::Permissions::from_mode(0o755)).unwrap();
    let target = destination.path().join("tree");
    let target_mode = disk::metadata(&target).unwrap().mode() & 0o777;
    disk::set_permissions(&target, disk::Permissions::from_mode(0o755)).unwrap();
    assert!(
        report.error.is_some() && report.completed.is_empty(),
        "{report:?}"
    );
    assert_eq!(mode, 0o555);
    assert_eq!(target_mode, 0o555);
    assert_eq!(report.retained_copies.len(), 1, "{report:?}");
    assert_eq!(disk::read_to_string(source.join("data")).unwrap(), "keep");
    assert_eq!(
        disk::read_to_string(destination.path().join("tree/data")).unwrap(),
        "keep"
    );
    no_staging(root.path());
}

#[test]
fn partial_cross_device_batches_keep_only_unfinished_cut_paths_in_the_app_clipboard() {
    use basalgrim::{app::App, input::Action, settings::Settings};
    use std::time::{Duration, Instant};
    fn settle(app: &mut App) {
        let end = Instant::now() + Duration::from_secs(5);
        while app.loading() {
            app.tick();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let Some((root, destination)) = pair() else {
        return;
    };
    let first = root.path().join("a");
    let last = root.path().join("z");
    disk::write(&first, "first").unwrap();
    disk::write(&last, "last").unwrap();
    disk::write(destination.path().join("z"), "precious").unwrap();
    let mut app = App::new(
        root.path().into(),
        Settings {
            preview: false,
            ..Settings::default()
        },
    );
    settle(&mut app);
    app.action(Action::SelectAll);
    app.action(Action::Cut);
    app.navigate(destination.path().into(), true);
    settle(&mut app);
    app.action(Action::Paste);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(app.dialog, Some(basalgrim::app::Dialog::Conflict(_))) {
        app.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    app.choose_conflict(basalgrim::conflicts::Choice::Skip);
    settle(&mut app);
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths.as_slice(),
        std::slice::from_ref(&last)
    );
    assert!(!first.exists() && last.exists());
    assert_eq!(app.operations[0].completed_paths, [first]);
    assert_eq!(
        app.operations[0].skipped_paths.as_slice(),
        std::slice::from_ref(&last)
    );
    assert_eq!(
        disk::read_to_string(destination.path().join("z")).unwrap(),
        "precious"
    );
    let retry = destination.path().join("retry");
    disk::create_dir(&retry).unwrap();
    app.navigate(retry.clone(), true);
    settle(&mut app);
    app.action(Action::Paste);
    settle(&mut app);
    assert_eq!(disk::read_to_string(retry.join("z")).unwrap(), "last");
    assert!(!last.exists());
    assert!(app.clipboard.as_ref().unwrap().paths.is_empty());
}

#[test]
fn incomplete_moves_and_verification_phases_are_visible_in_the_shared_operations_screen() {
    use basalgrim::{app::App, input::Action, operations::OperationRecord, settings::Settings, ui};
    use ratatui::{Terminal, backend::TestBackend};
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "payload").unwrap();
    let cancellation = Cancellation::default();
    let report = run(&source, destination.path(), &cancellation, &mut |p| {
        if p.phase == Phase::RemovingSource {
            cancellation.cancel();
        }
    });
    let mut record = OperationRecord::new(100, "Перемещение".into(), 1);
    record.progress.phase = Phase::Verifying;
    assert_eq!(record.status(), "Проверка копии…");
    record.progress.phase = Phase::RemovingSource;
    assert_eq!(record.status(), "Удаление исходника…");
    record.finish(report);
    let mut app = App::new(root.path().into(), Settings::default());
    app.operations.push_front(record);
    app.action(Action::Operations);
    let mut terminal = Terminal::new(TestBackend::new(170, 50)).unwrap();
    terminal.draw(|frame| ui::render(&mut app, frame)).unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Копия создана; перенос не завершён"));
    assert!(
        text.contains(
            &destination
                .path()
                .join("source")
                .to_string_lossy()
                .to_string()
        )
    );
    assert!(text.contains("Готово 0/1"));
}

#[test]
fn cancellation_after_source_claim_restores_its_name_and_keeps_both_copies() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let source = root.path().join("source");
    disk::write(&source, "payload").unwrap();
    let cancellation = Cancellation::default();
    let report = run(&source, destination.path(), &cancellation, &mut |p| {
        if p.phase == Phase::RemovingSource && claimed(root.path(), "source").is_some() {
            cancellation.cancel();
        }
    });
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert!(report.completed.is_empty());
    assert_eq!(report.retained_copies.len(), 1);
    assert_eq!(disk::read_to_string(&source).unwrap(), "payload");
    assert_eq!(
        disk::read_to_string(destination.path().join("source")).unwrap(),
        "payload"
    );
    no_staging(root.path());
}

#[test]
fn root_symlinks_move_without_following_and_special_files_are_rejected() {
    let Some((root, destination)) = pair() else {
        return;
    };
    let link = root.path().join("link");
    symlink("missing", &link).unwrap();
    fs::transfer_one(&link, &destination.path().join("link"), true).unwrap();
    assert!(disk::symlink_metadata(&link).is_err());
    assert_eq!(
        disk::read_link(destination.path().join("link")).unwrap(),
        Path::new("missing")
    );
    let pipe = root.path().join("pipe");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &pipe,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR,
        0,
    )
    .unwrap();
    let report = run(
        &pipe,
        destination.path(),
        &Cancellation::default(),
        &mut |_| {},
    );
    assert!(report.error.is_some() && report.completed.is_empty());
    assert!(disk::symlink_metadata(pipe).is_ok());
    assert!(!destination.path().join("pipe").exists());
    no_staging(root.path());
    no_staging(destination.path());
}

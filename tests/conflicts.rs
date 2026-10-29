#![cfg(target_os = "linux")]
use basalgrim::{
    app::{App, Dialog, Target},
    conflicts::{Choice, Decision},
    fs::{self, Operation},
    input::{Action, Input, Key, KeyPress},
    operations::{Cancellation, Phase},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs as disk,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn transfer(sources: Vec<PathBuf>, destination: &Path, cut: bool) -> Operation {
    Operation::Transfer {
        sources,
        destination: destination.into(),
        cut,
    }
}
fn decide(choice: Choice, remaining: bool) -> Decision {
    Decision { choice, remaining }
}
fn clean(path: &Path) {
    assert!(disk::read_dir(path).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".basalgrim-")
    }));
}
fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(app, frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if predicate(app) {
            break;
        }
        assert!(Instant::now() < end, "worker timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn settled(app: &mut App) {
    wait(app, |a| !a.loading());
}
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let dest = root.path().join("dest");
    disk::create_dir(&source).unwrap();
    disk::create_dir(&dest).unwrap();
    disk::write(source.join("file.txt"), "incoming").unwrap();
    disk::write(dest.join("file.txt"), "existing").unwrap();
    (root, source, dest)
}

#[test]
fn replace_skip_and_keep_both_have_distinct_safe_results() {
    for choice in [Choice::Replace, Choice::Skip, Choice::KeepBoth] {
        for cut in [false, true] {
            let (_root, source, dest) = fixture();
            let file = source.join("file.txt");
            let mut prompts = 0;
            let report = fs::operate_with_conflicts(
                transfer(vec![file.clone()], &dest, cut),
                &Cancellation::default(),
                &mut |_| {},
                &mut |c| {
                    prompts += 1;
                    assert_eq!(c.incoming.bytes, Some(8));
                    assert_eq!(c.existing.bytes, Some(8));
                    decide(choice, false)
                },
            );
            assert!(report.error.is_none(), "{report:?}");
            assert_eq!(prompts, 1);
            assert_eq!(
                disk::read_to_string(dest.join("file.txt")).unwrap(),
                if choice == Choice::Replace {
                    "incoming"
                } else {
                    "existing"
                }
            );
            if choice == Choice::KeepBoth {
                assert_eq!(
                    disk::read_to_string(dest.join("file (1).txt")).unwrap(),
                    "incoming"
                );
            }
            assert_eq!(report.completed.len(), usize::from(choice != Choice::Skip));
            assert_eq!(report.skipped.len(), usize::from(choice == Choice::Skip));
            assert_eq!(file.exists(), !cut || choice == Choice::Skip);
            clean(&dest);
            clean(&source);
        }
    }
}

#[test]
fn batch_policy_applies_only_to_its_operation_and_skip_is_reported_separately() {
    for choice in [Choice::Replace, Choice::Skip, Choice::KeepBoth] {
        let (_root, source, dest) = fixture();
        let mut files = vec![source.join("file.txt")];
        for name in ["second.txt", "third.txt"] {
            let p = source.join(name);
            disk::write(&p, "incoming").unwrap();
            files.push(p);
        }
        disk::write(dest.join("second.txt"), "existing").unwrap();
        let mut prompts = 0;
        let report = fs::operate_with_conflicts(
            transfer(files, &dest, false),
            &Cancellation::default(),
            &mut |_| {},
            &mut |c| {
                prompts += 1;
                assert_eq!(c.remaining, 2);
                decide(choice, true)
            },
        );
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(prompts, 1);
        assert_eq!(
            report.skipped.len(),
            if choice == Choice::Skip { 2 } else { 0 }
        );
        assert_eq!(
            report.completed.len(),
            if choice == Choice::Skip { 1 } else { 3 }
        );
        assert_eq!(report.progress.fraction(), 1.0);
        assert_eq!(
            disk::read_to_string(dest.join("third.txt")).unwrap(),
            "incoming"
        );
        let mut next = 0;
        fs::operate_with_conflicts(
            transfer(vec![source.join("file.txt")], &dest, false),
            &Cancellation::default(),
            &mut |_| {},
            &mut |_| {
                next += 1;
                decide(Choice::Skip, false)
            },
        );
        assert_eq!(next, 1);
    }
}

#[test]
fn self_copy_can_keep_both_and_self_move_is_skipped_without_destroying_data() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("file.txt");
    disk::write(&file, "same").unwrap();
    let report = fs::operate_with_conflicts(
        transfer(vec![file.clone()], root.path(), false),
        &Cancellation::default(),
        &mut |_| {},
        &mut |c| {
            assert!(!c.can_replace);
            decide(Choice::KeepBoth, false)
        },
    );
    assert!(report.error.is_none());
    assert_eq!(
        disk::read_to_string(root.path().join("file (1).txt")).unwrap(),
        "same"
    );
    let report = fs::operate_with_conflicts(
        transfer(vec![file.clone()], root.path(), true),
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| panic!("self move should be skipped"),
    );
    assert_eq!(report.skipped.as_slice(), std::slice::from_ref(&file));
    assert_eq!(disk::read_to_string(file).unwrap(), "same");
}

#[test]
fn directories_are_replaced_as_a_whole_and_symlink_targets_are_never_modified() {
    let (root, source, dest) = fixture();
    disk::create_dir(source.join("tree")).unwrap();
    disk::write(source.join("tree/new"), "new").unwrap();
    disk::create_dir(dest.join("tree")).unwrap();
    disk::write(dest.join("tree/old"), "old").unwrap();
    let outside = root.path().join("outside");
    disk::write(&outside, "precious").unwrap();
    disk::remove_file(dest.join("file.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("file.txt")).unwrap();
    let report = fs::operate_with_conflicts(
        transfer(
            vec![source.join("tree"), source.join("file.txt")],
            &dest,
            false,
        ),
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| decide(Choice::Replace, true),
    );
    assert!(report.error.is_none(), "{report:?}");
    assert!(!dest.join("tree/old").exists());
    assert_eq!(disk::read_to_string(dest.join("tree/new")).unwrap(), "new");
    assert!(!dest.join("file.txt").is_symlink());
    assert_eq!(disk::read_to_string(outside).unwrap(), "precious");
    clean(&dest);
}

#[test]
fn keep_both_handles_unicode_raw_names_long_names_and_rename() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let (root, source, dest) = fixture();
    for name in [
        std::ffi::OsString::from("界".repeat(83) + ".bin"),
        std::ffi::OsString::from_vec(b"raw-\xff.bin".to_vec()),
    ] {
        disk::write(source.join(&name), "new").unwrap();
        disk::write(dest.join(&name), "old").unwrap();
        let report = fs::operate_with_conflicts(
            transfer(vec![source.join(&name)], &dest, false),
            &Cancellation::default(),
            &mut |_| {},
            &mut |_| decide(Choice::KeepBoth, false),
        );
        assert!(report.error.is_none(), "{report:?}");
        let candidate = disk::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name() != Some(name.as_os_str()) && disk::read(p).is_ok_and(|b| b == b"new")
            })
            .unwrap();
        assert!(candidate.file_name().unwrap().as_bytes().len() <= 255);
        if name.to_str().is_some() {
            assert!(candidate.file_name().unwrap().to_str().is_some());
        }
        disk::remove_file(candidate).unwrap();
    }
    let original = root.path().join("original.txt");
    disk::write(&original, "rename").unwrap();
    disk::write(root.path().join("name.txt"), "keep").unwrap();
    let report = fs::operate_with_conflicts(
        Operation::Rename {
            source: original.clone(),
            name: "name.txt".into(),
        },
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| decide(Choice::KeepBoth, false),
    );
    assert!(report.error.is_none());
    assert!(!original.exists());
    assert_eq!(
        disk::read_to_string(root.path().join("name (1).txt")).unwrap(),
        "rename"
    );
}

#[test]
fn changed_target_requires_a_new_decision_even_after_replace_all() {
    let (_root, source, dest) = fixture();
    let target = dest.join("file.txt");
    let mut prompts = 0;
    let report = fs::operate_with_conflicts(
        transfer(vec![source.join("file.txt")], &dest, false),
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| {
            prompts += 1;
            if prompts == 1 {
                disk::write(&target, "a different occupant").unwrap();
                decide(Choice::Replace, true)
            } else {
                decide(Choice::Skip, false)
            }
        },
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(prompts, 2);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        disk::read_to_string(target).unwrap(),
        "a different occupant"
    );
    clean(&dest);
}

#[test]
fn raced_publication_asks_about_the_new_target_and_preserves_it() {
    let (_root, source, dest) = fixture();
    let target = dest.join("file.txt");
    disk::remove_file(&target).unwrap();
    disk::write(source.join("file.txt"), vec![1; 600_000]).unwrap();
    let mut raced = false;
    let mut prompts = 0;
    let report = fs::operate_with_conflicts(
        transfer(vec![source.join("file.txt")], &dest, false),
        &Cancellation::default(),
        &mut |p| {
            if p.bytes_done > 0 && !raced {
                raced = true;
                disk::write(&target, "raced").unwrap();
            }
        },
        &mut |_| {
            prompts += 1;
            decide(Choice::KeepBoth, false)
        },
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(prompts, 1);
    assert_eq!(disk::read_to_string(target).unwrap(), "raced");
    assert_eq!(
        disk::read(dest.join("file (1).txt")).unwrap(),
        vec![1; 600_000]
    );
    clean(&dest);
}

#[test]
fn cancel_during_copy_leaves_the_old_target_visible_and_cancels_pending_prompts() {
    let (_root, source, dest) = fixture();
    let target = dest.join("file.txt");
    disk::write(source.join("file.txt"), vec![2; 600_000]).unwrap();
    let cancel = Cancellation::default();
    let report = fs::operate_with_conflicts(
        transfer(vec![source.join("file.txt")], &dest, false),
        &cancel,
        &mut |p| {
            if p.bytes_done > 0 {
                assert_eq!(disk::read_to_string(&target).unwrap(), "existing");
                cancel.cancel();
            }
        },
        &mut |_| decide(Choice::Replace, false),
    );
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert!(report.completed.is_empty());
    clean(&dest);
    assert_eq!(disk::read_to_string(target).unwrap(), "existing");
}

#[test]
fn cleanup_failure_retains_changed_backup_and_still_counts_the_published_copy() {
    let (_root, source, dest) = fixture();
    let mut changed = false;
    let report = fs::operate_with_conflicts(
        transfer(vec![source.join("file.txt")], &dest, true),
        &Cancellation::default(),
        &mut |p| {
            if p.phase == Phase::CleaningUp && !changed {
                let backup = disk::read_dir(&dest)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".basalgrim-replaced-")
                    })
                    .unwrap()
                    .join("file.txt");
                disk::write(backup, "newly edited backup").unwrap();
                changed = true;
            }
        },
        &mut |_| decide(Choice::Replace, false),
    );
    assert!(changed && report.error.is_some(), "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert_eq!(report.destination_recoveries.len(), 1);
    assert_eq!(
        disk::read_to_string(&report.destination_recoveries[0].backup).unwrap(),
        "newly edited backup"
    );
    assert_eq!(
        disk::read_to_string(dest.join("file.txt")).unwrap(),
        "incoming"
    );
    assert!(!source.join("file.txt").exists());
}

#[test]
fn cross_device_replacement_and_keep_both_keep_move_completion_semantics() {
    use std::os::unix::fs::MetadataExt;
    let (_root, source, _) = fixture();
    let Ok(dest) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    if disk::metadata(&source).unwrap().dev() == disk::metadata(dest.path()).unwrap().dev() {
        return;
    }
    disk::write(dest.path().join("file.txt"), "old").unwrap();
    let report = fs::operate_with_conflicts(
        transfer(vec![source.join("file.txt")], dest.path(), true),
        &Cancellation::default(),
        &mut |_| {},
        &mut |_| decide(Choice::Replace, false),
    );
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 1);
    assert!(!source.join("file.txt").exists());
    assert_eq!(
        disk::read_to_string(dest.path().join("file.txt")).unwrap(),
        "incoming"
    );
    clean(dest.path());
    clean(&source);
}

fn app_fixture() -> (tempfile::TempDir, App, PathBuf) {
    let (root, source, dest) = fixture();
    let mut app = App::new(
        source,
        Settings {
            preview: false,
            ..Settings::default()
        },
    );
    settled(&mut app);
    app.action(Action::Copy);
    app.navigate(dest.clone(), true);
    settled(&mut app);
    app.action(Action::Paste);
    wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
    (root, app, dest)
}

#[test]
fn conflict_mouse_controls_work_in_compact_layouts_without_hover_or_dismissal_click_through() {
    for (w, h) in [(100, 32), (34, 14)] {
        let (_root, mut app, dest) = app_fixture();
        let selected = app.tab().selected;
        let before = app.operation_count;
        let text = render(&mut app, w, h);
        assert!(text.contains("Сохранить оба"));
        let rect = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::ConflictChoice(Choice::Skip)).then_some(*r))
            .unwrap();
        app.handle(Input::Pointer(Some((rect.x, rect.y).into())));
        render(&mut app, w, h);
        assert_eq!(app.tab().selected, selected);
        assert!(matches!(app.dialog, Some(Dialog::Conflict(_))));
        assert_eq!(
            disk::read_to_string(dest.join("file.txt")).unwrap(),
            "existing"
        );
        app.handle(Input::PointerDown {
            x: rect.x,
            y: rect.y,
            double: false,
            toggle: false,
        });
        app.handle(Input::PointerUp {
            x: rect.x,
            y: rect.y,
        });
        settled(&mut app);
        assert_eq!(app.operation_count, before + 1);
        assert_eq!(app.operations[0].skipped_paths.len(), 1);
        assert_eq!(
            disk::read_to_string(dest.join("file.txt")).unwrap(),
            "existing"
        );
    }
}

#[test]
fn default_enter_keeps_both_and_escape_or_stop_cancels_the_worker_prompt() {
    let (_root, mut app, dest) = app_fixture();
    app.handle(Input::Key(KeyPress::plain(Key::Enter)));
    settled(&mut app);
    assert_eq!(
        disk::read_to_string(dest.join("file (1).txt")).unwrap(),
        "incoming"
    );
    for stop in [false, true] {
        let (_root, mut app, dest) = app_fixture();
        let key = if stop {
            app.settings
                .bindings
                .iter()
                .find(|b| b.action == Action::CancelOperation)
                .unwrap()
                .key
        } else {
            KeyPress::plain(Key::Escape)
        };
        app.handle(Input::Key(key));
        settled(&mut app);
        assert!(app.operations[0].cancelled);
        assert!(app.operations[0].completed_paths.is_empty());
        assert_eq!(
            disk::read_to_string(dest.join("file.txt")).unwrap(),
            "existing"
        );
        assert!(!matches!(app.dialog, Some(Dialog::Conflict(_))));
    }
}

#[test]
fn replacing_a_file_with_a_directory_or_the_reverse_does_not_merge_contents() {
    for incoming_directory in [false, true] {
        let (_root, source, dest) = fixture();
        let input = source.join("file.txt");
        let target = dest.join("file.txt");
        if incoming_directory {
            disk::remove_file(&input).unwrap();
            disk::create_dir(&input).unwrap();
            disk::write(input.join("new"), "new").unwrap();
        } else {
            disk::remove_file(&target).unwrap();
            disk::create_dir(&target).unwrap();
            disk::write(target.join("old"), "old").unwrap();
        }
        let report = fs::operate_with_conflicts(
            transfer(vec![input], &dest, false),
            &Cancellation::default(),
            &mut |_| {},
            &mut |_| decide(Choice::Replace, false),
        );
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(target.is_dir(), incoming_directory);
        if incoming_directory {
            assert_eq!(disk::read_to_string(target.join("new")).unwrap(), "new");
        } else {
            assert_eq!(disk::read_to_string(&target).unwrap(), "incoming");
        }
        clean(&dest);
    }
}

#[test]
fn cancellation_after_one_applied_replacement_preserves_unprocessed_targets() {
    let (_root, source, dest) = fixture();
    disk::write(source.join("second"), "new second").unwrap();
    disk::write(dest.join("second"), "old second").unwrap();
    let cancel = Cancellation::default();
    let mut prompts = 0;
    let report = fs::operate_with_conflicts(
        transfer(
            vec![source.join("file.txt"), source.join("second")],
            &dest,
            true,
        ),
        &cancel,
        &mut |p| {
            if p.items_done == 1 {
                cancel.cancel();
            }
        },
        &mut |_| {
            prompts += 1;
            decide(Choice::Replace, true)
        },
    );
    assert!(report.cancelled && report.error.is_none(), "{report:?}");
    assert_eq!(prompts, 1);
    assert_eq!(report.completed.len(), 1);
    assert_eq!(
        disk::read_to_string(dest.join("file.txt")).unwrap(),
        "incoming"
    );
    assert_eq!(
        disk::read_to_string(dest.join("second")).unwrap(),
        "old second"
    );
    assert!(source.join("second").exists());
    clean(&dest);
}

#[test]
fn checkbox_scope_is_shared_by_mouse_and_keyboard_and_suspended_help_is_restored() {
    let (_root, source, dest) = fixture();
    disk::write(source.join("second"), "new").unwrap();
    disk::write(dest.join("second"), "old").unwrap();
    let mut app = App::new(
        source,
        Settings {
            preview: false,
            ..Settings::default()
        },
    );
    settled(&mut app);
    app.action(Action::SelectAll);
    app.action(Action::Copy);
    app.navigate(dest.clone(), true);
    settled(&mut app);
    app.action(Action::Paste);
    app.action(Action::Help);
    wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
    render(&mut app, 34, 14);
    let scope = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::ConflictRemaining).then_some(*r))
        .unwrap();
    app.handle(Input::Click {
        x: scope.x,
        y: scope.y,
        double: false,
        toggle: false,
    });
    assert!(matches!(&app.dialog,Some(Dialog::Conflict(c))if c.remaining));
    app.handle(Input::Key(KeyPress::plain(Key::Char(' '))));
    assert!(matches!(&app.dialog,Some(Dialog::Conflict(c))if !c.remaining));
    app.handle(Input::Click {
        x: scope.x,
        y: scope.y,
        double: false,
        toggle: false,
    });
    app.choose_conflict(Choice::Skip);
    settled(&mut app);
    assert!(matches!(app.dialog, Some(Dialog::Help)));
    assert_eq!(app.operations[0].skipped_paths.len(), 2);
    assert_eq!(disk::read_to_string(dest.join("second")).unwrap(), "old");
}

#[test]
fn a_disabled_replace_row_cannot_resolve_a_self_copy() {
    let root = tempfile::tempdir().unwrap();
    disk::write(root.path().join("file"), "keep").unwrap();
    let mut app = App::new(
        root.path().into(),
        Settings {
            preview: false,
            ..Settings::default()
        },
    );
    settled(&mut app);
    app.action(Action::Copy);
    app.action(Action::Paste);
    wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
    render(&mut app, 34, 14);
    let row = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::ConflictChoice(Choice::Replace)).then_some(*r))
        .unwrap();
    app.handle(Input::Click {
        x: row.x,
        y: row.y,
        double: false,
        toggle: false,
    });
    assert!(matches!(app.dialog, Some(Dialog::Conflict(_))));
    app.cancel_dialog();
    settled(&mut app);
    assert_eq!(
        disk::read_to_string(root.path().join("file")).unwrap(),
        "keep"
    );
    clean(root.path());
}

#[test]
fn worker_wait_is_cancellable_even_when_the_reply_sender_remains_alive() {
    use basalgrim::worker::{Event, Worker};
    let (_root, source, dest) = fixture();
    let worker = Worker::new();
    let cancel = Cancellation::default();
    assert!(worker.operate(
        123,
        transfer(vec![source.join("file.txt")], &dest, false),
        cancel.clone()
    ));
    let reply = match worker.events.recv_timeout(Duration::from_secs(3)).unwrap() {
        Event::Conflict {
            token: 123, reply, ..
        } => reply,
        _ => panic!("expected conflict"),
    };
    cancel.cancel();
    match worker.events.recv_timeout(Duration::from_secs(3)).unwrap() {
        Event::Operation { token: 123, report } => assert!(report.cancelled),
        _ => panic!("expected cancelled report"),
    };
    assert!(reply.try_send(decide(Choice::Replace, false)).is_err());
    assert_eq!(
        disk::read_to_string(dest.join("file.txt")).unwrap(),
        "existing"
    );
    clean(&dest);
}

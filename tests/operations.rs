use basalgrim::{
    fs::{self, Operation},
    operations::{Cancellation, Phase, Progress},
};
use std::{fs as disk, path::Path};

fn copy(sources: Vec<std::path::PathBuf>, destination: &Path) -> Operation {
    Operation::Transfer {
        sources,
        destination: destination.into(),
        cut: false,
    }
}
fn no_staging(destination: &Path) {
    assert!(disk::read_dir(destination).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".basalgrim-copy-")
    }));
}

#[test]
fn recursive_progress_counts_bytes_entries_and_never_follows_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    disk::create_dir_all(source.join("nested")).unwrap();
    disk::create_dir(&destination).unwrap();
    disk::write(source.join("data"), vec![7; 300_000]).unwrap();
    disk::write(source.join("nested/note"), "abc").unwrap();
    std::os::unix::fs::symlink(".", source.join("loop")).unwrap();
    let (mut bytes, mut saw_running) = (0, false);
    let report = fs::operate_with_control(
        copy(vec![source.clone()], &destination),
        &Cancellation::default(),
        &mut |progress| {
            assert!(progress.bytes_done >= bytes);
            bytes = progress.bytes_done;
            saw_running |= progress.phase == Phase::Running && progress.bytes_done > 0;
        },
    );
    assert!(report.error.is_none());
    assert!(!report.cancelled);
    assert_eq!(report.completed, [source]);
    assert_eq!(report.progress.bytes_total, Some(300_003));
    assert_eq!(report.progress.bytes_done, 300_003);
    assert_eq!(report.progress.entries_total, Some(5));
    assert_eq!(report.progress.entries_done, 5);
    assert!(saw_running);
    assert_eq!(report.progress.fraction(), 1.0);
    assert_eq!(
        disk::read_link(destination.join("source/loop")).unwrap(),
        Path::new(".")
    );
    assert_eq!(
        disk::read(destination.join("source/data")).unwrap(),
        vec![7; 300_000]
    );
    no_staging(&destination);
}

#[test]
fn cancellation_during_copy_cleans_partial_output_and_preserves_the_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("large.bin");
    let destination = root.path().join("destination");
    disk::create_dir(&destination).unwrap();
    disk::write(&source, vec![42; 2_000_000]).unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        copy(vec![source.clone()], &destination),
        &cancellation,
        &mut |progress| {
            if progress.bytes_done > 0 {
                cancellation.cancel();
            }
        },
    );
    assert!(report.cancelled);
    assert!(report.error.is_none());
    assert!(report.completed.is_empty());
    assert!(report.progress.bytes_done > 0 && report.progress.bytes_done < 2_000_000);
    assert!(!destination.join("large.bin").exists());
    assert_eq!(disk::read(source).unwrap(), vec![42; 2_000_000]);
    no_staging(&destination);
}

#[test]
fn cancelling_preparation_or_an_undispatched_operation_has_no_output() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    disk::create_dir_all(source.join("nested")).unwrap();
    disk::create_dir(&destination).unwrap();
    disk::write(source.join("nested/file"), "keep").unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        copy(vec![source], &destination),
        &cancellation,
        &mut |progress| {
            if progress.phase == Phase::Preparing && progress.current.is_some() {
                cancellation.cancel();
            }
        },
    );
    assert!(report.cancelled);
    assert_eq!(report.progress.bytes_done, 0);
    assert_eq!(disk::read_dir(&destination).unwrap().count(), 0);
    let report = fs::operate_with_control(
        Operation::Create {
            parent: destination.clone(),
            name: "uncreated".into(),
            directory: false,
        },
        &cancellation,
        &mut |_| {},
    );
    assert!(report.cancelled);
    assert!(!destination.join("uncreated").exists());
}

#[test]
fn cancellation_before_publication_cleans_read_only_staging() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("readonly");
    let destination = root.path().join("destination");
    disk::create_dir(&source).unwrap();
    disk::create_dir(&destination).unwrap();
    disk::write(source.join("data"), "original").unwrap();
    disk::set_permissions(&source, disk::Permissions::from_mode(0o555)).unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        copy(vec![source.clone()], &destination),
        &cancellation,
        &mut |progress| {
            if progress.phase == Phase::Running
                && progress.entries_done == 2
                && progress.current.as_deref() == Some(&source)
            {
                cancellation.cancel();
            }
        },
    );
    let source_mode = disk::metadata(&source).unwrap().permissions().mode() & 0o777;
    disk::set_permissions(&source, disk::Permissions::from_mode(0o755)).unwrap();
    assert!(report.cancelled);
    assert!(report.error.is_none());
    assert_eq!(source_mode, 0o555);
    assert_eq!(
        disk::read_to_string(source.join("data")).unwrap(),
        "original"
    );
    assert!(!destination.join("readonly").exists());
    no_staging(&destination);
}

#[test]
fn cancelling_between_items_preserves_exact_completed_paths() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let destination = root.path().join("destination");
    disk::write(&first, "one").unwrap();
    disk::write(&second, "two").unwrap();
    disk::create_dir(&destination).unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        copy(vec![first.clone(), second.clone()], &destination),
        &cancellation,
        &mut |progress| {
            if progress.items_done == 1 {
                cancellation.cancel();
            }
        },
    );
    assert!(report.cancelled);
    assert_eq!(report.completed.as_slice(), std::slice::from_ref(&first));
    assert_eq!(report.total, 2);
    assert_eq!(report.progress.bytes_done, 3);
    assert_eq!(report.progress.bytes_total, Some(6));
    assert_eq!(
        disk::read_to_string(destination.join("first")).unwrap(),
        "one"
    );
    assert!(!destination.join("second").exists());
    assert!(first.exists() && second.exists());
    no_staging(&destination);
}

#[test]
fn cancellation_racing_an_atomic_move_counts_the_completed_move() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let destination = root.path().join("destination");
    disk::write(&first, "one").unwrap();
    disk::write(&second, "two").unwrap();
    disk::create_dir(&destination).unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        Operation::Transfer {
            sources: vec![first.clone(), second.clone()],
            destination: destination.clone(),
            cut: true,
        },
        &cancellation,
        &mut |progress| {
            if progress.entries_done == 1 {
                cancellation.cancel();
            }
        },
    );
    assert!(report.cancelled);
    assert_eq!(report.completed.as_slice(), std::slice::from_ref(&first));
    assert!(!first.exists());
    assert!(second.exists());
    assert_eq!(
        disk::read_to_string(destination.join("first")).unwrap(),
        "one"
    );
    assert!(!destination.join("second").exists());
}

#[test]
fn a_late_cancel_does_not_turn_a_completed_publication_into_a_cancelled_report() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let destination = root.path().join("destination");
    disk::write(&source, "keep").unwrap();
    disk::create_dir(&destination).unwrap();
    let cancellation = Cancellation::default();
    let report = fs::operate_with_control(
        copy(vec![source.clone()], &destination),
        &cancellation,
        &mut |progress| {
            if progress.items_done == 1 {
                cancellation.cancel();
            }
        },
    );
    assert!(cancellation.is_cancelled());
    assert!(!report.cancelled);
    assert!(report.error.is_none());
    assert_eq!(report.completed, [source]);
    assert_eq!(
        disk::read_to_string(destination.join("source")).unwrap(),
        "keep"
    );
}

#[test]
fn completion_uses_actual_bytes_when_a_source_changes_after_the_estimate() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("changing");
    let destination = root.path().join("destination");
    disk::write(&source, vec![5; 300_000]).unwrap();
    disk::create_dir(&destination).unwrap();
    let mut changed = false;
    let report = fs::operate_with_control(
        copy(vec![source.clone()], &destination),
        &Cancellation::default(),
        &mut |progress| {
            if !changed && progress.phase == Phase::Running {
                disk::OpenOptions::new()
                    .write(true)
                    .open(&source)
                    .unwrap()
                    .set_len(100_000)
                    .unwrap();
                changed = true;
            }
        },
    );
    assert!(report.error.is_none());
    assert_eq!(report.progress.bytes_done, 100_000);
    assert_eq!(report.progress.bytes_total, Some(100_000));
    assert_eq!(report.progress.fraction(), 1.0);
    assert_eq!(
        disk::metadata(destination.join("changing")).unwrap().len(),
        100_000
    );
}

#[test]
fn a_later_collision_does_not_erase_earlier_work_or_overwrite_data() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let destination = root.path().join("destination");
    disk::write(&first, "one").unwrap();
    disk::write(&second, "two").unwrap();
    disk::create_dir(&destination).unwrap();
    disk::write(destination.join("second"), "precious").unwrap();
    let report = fs::operate_with_control(
        copy(vec![first.clone(), second], &destination),
        &Cancellation::default(),
        &mut |_: &Progress| {},
    );
    assert!(report.error.is_some());
    assert!(!report.cancelled);
    assert_eq!(report.completed, [first]);
    assert_eq!(report.progress.items_done, 1);
    assert_eq!(
        disk::read_to_string(destination.join("first")).unwrap(),
        "one"
    );
    assert_eq!(
        disk::read_to_string(destination.join("second")).unwrap(),
        "precious"
    );
    no_staging(&destination);
}

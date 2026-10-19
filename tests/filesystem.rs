use basalgrim::fs::{self, Operation};
use std::{fs as stdfs, path::Path};

#[test]
fn create_and_rename_never_overwrite_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().to_path_buf();
    stdfs::write(parent.join("keep"), "precious").unwrap();
    let report = fs::operate(Operation::Create {
        parent: parent.clone(),
        name: "keep".into(),
        directory: false,
    });
    assert!(report.error.is_some());
    assert_eq!(
        stdfs::read_to_string(parent.join("keep")).unwrap(),
        "precious"
    );
    stdfs::write(parent.join("source"), "original").unwrap();
    let report = fs::operate(Operation::Rename {
        source: parent.join("source"),
        name: "keep".into(),
    });
    assert!(report.error.is_some());
    assert!(parent.join("source").exists());
    assert_eq!(
        stdfs::read_to_string(parent.join("keep")).unwrap(),
        "precious"
    );
}

#[test]
fn invalid_names_cannot_escape_the_current_directory() {
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "new\nfile",
        "nul\0byte",
    ] {
        assert!(fs::validate_name(name).is_err(), "{name:?}");
    }
    for name in ["Новая папка", ".config", "notes..txt", "résumé.md"] {
        assert!(fs::validate_name(name).is_ok());
    }
}

#[test]
fn recursive_copy_preserves_contents_and_source() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("source");
    let dst = root.path().join("target");
    stdfs::create_dir_all(src.join("nested/deep")).unwrap();
    stdfs::write(src.join("nested/deep/данные.txt"), "содержимое").unwrap();
    fs::transfer_one(&src, &dst, false).unwrap();
    assert_eq!(
        stdfs::read_to_string(dst.join("nested/deep/данные.txt")).unwrap(),
        "содержимое"
    );
    assert!(src.join("nested/deep/данные.txt").exists());
    assert_eq!(stdfs::read_dir(root.path()).unwrap().count(), 2);
}

#[test]
fn copying_a_directory_into_itself_or_a_descendant_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("source");
    stdfs::create_dir_all(src.join("child")).unwrap();
    assert!(fs::transfer_one(&src, &src.join("copy"), false).is_err());
    assert!(fs::transfer_one(&src, &src.join("child/copy"), false).is_err());
    assert!(!src.join("copy").exists());
    assert!(!src.join("child/copy").exists());
}

#[test]
fn partial_batches_report_completed_items_without_touching_collisions() {
    let root = tempfile::tempdir().unwrap();
    let dest = root.path().join("dest");
    stdfs::create_dir(&dest).unwrap();
    let sources = ["a", "b", "c"].map(|name| {
        let p = root.path().join(name);
        stdfs::write(&p, name).unwrap();
        p
    });
    stdfs::write(dest.join("b"), "keep").unwrap();
    let report = fs::operate(Operation::Transfer {
        sources: sources.to_vec(),
        destination: dest.clone(),
        cut: false,
    });
    assert_eq!(report.completed, vec![sources[0].clone()]);
    assert_eq!(report.total, 3);
    assert!(report.error.is_some());
    assert_eq!(stdfs::read_to_string(dest.join("b")).unwrap(), "keep");
    assert!(!dest.join("c").exists());
    assert!(sources.iter().all(|p| p.exists()));
}

#[test]
fn move_and_rename_preserve_data_and_reject_an_occupied_target() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    stdfs::write(&a, "payload").unwrap();
    fs::transfer_one(&a, &b, true).unwrap();
    assert!(!a.exists());
    assert_eq!(stdfs::read_to_string(&b).unwrap(), "payload");
    stdfs::write(&a, "other").unwrap();
    assert!(fs::transfer_one(&a, &b, true).is_err());
    assert_eq!(stdfs::read_to_string(&a).unwrap(), "other");
}

#[cfg(unix)]
#[test]
fn copying_symlinks_does_not_follow_targets_or_cycles() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dst = root.path().join("dst");
    stdfs::create_dir(&src).unwrap();
    symlink(".", src.join("loop")).unwrap();
    symlink("missing", src.join("broken")).unwrap();
    fs::transfer_one(&src, &dst, false).unwrap();
    assert_eq!(stdfs::read_link(dst.join("loop")).unwrap(), Path::new("."));
    assert_eq!(
        stdfs::read_link(dst.join("broken")).unwrap(),
        Path::new("missing")
    );
}

#[cfg(unix)]
#[test]
fn dangling_symlinks_and_aliases_cannot_bypass_collision_or_recursion_checks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    stdfs::create_dir(&src).unwrap();
    let alias = root.path().join("alias");
    symlink(&src, &alias).unwrap();
    assert!(fs::transfer_one(&src, &alias.join("recursive"), false).is_err());
    let file = root.path().join("file");
    stdfs::write(&file, "payload").unwrap();
    let broken = root.path().join("broken");
    symlink("absent", &broken).unwrap();
    assert!(fs::transfer_one(&file, &broken, false).is_err());
    assert!(broken.is_symlink());
    assert!(fs::rename_new(&file, &broken).is_err());
    assert!(file.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn failed_copy_removes_staging_and_never_publishes_a_partial_tree() {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    stdfs::create_dir(&src).unwrap();
    stdfs::write(src.join("file"), "copied before error").unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        src.join("pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(fs::transfer_one(&src, &root.path().join("dst"), false).is_err());
    assert!(!root.path().join("dst").exists());
    assert_eq!(stdfs::read_dir(root.path()).unwrap().count(), 1);
    assert!(fs::preview(&src.join("pipe")).unwrap().lines[0].contains("Специальный"));
}

#[cfg(target_os = "linux")]
#[test]
fn failed_copy_cleans_read_only_directories_without_changing_the_source() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let readonly = source.join("read-only");
    stdfs::create_dir_all(&readonly).unwrap();
    stdfs::write(readonly.join("file"), "payload").unwrap();
    stdfs::set_permissions(&readonly, stdfs::Permissions::from_mode(0o555)).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        source.join("pipe"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(fs::transfer_one(&source, &root.path().join("destination"), false).is_err());
    assert_eq!(stdfs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(
        stdfs::metadata(&readonly).unwrap().permissions().mode() & 0o777,
        0o555
    );
    stdfs::set_permissions(readonly, stdfs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn previews_are_bounded_and_escape_terminal_controls() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("large.txt");
    stdfs::write(&file, "line\x1b[31m\n".repeat(20_000)).unwrap();
    let preview = fs::preview(&file).unwrap();
    assert!(preview.lines.len() <= 301);
    assert!(preview.lines.iter().all(|s| !s.contains('\x1b')));
    assert!(preview.lines.last().unwrap().contains("ограничен"));
    stdfs::write(&file, [0, 1, 2, 3]).unwrap();
    assert_eq!(fs::preview(&file).unwrap().lines[0], "Бинарный файл");
}

#[test]
fn listing_sorts_directories_first_and_tracks_hidden_files() {
    let root = tempfile::tempdir().unwrap();
    stdfs::create_dir(root.path().join("z-folder")).unwrap();
    stdfs::write(root.path().join("a.txt"), "a").unwrap();
    stdfs::write(root.path().join(".hidden"), "b").unwrap();
    let mut entries = fs::read_directory(root.path()).unwrap();
    fs::Sort::Name.sort(&mut entries);
    assert!(entries[0].directory);
    assert!(entries.iter().find(|e| e.name == ".hidden").unwrap().hidden);
}

#[test]
fn concurrent_renames_have_one_winner_and_never_replace_the_winner() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let dest = root.path().join("dest");
    stdfs::write(&a, "a").unwrap();
    stdfs::write(&b, "b").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let barrier2 = barrier.clone();
    let dest2 = dest.clone();
    let job = std::thread::spawn(move || {
        barrier2.wait();
        fs::rename_new(&a, &dest2).is_ok()
    });
    barrier.wait();
    let second = fs::rename_new(&b, &dest).is_ok();
    let first = job.join().unwrap();
    assert_ne!(first, second);
    assert_eq!(
        stdfs::read_to_string(dest).unwrap(),
        if first { "a" } else { "b" }
    );
}

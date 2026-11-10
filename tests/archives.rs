use basalgrim::{
    archive::{Catalog, Extraction, Format},
    conflicts::{Choice, Decision},
    fs::{self as ops, Operation},
    journal::Database,
    operations::Cancellation,
};
use std::{fs, io::Write, path::Path};

fn pack(source: &Path, destination: &Path, format: Format) {
    let report = ops::operate(Operation::ArchiveCreate {
        sources: vec![source.into()],
        destination: destination.into(),
        format,
    });
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(report.completed.len(), 1);
}
fn tree(root: &Path) {
    fs::create_dir_all(root.join("nested/empty")).unwrap();
    fs::write(root.join("nested/Привет.txt"), "Точное содержимое\n").unwrap();
    fs::write(root.join(".hidden"), "hidden").unwrap();
    std::os::unix::fs::symlink("nested/Привет.txt", root.join("link")).unwrap();
}
fn clean_staging(root: &Path) {
    assert!(!fs::read_dir(root).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b".basalgrim-archive-")
    }));
}
#[test]
fn round_trip_and_virtual_navigation_preserve_files_empty_directories_links_and_hidden() {
    for format in [Format::Zip, Format::Tar, Format::TarGz] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("data");
        tree(&source);
        let archive = temp.path().join(format!("copy{}", format.suffix()));
        pack(&source, &archive, format);
        let catalog = Catalog::read(&archive, &Default::default()).unwrap();
        assert_eq!(catalog.list(&archive).unwrap()[0].name, "data");
        assert_eq!(catalog.list(&archive.join("data/nested")).unwrap().len(), 2);
        let preview = catalog
            .preview(
                &archive.join("data/nested/Привет.txt"),
                false,
                &Default::default(),
            )
            .unwrap();
        assert!(preview.lines.join("\n").contains("Точное содержимое"));
        let output = temp.path().join("out");
        let report = ops::operate(Operation::ArchiveExtract {
            request: Extraction::all(archive.clone()),
            destination: output.clone(),
        });
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(
            fs::read(output.join("data/nested/Привет.txt")).unwrap(),
            fs::read(source.join("nested/Привет.txt")).unwrap()
        );
        assert!(output.join("data/nested/empty").is_dir());
        assert_eq!(
            fs::read_link(output.join("data/link")).unwrap(),
            Path::new("nested/Привет.txt")
        );
        assert_eq!(
            fs::read_to_string(output.join("data/.hidden")).unwrap(),
            "hidden"
        );
        clean_staging(temp.path());
    }
}
#[test]
fn selected_members_keep_relative_names_and_reject_changed_archive() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("data");
    tree(&source);
    let archive = temp.path().join("data.tar.gz");
    pack(&source, &archive, Format::TarGz);
    let catalog = Catalog::read(&archive, &Default::default()).unwrap();
    let request = Extraction {
        source: archive.clone(),
        stamp: Some(catalog.stamp.clone()),
        base: "data/nested".into(),
        selected: vec!["data/nested/Привет.txt".into()],
    };
    let out = temp.path().join("selection");
    let report = ops::operate(Operation::ArchiveExtract {
        request: request.clone(),
        destination: out.clone(),
    });
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
    assert!(out.join("Привет.txt").is_file());
    fs::write(&archive, "changed").unwrap();
    let second = temp.path().join("stale");
    let report = ops::operate(Operation::ArchiveExtract {
        request,
        destination: second.clone(),
    });
    assert!(report.error.is_some());
    assert!(!second.exists());
    clean_staging(temp.path());
}
#[test]
fn archive_creation_and_extraction_are_journalled_with_conflicts_undo_and_redo() {
    for choice in [Choice::KeepBoth, Choice::Replace, Choice::Skip] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("data");
        tree(&source);
        let archive = temp.path().join("data.zip");
        let mut db = Database::open(&temp.path().join("history.sqlite3")).unwrap();
        let operation = Operation::ArchiveCreate {
            sources: vec![source.clone()],
            destination: archive.clone(),
            format: Format::Zip,
        };
        let mut recorder = db.begin(&operation, 1).unwrap();
        let report = ops::operate_with_journal(
            operation,
            &Default::default(),
            &mut |_| {},
            &mut |_| Decision {
                choice,
                remaining: true,
            },
            &mut recorder,
        );
        assert!(report.error.is_none(), "{report:?}");
        let packed = report.journal.unwrap().id;
        drop(recorder);
        assert_eq!(db.history_state(packed).unwrap().undo, 1);
        let undo = db.replay(packed, false, 2, &Default::default(), &mut |_| {});
        assert!(undo.error.is_none(), "{undo:?}");
        assert!(!archive.exists());
        assert!(source.exists());
        let redo = db.replay(packed, true, 3, &Default::default(), &mut |_| {});
        assert!(redo.error.is_none(), "{redo:?}");
        assert!(Catalog::read(&archive, &Default::default()).is_ok());
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("old"), "retained").unwrap();
        let operation = Operation::ArchiveExtract {
            request: Extraction::all(archive.clone()),
            destination: output.clone(),
        };
        let mut recorder = db.begin(&operation, 2).unwrap();
        let report = ops::operate_with_journal(
            operation,
            &Default::default(),
            &mut |_| {},
            &mut |_| Decision {
                choice,
                remaining: true,
            },
            &mut recorder,
        );
        assert!(report.error.is_none(), "{report:?}");
        drop(recorder);
        if choice == Choice::Skip {
            assert_eq!(report.skipped.len(), 1);
            assert!(report.completed.is_empty());
            assert!(output.join("old").exists());
            continue;
        }
        let id = report.journal.unwrap().id;
        assert_eq!(db.history_state(id).unwrap().undo, 1);
        let actual = if choice == Choice::KeepBoth {
            temp.path().join("output (1)")
        } else {
            output.clone()
        };
        assert_eq!(report.completed, vec![actual.clone()]);
        assert!(actual.join("data/nested/Привет.txt").exists());
        let report = db.replay(id, false, 3, &Default::default(), &mut |_| {});
        assert!(report.error.is_none(), "{report:?}");
        assert!(output.join("old").exists());
        fs::remove_file(&archive).unwrap();
        fs::remove_dir_all(&source).unwrap();
        drop(db);
        let mut db = Database::open(&temp.path().join("history.sqlite3")).unwrap();
        let report = db.replay(id, true, 4, &Default::default(), &mut |_| {});
        assert!(report.error.is_none(), "{report:?}");
        assert!(actual.join("data/nested/Привет.txt").exists());
    }
}
#[test]
fn cancellation_discards_partial_archive_and_extraction_without_touching_existing_data() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("large");
    fs::write(&source, vec![b'a'; 2 * 1024 * 1024]).unwrap();
    let archive = temp.path().join("large.tar.gz");
    let cancel = Cancellation::default();
    let report = ops::operate_with_control(
        Operation::ArchiveCreate {
            sources: vec![source.clone()],
            destination: archive.clone(),
            format: Format::TarGz,
        },
        &cancel,
        &mut |p| {
            if p.bytes_done > 0 {
                cancel.cancel();
            }
        },
    );
    assert!(report.cancelled, "{report:?}");
    assert!(!archive.exists());
    assert!(source.exists());
    clean_staging(temp.path());
    pack(&source, &archive, Format::TarGz);
    let destination = temp.path().join("out");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("old"), "keep").unwrap();
    let cancel = Cancellation::default();
    let report = ops::operate_with_control(
        Operation::ArchiveExtract {
            request: Extraction::all(archive),
            destination: destination.clone(),
        },
        &cancel,
        &mut |p| {
            if p.bytes_done > 0 {
                cancel.cancel();
            }
        },
    );
    assert!(report.cancelled, "{report:?}");
    assert_eq!(fs::read_to_string(destination.join("old")).unwrap(), "keep");
    clean_staging(temp.path());
}
fn unsafe_zip(path: &Path, name: &str, symlink_parent: bool) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    if symlink_parent {
        zip.add_symlink("parent", "../outside", options).unwrap();
    }
    zip.start_file(name, options).unwrap();
    zip.write_all(b"evil").unwrap();
    zip.finish().unwrap();
}
#[test]
fn traversal_absolute_names_and_symlink_parents_are_rejected_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("outside")).unwrap();
    for (name, link) in [
        ("../escape", false),
        ("/absolute", false),
        ("C:/escape", false),
        ("parent/child", true),
    ] {
        let archive = temp.path().join("bad.zip");
        unsafe_zip(&archive, name, link);
        let out = temp.path().join("out");
        let report = ops::operate(Operation::ArchiveExtract {
            request: Extraction::all(archive),
            destination: out.clone(),
        });
        assert!(report.error.is_some(), "{name}: {report:?}");
        assert!(!out.exists());
        assert_eq!(
            fs::read_dir(temp.path().join("outside")).unwrap().count(),
            0
        );
        clean_staging(temp.path());
    }
}
#[test]
fn refuses_recursive_output_and_truncated_or_corrupt_payload() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("data");
    tree(&source);
    let nested = source.join("self.zip");
    let report = ops::operate(Operation::ArchiveCreate {
        sources: vec![source.clone()],
        destination: nested.clone(),
        format: Format::Zip,
    });
    assert!(report.error.is_some());
    assert!(!nested.exists());
    clean_staging(&source);
    let archive = temp.path().join("data.zip");
    pack(&source, &archive, Format::Zip);
    let mut data = fs::read(&archive).unwrap();
    data.truncate(data.len() / 2);
    fs::write(&archive, data).unwrap();
    let out = temp.path().join("out");
    let report = ops::operate(Operation::ArchiveExtract {
        request: Extraction::all(archive),
        destination: out.clone(),
    });
    assert!(report.error.is_some());
    assert!(!out.exists());
    clean_staging(temp.path());
}

#[test]
fn tar_preserves_raw_unix_names_and_zip_reports_unsupported_names() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    let name = OsString::from_vec(b"raw-\xff".to_vec());
    let source = temp.path().join(&name);
    fs::write(&source, "raw").unwrap();
    let archive = temp
        .path()
        .join(OsString::from_vec(b"pack-\xff.tar.gz".to_vec()));
    pack(&source, &archive, Format::TarGz);
    assert_eq!(Format::of(&archive), Some(Format::TarGz));
    let output = basalgrim::archive::unpacked_path(&archive);
    assert_eq!(output.file_name().unwrap().as_encoded_bytes(), b"pack-\xff");
    let report = ops::operate(Operation::ArchiveExtract {
        request: Extraction::all(archive),
        destination: output.clone(),
    });
    assert!(report.error.is_none(), "{report:?}");
    assert_eq!(fs::read(output.join(name)).unwrap(), b"raw");
    let zip = temp.path().join("out.zip");
    let report = ops::operate(Operation::ArchiveCreate {
        sources: vec![source],
        destination: zip.clone(),
        format: Format::Zip,
    });
    assert!(report.error.unwrap().contains("TAR"));
    assert!(!zip.exists());
    clean_staging(temp.path());
}

fn link_tar(path: &Path, malicious: bool) {
    let mut writer = tar::Builder::new(fs::File::create(path).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o600);
    header.set_size(4);
    writer
        .append_data(&mut header, "real", &b"data"[..])
        .unwrap();
    header.set_size(0);
    if malicious {
        header.set_entry_type(tar::EntryType::Symlink);
        writer
            .append_link(&mut header, "parent", "../outside")
            .unwrap();
    }
    header.set_entry_type(tar::EntryType::Link);
    writer.append_link(&mut header, "first", "second").unwrap();
    writer
        .append_link(
            &mut header,
            "second",
            if malicious { "parent/secret" } else { "real" },
        )
        .unwrap();
    writer.finish().unwrap();
}

#[test]
fn hardlinks_resolve_only_to_catalog_files_never_through_symlink_ancestors() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("outside")).unwrap();
    fs::write(temp.path().join("outside/secret"), "untouched").unwrap();
    for malicious in [false, true] {
        let archive = temp.path().join("links.tar");
        link_tar(&archive, malicious);
        let output = temp.path().join(if malicious { "bad" } else { "good" });
        let report = ops::operate(Operation::ArchiveExtract {
            request: Extraction::all(archive),
            destination: output.clone(),
        });
        if malicious {
            assert!(report.error.is_some(), "{report:?}");
            assert!(!output.exists());
        } else {
            assert!(report.error.is_none(), "{report:?}");
            assert_eq!(fs::read(output.join("first")).unwrap(), b"data");
            assert_eq!(
                fs::metadata(output.join("first")).unwrap().ino(),
                fs::metadata(output.join("real")).unwrap().ino()
            );
        }
        assert_eq!(
            fs::read_to_string(temp.path().join("outside/secret")).unwrap(),
            "untouched"
        );
        clean_staging(temp.path());
    }
}

#[test]
fn oversized_gnu_metadata_and_zip64_directory_are_rejected_before_allocation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("huge.tar");
    let mut header = tar::Header::new_gnu();
    header.set_path("././@LongLink").unwrap();
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_size(1 << 40);
    header.set_cksum();
    fs::write(&path, header.as_bytes()).unwrap();
    let error = Catalog::read(&path, &Default::default()).unwrap_err();
    assert!(error.to_string().contains("64 КиБ"), "{error:#}");
    let path = temp.path().join("huge.zip");
    let mut bytes = vec![0u8; 56 + 20 + 22];
    bytes[..4].copy_from_slice(b"PK\x06\x06");
    bytes[4..12].copy_from_slice(&44u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&(u64::MAX - 1).to_le_bytes());
    bytes[56..60].copy_from_slice(b"PK\x06\x07");
    bytes[72..76].copy_from_slice(&1u32.to_le_bytes());
    bytes[76..80].copy_from_slice(b"PK\x05\x06");
    bytes[86..88].copy_from_slice(&u16::MAX.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    let error = Catalog::read(&path, &Default::default()).unwrap_err();
    assert!(
        error.to_string().contains("Слишком большой каталог"),
        "{error:#}"
    );
}

#[test]
fn duplicate_zip_members_are_reported_instead_of_silently_hidden() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("duplicates.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for name in ["file-a", "file-b"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"data").unwrap();
    }
    zip.finish().unwrap();
    let mut bytes = fs::read(&path).unwrap();
    for i in 0..bytes.len().saturating_sub(6) {
        if &bytes[i..i + 6] == b"file-b" {
            bytes[i + 5] = b'a';
        }
    }
    fs::write(&path, bytes).unwrap();
    let error = Catalog::read(&path, &Default::default()).unwrap_err();
    assert!(error.to_string().contains("Повторяющееся имя"), "{error:#}");
}

#[test]
fn unix_special_zip_entries_are_not_treated_as_regular_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("special.zip");
    unsafe_zip(&path, "device", false);
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    bytes[at + 38..at + 42].copy_from_slice(&((0o020600u32) << 16).to_le_bytes());
    fs::write(&path, bytes).unwrap();
    let error = Catalog::read(&path, &Default::default()).unwrap_err();
    assert!(
        error.to_string().contains("специальный файл ZIP"),
        "{error:#}"
    );
}

#[test]
fn publication_journal_failures_preserve_sources_and_exact_archive_completion() {
    for extract in [false, true] {
        for after in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("original");
            fs::write(&source, "preserved").unwrap();
            let archive = temp.path().join("input.tar.gz");
            pack(&source, &archive, Format::TarGz);
            let destination = temp.path().join(if extract { "out" } else { "out.zip" });
            let operation = if extract {
                Operation::ArchiveExtract {
                    request: Extraction::all(archive.clone()),
                    destination: destination.clone(),
                }
            } else {
                Operation::ArchiveCreate {
                    sources: vec![source.clone()],
                    destination: destination.clone(),
                    format: Format::Zip,
                }
            };
            let dbpath = temp.path().join("history.sqlite3");
            let mut db = Database::open(&dbpath).unwrap();
            rusqlite::Connection::open(&dbpath).unwrap().execute_batch(if after {
                "CREATE TRIGGER fail AFTER UPDATE OF state ON steps WHEN NEW.state='done' AND NEW.kind='publish' BEGIN SELECT RAISE(FAIL,'result failure'); END;"
            } else {
                "CREATE TRIGGER fail BEFORE INSERT ON steps WHEN NEW.kind='publish' BEGIN SELECT RAISE(FAIL,'intent failure'); END;"
            }).unwrap();
            let mut recorder = db.begin(&operation, 1).unwrap();
            let report = ops::operate_with_journal(
                operation,
                &Default::default(),
                &mut |_| {},
                &mut |_| Decision {
                    choice: Choice::KeepBoth,
                    remaining: false,
                },
                &mut recorder,
            );
            assert!(report.error.is_some(), "{report:?}");
            assert_eq!(report.completed.len(), usize::from(after), "{report:?}");
            assert_eq!(destination.exists(), after);
            assert_eq!(fs::read_to_string(&source).unwrap(), "preserved");
            assert!(archive.is_file());
            clean_staging(temp.path());
        }
    }
}

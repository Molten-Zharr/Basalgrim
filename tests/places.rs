use basalgrim::places::{Command, Config, Section, Store};
use std::{fs, path::Path};
fn config(root: &Path) -> Config {
    Config {
        database: root.join("places.sqlite3"),
        bookmarks: root.join("bookmarks"),
        legacy_bookmarks: root.join("legacy"),
        recent: root.join("recently-used.xbel"),
    }
}
fn recent(uri: &str, private: bool) -> String {
    format!(
        r#"<bookmark href="{uri}" visited="2026-11-01T12:00:00Z"><title>Photo &amp; notes</title><info><metadata owner="http://freedesktop.org"><bookmark:application name="OtherApp" exec="must-not-run"/>{}</metadata></info></bookmark>"#,
        if private { "<bookmark:private/>" } else { "" }
    )
}
#[test]
fn desktop_imports_decode_labels_paths_and_private_recent_entries_without_rewriting_sources() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let folder = root.path().join("a folder");
    fs::create_dir(&folder).unwrap();
    let uri = basalgrim::clipboard::files::uri(&folder).unwrap();
    let gtk = format!(
        "{uri} Work folder\nfile:///tmp/another Another\nsmb://server/share Remote\nfile://remote/invalid Bad\n{uri} Duplicate\n"
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    let xml = format!(
        r#"<?xml version="1.0"?><xbel xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">{}{}</xbel>"#,
        recent("file:///tmp/a%20file.txt", false),
        recent("file:///tmp/private.txt", true)
    );
    fs::write(&c.recent, &xml).unwrap();
    let mut store = Store::open(&c.database).unwrap();
    let snapshot = store.snapshot(&c).unwrap();
    assert_eq!(snapshot.bookmarks.len(), 2);
    assert_eq!(snapshot.bookmarks[0].label, "Work folder");
    assert_eq!(snapshot.recent.len(), 1);
    assert_eq!(snapshot.recent[0].path, Path::new("/tmp/a file.txt"));
    assert_eq!(snapshot.recent[0].label, "Photo & notes");
    store.apply(Command::Bookmark(folder.clone())).unwrap();
    let snapshot = store.snapshot(&c).unwrap();
    assert_eq!(snapshot.bookmarks.len(), 2);
    assert!(!snapshot.bookmarks[0].desktop);
    store
        .apply(Command::Remove {
            section: Section::Recent,
            path: "/tmp/a file.txt".into(),
        })
        .unwrap();
    assert!(store.snapshot(&c).unwrap().recent.is_empty());
    assert_eq!(fs::read_to_string(&c.bookmarks).unwrap(), gtk);
    assert_eq!(fs::read_to_string(&c.recent).unwrap(), xml);
}
#[test]
fn own_places_round_trip_raw_paths_rename_labels_remove_metadata_and_merge_instances() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let a = root.path().join(OsString::from_vec(b"raw-\xff".to_vec()));
    let b = root.path().join("other");
    fs::write(&a, "A").unwrap();
    fs::write(&b, "B").unwrap();
    let mut first = Store::open(&c.database).unwrap();
    let mut second = Store::open(&c.database).unwrap();
    first
        .apply(Command::Favorite {
            paths: vec![a.clone()],
            add: true,
        })
        .unwrap();
    second
        .apply(Command::Favorite {
            paths: vec![b.clone()],
            add: true,
        })
        .unwrap();
    first
        .apply(Command::Rename {
            section: Section::Favorites,
            path: a.clone(),
            label: "Имя закладки".into(),
        })
        .unwrap();
    assert_eq!(second.snapshot(&c).unwrap().favorites.len(), 2);
    drop(first);
    drop(second);
    let mut store = Store::open(&c.database).unwrap();
    let snapshot = store.snapshot(&c).unwrap();
    assert_eq!(snapshot.favorites[0].path, a);
    assert_eq!(snapshot.favorites[0].label, "Имя закладки");
    store
        .apply(Command::Remove {
            section: Section::Favorites,
            path: a.clone(),
        })
        .unwrap();
    assert_eq!(fs::read(a).unwrap(), b"A");
    assert_eq!(fs::read(b).unwrap(), b"B");
}
#[test]
fn malformed_imports_and_foreign_stores_are_preserved_and_do_not_expand_entities() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    fs::write(
        &c.recent,
        "<!DOCTYPE xbel [<!ENTITY leak SYSTEM 'file:///etc/passwd'>]><xbel>&leak;</xbel>",
    )
    .unwrap();
    let store = Store::open(&c.database).unwrap();
    let snapshot = store.snapshot(&c).unwrap();
    assert!(snapshot.recent.is_empty());
    assert!(snapshot.warnings[0].contains("DTD"));
    fs::write(&c.recent, "<xbel><bookmark href='file:///tmp/a'>").unwrap();
    assert!(!store.snapshot(&c).unwrap().warnings.is_empty());
    for data in [b"corrupt".as_slice(), b"not a database"] {
        let path = root.path().join("foreign");
        fs::write(&path, data).unwrap();
        assert!(Store::open(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), data);
    }
    let foreign = root.path().join("other.sqlite3");
    rusqlite::Connection::open(&foreign)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(value); PRAGMA user_version=1;")
        .unwrap();
    let bytes = fs::read(&foreign).unwrap();
    assert!(Store::open(&foreign).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), bytes);
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&c.database, &link).unwrap();
    assert!(Store::open(&link).is_err());
}
#[test]
fn failed_group_favorite_is_atomic_and_forgotten_recent_returns_only_on_a_new_open() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let mut store = Store::open(&c.database).unwrap();
    let path = root.path().join("file");
    fs::write(&path, "contents").unwrap();
    assert!(
        store
            .apply(Command::Favorite {
                paths: vec![path.clone(), root.path().join("missing")],
                add: true
            })
            .is_err()
    );
    assert!(store.snapshot(&c).unwrap().favorites.is_empty());
    store.apply(Command::Recent(path.clone())).unwrap();
    store
        .apply(Command::Remove {
            section: Section::Recent,
            path: path.clone(),
        })
        .unwrap();
    assert!(store.snapshot(&c).unwrap().recent.is_empty());
    store.apply(Command::Recent(path.clone())).unwrap();
    assert_eq!(store.snapshot(&c).unwrap().recent[0].path, path);
    assert_eq!(fs::read_to_string(path).unwrap(), "contents");
}

#[test]
fn empty_desktop_files_legacy_fallback_and_updated_recent_entries_are_supported() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let mut store = Store::open(&c.database).unwrap();
    fs::write(&c.legacy_bookmarks, "file:///tmp/legacy Legacy\n").unwrap();
    fs::write(&c.recent, "<xbel/>").unwrap();
    let s = store.snapshot(&c).unwrap();
    assert!(s.warnings.is_empty() && s.recent.is_empty());
    assert_eq!(s.bookmarks[0].label, "Legacy");
    // An existing, empty GTK3 file supersedes the older GTK2 bookmarks.
    fs::write(&c.bookmarks, "").unwrap();
    assert!(store.snapshot(&c).unwrap().bookmarks.is_empty());
    let xml = |stamp: i64| {
        format!(
            "<xbel><bookmark href='file:///tmp/recent' visited='{}'/></xbel>",
            chrono::DateTime::from_timestamp(stamp, 0)
                .unwrap()
                .to_rfc3339()
        )
    };
    fs::write(&c.recent, xml(chrono::Utc::now().timestamp() - 60)).unwrap();
    assert_eq!(store.snapshot(&c).unwrap().recent.len(), 1);
    store
        .apply(Command::Remove {
            section: Section::Recent,
            path: "/tmp/recent".into(),
        })
        .unwrap();
    assert!(store.snapshot(&c).unwrap().recent.is_empty());
    fs::write(&c.recent, xml(chrono::Utc::now().timestamp() + 60)).unwrap();
    assert_eq!(store.snapshot(&c).unwrap().recent.len(), 1);
}

#[test]
fn oversized_imports_do_not_hide_own_places_and_oversized_database_values_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let mut store = Store::open(&c.database).unwrap();
    store.apply(Command::Bookmark(root.path().into())).unwrap();
    fs::File::create(&c.bookmarks)
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    let s = store.snapshot(&c).unwrap();
    assert_eq!(s.bookmarks.len(), 1);
    assert!(s.warnings[0].contains("8 МиБ"));
    rusqlite::Connection::open(&c.database)
        .unwrap()
        .execute(
            "INSERT INTO dismissed(path,visited) VALUES(zeroblob(65537),0)",
            [],
        )
        .unwrap();
    assert!(store.snapshot(&c).is_err());
}

#[test]
fn removed_imported_bookmarks_stay_hidden_and_can_be_added_again_across_instances() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let folder = root.path().join(OsString::from_vec(b"video-\xff".to_vec()));
    fs::create_dir(&folder).unwrap();
    let gtk = format!(
        "{} Videos\n",
        basalgrim::clipboard::files::uri(&folder).unwrap()
    );
    fs::write(&c.bookmarks, &gtk).unwrap();
    let mut first = Store::open(&c.database).unwrap();
    let mut second = Store::open(&c.database).unwrap();
    assert_eq!(second.snapshot(&c).unwrap().bookmarks.len(), 1);
    first
        .apply(Command::Remove {
            section: Section::Bookmarks,
            path: folder.clone(),
        })
        .unwrap();
    assert!(second.snapshot(&c).unwrap().bookmarks.is_empty());
    drop(first);
    let mut restarted = Store::open(&c.database).unwrap();
    assert!(restarted.snapshot(&c).unwrap().bookmarks.is_empty());
    second.apply(Command::Bookmark(folder.clone())).unwrap();
    let s = restarted.snapshot(&c).unwrap();
    assert_eq!(s.bookmarks.len(), 1);
    assert_eq!(s.bookmarks[0].path, folder);
    assert!(!s.bookmarks[0].desktop);
    // Removing a personal copy must not expose the underlying import again.
    restarted
        .apply(Command::Remove {
            section: Section::Bookmarks,
            path: folder.clone(),
        })
        .unwrap();
    assert!(second.snapshot(&c).unwrap().bookmarks.is_empty());
    assert!(folder.is_dir());
    assert_eq!(fs::read_to_string(&c.bookmarks).unwrap(), gtk);
}

#[test]
fn bookmark_hiding_migrates_v1_without_losing_existing_places_or_recent_suppressions() {
    let root = tempfile::tempdir().unwrap();
    let c = config(root.path());
    let db = rusqlite::Connection::open(&c.database).unwrap();
    db.execute_batch("CREATE TABLE places(section INTEGER NOT NULL,path BLOB NOT NULL,label TEXT NOT NULL,visited INTEGER NOT NULL,PRIMARY KEY(section,path)); CREATE TABLE dismissed(path BLOB PRIMARY KEY,visited INTEGER NOT NULL); PRAGMA application_id=1112756300; PRAGMA user_version=1;").unwrap();
    db.execute(
        "INSERT INTO places VALUES(0,?1,'Saved label',0)",
        [root.path().as_os_str().as_encoded_bytes()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO dismissed VALUES(?1,9999999999)",
        [b"/tmp/forgotten".as_slice()],
    )
    .unwrap();
    drop(db);
    fs::write(
        &c.recent,
        "<xbel><bookmark href='file:///tmp/forgotten'/></xbel>",
    )
    .unwrap();
    let mut store = Store::open(&c.database).unwrap();
    let snapshot = store.snapshot(&c).unwrap();
    assert_eq!(snapshot.bookmarks[0].label, "Saved label");
    assert!(snapshot.recent.is_empty());
    store
        .apply(Command::Remove {
            section: Section::Bookmarks,
            path: root.path().into(),
        })
        .unwrap();
    assert!(store.snapshot(&c).unwrap().bookmarks.is_empty());
    let db = rusqlite::Connection::open(&c.database).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    // Bad hidden-path data is rejected before allocating unbounded row contents.
    db.execute("INSERT INTO hidden_bookmarks VALUES(zeroblob(65537))", [])
        .unwrap();
    assert!(store.snapshot(&c).is_err());
}

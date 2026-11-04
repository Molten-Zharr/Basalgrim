use basalgrim::clipboard::files::*;
use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf, sync::Arc};

#[test]
fn file_formats_roundtrip_raw_unix_names_and_all_desktop_flavours() {
    let list = FileList {
        paths: vec![
            PathBuf::from("/tmp/Картинка #1?%\n.png"),
            PathBuf::from(OsString::from_vec(b"/tmp/raw-\xff".to_vec())),
        ],
        cut: true,
    };
    let formats = encode(&list, "instance", 42).unwrap();
    assert_eq!(decode(&formats).unwrap(), Some(list.clone()));
    assert!(!formats[GNOME].ends_with(b"\n"));
    assert!(formats[URI_LIST].ends_with(b"\r\n"));
    assert_eq!(&*formats[KDE_CUT], b"1");
    assert_eq!(local_generation(&formats, "instance"), Some(42));
    assert_eq!(local_generation(&formats, "other"), None);
    for keep in [vec![GNOME], vec![URI_LIST, KDE_CUT]] {
        let subset = formats
            .iter()
            .filter(|(m, _)| keep.contains(&m.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(decode(&subset).unwrap(), Some(list.clone()));
    }
}
#[test]
fn uri_list_comments_duplicates_and_copy_default() {
    let formats = Formats::from([(
        URI_LIST.into(),
        Arc::from(b"# paths\r\nfile:///tmp/a%20b\r\n\r\nfile://localhost/tmp/a%20b\r\n".as_slice()),
    )]);
    assert_eq!(
        decode(&formats).unwrap(),
        Some(FileList {
            paths: vec!["/tmp/a b".into()],
            cut: false
        })
    );
    for uri in ["file:///tmp/a", "file:/tmp/a", "file://localhost/tmp/a"] {
        assert_eq!(path(uri).unwrap(), PathBuf::from("/tmp/a"));
    }
}
#[test]
fn malformed_remote_or_ambiguous_clipboards_are_rejected_as_a_whole() {
    for uri in [
        "https://example.org/a",
        "file://remote/tmp/a",
        "file:relative",
        "file:///tmp/a%",
        "file:///tmp/%00",
        "file:///tmp/a b",
        "file:///tmp/a?query",
        "file:///tmp/a#fragment",
    ] {
        assert!(path(uri).is_err(), "{uri}");
    }
    let list = FileList {
        paths: vec!["/tmp/a".into()],
        cut: true,
    };
    for (mime, value) in [
        (KDE_CUT, b"0".as_slice()),
        (KDE_CUT, b"wat"),
        (URI_LIST, b"file:///different\r\n"),
        (GNOME, b"cut\nfile:///tmp/a\n"),
    ] {
        let mut formats = encode(&list, "test", 1).unwrap();
        formats.insert(mime.into(), value.into());
        assert!(decode(&formats).is_err(), "{mime}");
    }
    let text = encode_text("/tmp/a".into(), "test", 1, "text");
    assert_eq!(
        decode(&text).unwrap(),
        None,
        "Plain text is never interpreted as filesystem authority"
    );
}
#[test]
fn clipboard_lists_and_payloads_are_bounded() {
    let formats = Formats::from([(URI_LIST.into(), vec![b'x'; MAX_BYTES + 1].into())]);
    assert!(decode(&formats).is_err());
    let formats = Formats::from([(
        URI_LIST.into(),
        "file:///tmp/a\n".repeat(MAX_PATHS + 1).into_bytes().into(),
    )]);
    // Count input lines as well as unique paths, so duplicate floods are bounded.
    assert!(decode(&formats).is_err());
    assert!(
        encode(
            &FileList {
                paths: vec!["relative".into()],
                cut: false
            },
            "test",
            1
        )
        .is_err()
    );
    let formats = encode(
        &FileList {
            paths: vec![],
            cut: true,
        },
        "test",
        1,
    )
    .unwrap();
    assert_eq!(decode(&formats).unwrap(), None);
}

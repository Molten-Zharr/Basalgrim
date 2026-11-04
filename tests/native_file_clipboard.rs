#![cfg(feature = "file-clipboard")]
//! Invoked only on a private Xvfb/headless compositor by the native smoke scripts.
use basalgrim::clipboard::{
    files::FileList,
    native::{Event, Incoming, Payload, Request, Runtime},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn until(runtime: &Runtime, mut accept: impl FnMut(Event) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(Instant::now() < deadline, "Native selection timed out");
        let event = runtime
            .events
            .recv_timeout(Duration::from_secs(2))
            .expect("Native clipboard response");
        if let Event::Ready(Err(error)) = &event {
            panic!("{error}")
        }
        if accept(event) {
            return;
        }
    }
}
fn write(runtime: &Runtime, id: u64, generation: u64, payload: Payload, expected: Option<u64>) {
    runtime
        .submit(Request::Write {
            id,
            generation,
            payload,
            expected,
        })
        .unwrap();
    until(runtime, |event| match event {
        Event::Written {
            id: current,
            result,
        } if id == current => {
            result.unwrap();
            true
        }
        _ => false,
    });
}
fn read(runtime: &Runtime, id: u64) -> Incoming {
    runtime.submit(Request::Read { id }).unwrap();
    let mut incoming = None;
    until(runtime, |event| match event {
        Event::Read {
            id: current,
            result,
        } if id == current => {
            incoming = Some(result.unwrap());
            true
        }
        _ => false,
    });
    incoming.unwrap()
}
#[test]
#[ignore = "requires an explicitly isolated X11 or Wayland display"]
fn native_file_clipboard_multi_format_roundtrip_and_ownership() {
    assert_eq!(
        std::env::var("BASALGRIM_CLIPBOARD_TEST_ISOLATED").as_deref(),
        Ok("1"),
        "Never run against the user's clipboard"
    );
    let a = Runtime::terminal();
    let b = Runtime::terminal();
    for runtime in [&a, &b] {
        until(runtime, |event| matches!(event, Event::Ready(Ok(()))));
    }
    let files = FileList {
        paths: (0..2400)
            .map(|i| PathBuf::from(format!("/tmp/Картинки {i}/{} #%.png", "long".repeat(20))))
            .collect(),
        cut: true,
    };
    write(&a, 1, 11, Payload::Files(files.clone()), None);
    assert!(matches!(read(&a, 2), Incoming::Local(11)));
    match read(&b, 3) {
        Incoming::External {
            files: Some(actual),
            ..
        } => assert_eq!(actual, files),
        other => panic!("{other:?}"),
    }
    let remaining = FileList {
        paths: vec![files.paths[0].clone()],
        cut: true,
    };
    write(&a, 30, 11, Payload::Files(remaining.clone()), Some(11));
    match read(&b, 31) {
        Incoming::External {
            files: Some(actual),
            ..
        } => assert_eq!(actual, remaining),
        other => panic!("{other:?}"),
    }
    write(
        &a,
        32,
        11,
        Payload::Files(FileList {
            paths: vec![],
            cut: true,
        }),
        Some(11),
    );
    assert!(matches!(
        read(&b, 33),
        Incoming::External { files: None, .. }
    ));
    // Text copying preserves the instance marker, without exposing file MIME to text consumers.
    write(&a, 4, 11, Payload::Text("/some path".into()), None);
    write(
        &a,
        5,
        11,
        Payload::Files(FileList {
            paths: vec![],
            cut: true,
        }),
        Some(11),
    );
    assert!(matches!(read(&a, 6), Incoming::Local(11)));
    assert!(matches!(
        read(&b, 7),
        Incoming::External { files: None, .. }
    ));
    let foreign = FileList {
        paths: vec!["/tmp/new-copy".into()],
        cut: false,
    };
    write(&b, 8, 22, Payload::Files(foreign.clone()), None);
    write(
        &a,
        9,
        11,
        Payload::Files(FileList {
            paths: vec![],
            cut: true,
        }),
        Some(11),
    );
    match read(&a, 10) {
        Incoming::External {
            files: Some(actual),
            ..
        } => assert_eq!(actual, foreign),
        other => panic!("{other:?}"),
    }
}

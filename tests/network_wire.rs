use basalgrim::network::{Answer, Event, Gvfs, Network, Request};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[test]
#[ignore = "requires scripts/network_fixture.py loopback servers and a private GVfs session"]
fn real_gvfs_authentication_browse_and_disconnect() {
    let uri = std::env::var("BASALGRIM_TEST_NETWORK_URI").expect("private loopback fixture URI");
    let base = PathBuf::from(std::env::var_os("BASALGRIM_TEST_NETWORK_ROOT").unwrap());
    assert_eq!(
        gio::glib::Uri::parse(&uri, gio::glib::UriFlags::NONE)
            .unwrap()
            .host()
            .as_deref(),
        Some("127.0.0.1")
    );
    let mut network = Network::default();
    network.attach(|| Box::<Gvfs>::default());
    let no_unix_permissions = uri.starts_with("dav:") || uri.starts_with("smb:");
    network.start(1, Request::Connect(uri)).unwrap();
    let end = Instant::now() + Duration::from_secs(35);
    let location = 'result: loop {
        for event in network.poll() {
            match event {
                Event::Prompt {
                    operation,
                    prompt,
                    value,
                } => {
                    let answer = match value {
                        basalgrim::network::Prompt::Password { .. } => Answer::Credentials {
                            user: "demo".into(),
                            password: Zeroizing::new("fixture-pass".into()),
                            domain: "WORKGROUP".into(),
                            anonymous: false,
                        },
                        basalgrim::network::Prompt::Question { choices, .. } => Answer::Choice(
                            choices
                                .iter()
                                .position(|c| c == "Log In Anyway")
                                .expect("explicit fixture host-key confirmation"),
                        ),
                    };
                    network.reply(operation, prompt, answer);
                }
                Event::Done { result, .. } => {
                    let location = result.unwrap().unwrap();
                    assert!(
                        location.path.starts_with(base.join("runtime/gvfs")),
                        "{location:?}"
                    );
                    assert_eq!(
                        std::fs::read_to_string(location.path.join("hello.txt")).unwrap(),
                        "independent network server\n"
                    );
                    assert_eq!(
                        std::fs::read_to_string(location.path.join("Подпапка/nested.txt")).unwrap(),
                        "nested network file\n"
                    );
                    let download = basalgrim::fs::operate(basalgrim::fs::Operation::Transfer {
                        sources: vec![location.path.join("hello.txt")],
                        destination: base.join("local"),
                        cut: false,
                    });
                    assert!(download.error.is_none(), "{download:?}");
                    assert_eq!(download.completed.len(), 1);
                    assert_eq!(
                        std::fs::read_to_string(base.join("local/hello.txt")).unwrap(),
                        "independent network server\n"
                    );
                    let source = base.join("local/upload.txt");
                    std::fs::write(&source, "upload through the common engine").unwrap();
                    let report = basalgrim::fs::operate(basalgrim::fs::Operation::Transfer {
                        sources: vec![source.clone()],
                        destination: location.path.clone(),
                        cut: false,
                    });
                    if no_unix_permissions {
                        // This server has no Unix permissions. The strict copy engine must
                        // reject the unsupported metadata operation without publishing data.
                        assert!(
                            report
                                .error
                                .as_ref()
                                .is_some_and(|e| e.contains("not supported")),
                            "{report:?}"
                        );
                        assert!(report.completed.is_empty());
                        assert!(!base.join("remote/upload.txt").exists());
                        assert!(std::fs::read_dir(base.join("remote")).unwrap().all(|e| {
                            !e.unwrap()
                                .file_name()
                                .to_string_lossy()
                                .starts_with(".basalgrim-")
                        }));
                        break 'result location;
                    }
                    assert!(report.error.is_none(), "{report:?}");
                    assert_eq!(report.completed, vec![source.clone()]);
                    assert_eq!(
                        std::fs::read_to_string(base.join("remote/upload.txt")).unwrap(),
                        "upload through the common engine"
                    );
                    // A conflicting remote destination must stay intact.
                    std::fs::write(&source, "changed local data").unwrap();
                    let conflict = basalgrim::fs::operate(basalgrim::fs::Operation::Transfer {
                        sources: vec![source.clone()],
                        destination: location.path.clone(),
                        cut: false,
                    });
                    assert!(conflict.completed.is_empty());
                    assert!(conflict.error.is_some());
                    assert_eq!(
                        std::fs::read_to_string(base.join("remote/upload.txt")).unwrap(),
                        "upload through the common engine"
                    );
                    assert!(source.exists());
                    break 'result location;
                }
                _ => {}
            }
        }
        assert!(Instant::now() < end, "GVfs connect timeout");
        std::thread::sleep(Duration::from_millis(10));
    };
    // FUSE releases can reach the daemon after read_to_string has closed its fd.
    std::thread::sleep(Duration::from_millis(500));
    network.start(2, Request::Disconnect(location)).unwrap();
    loop {
        if let Some(Event::Done { result, .. }) = network
            .poll()
            .into_iter()
            .find(|e| matches!(e, Event::Done { .. }))
        {
            assert!(result.unwrap().is_none());
            break;
        }
        assert!(
            Instant::now() < end + Duration::from_secs(10),
            "GVfs disconnect timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

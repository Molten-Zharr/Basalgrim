//! Independent GLib service, real D-Bus marshaling, no actual block device operations.
use basalgrim::devices::{Backend, Kind, UDisks, execute};
use std::{
    io::{BufRead, BufReader},
    os::unix::ffi::OsStrExt,
    path::Path,
    process::{Child, Command, Stdio},
};

struct Peer(Child);
impl Drop for Peer {
    fn drop(&mut self) {
        self.0.stdin.take();
        let _ = self.0.wait();
    }
}
fn edit(base: &Path, update: impl FnOnce(&mut serde_json::Value)) {
    let path = base.join("devices.json");
    let mut state = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    update(&mut state);
    let temp = base.join("state.tmp");
    std::fs::write(&temp, serde_json::to_vec(&state).unwrap()).unwrap();
    std::fs::rename(temp, path).unwrap();
}

#[test]
#[ignore = "requires dbus-daemon and /usr/bin/python3 with PyGObject; uses only a private bus"]
fn independent_udisks_wire_mount_errors_raw_paths_replug_and_eject() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let mut peer = Peer(
        Command::new("python3")
            .args(["scripts/devices_fixture.py", "--host"])
            .arg(base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(peer.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    assert!(address.starts_with("unix:"), "{address}");
    let mut backend = UDisks::at(address.trim().into());
    let snapshot = backend.snapshot().unwrap();
    assert_eq!(snapshot.devices().len(), 2);
    assert!(snapshot.owner.starts_with(':'));
    let key = snapshot.devices()[0].key.clone();
    let plan = snapshot.plan(&key, Kind::Open).unwrap();
    let outcome = execute(&mut backend, plan);
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert_eq!(outcome.opened, Some(base.join("USB1")));

    for (error, message) in [("DeviceBusy", "занят"), ("NotAuthorized", "прав")] {
        edit(base, |s| s["error"] = error.into());
        let plan = backend
            .snapshot()
            .unwrap()
            .plan(&key, Kind::Unmount)
            .unwrap();
        let outcome = execute(&mut backend, plan);
        assert!(outcome.error.unwrap().contains(message));
        assert!(backend.snapshot().unwrap().devices()[0].mounted());
    }
    edit(base, |s| {
        s["error"] = "".into();
        s["raw"] = true.into();
    });
    let snapshot = backend.snapshot().unwrap();
    let bytes = snapshot.volumes[0].mounts[0].as_os_str().as_bytes();
    assert!(bytes.ends_with(b"USB1-\xff"), "{bytes:?}");
    let plan = snapshot.plan(&key, Kind::Eject).unwrap();
    edit(base, |s| s["detected"] = 43.into());
    assert!(
        execute(&mut backend, plan)
            .error
            .unwrap()
            .contains("изменились")
    );
    edit(base, |s| {
        s["raw"] = false.into();
        s["mounted"] = serde_json::json!([false, true]);
        s["replace_after_mount"] = true.into();
    });
    let plan = backend.snapshot().unwrap().plan(&key, Kind::Open).unwrap();
    let outcome = execute(&mut backend, plan);
    assert!(outcome.opened.is_none());
    assert!(outcome.error.unwrap().contains("заменён"));

    let plan = backend.snapshot().unwrap().plan(&key, Kind::Eject).unwrap();
    let outcome = execute(&mut backend, plan);
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert_eq!(outcome.disconnected.len(), 2);
    assert!(backend.snapshot().unwrap().devices().is_empty());
    let calls: Vec<serde_json::Value> = std::fs::read_to_string(base.join("device-calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.last().unwrap()["method"], "PowerOff");
    assert!(
        calls
            .iter()
            .filter(|c| c["method"] == "Unmount")
            .all(|c| c["options"]["force"] == false)
    );
    assert_eq!(
        std::fs::read_to_string(base.join("USB1/device-1.txt")).unwrap(),
        "test device, ordinary folder\n"
    );
}

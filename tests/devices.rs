use basalgrim::{
    app::{App, DeviceControl, Target},
    devices::{Backend, Drive, Kind, Method, Snapshot, Volume, execute},
    input::{Action, Input},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    snapshot: Snapshot,
    calls: Vec<(String, Method)>,
    fail: Option<(String, Method)>,
    replace_after_unmount: bool,
}
#[derive(Clone)]
struct Fake(Arc<Mutex<State>>);
impl Backend for Fake {
    fn snapshot(&mut self) -> anyhow::Result<Snapshot> {
        Ok(self.0.lock().unwrap().snapshot.clone())
    }
    fn invoke(&mut self, owner: &str, path: &str, method: Method) -> anyhow::Result<()> {
        let mut s = self.0.lock().unwrap();
        assert_eq!(owner, s.snapshot.owner);
        s.calls.push((path.into(), method));
        anyhow::ensure!(
            s.fail.as_ref() != Some(&(path.into(), method)),
            "DeviceBusy"
        );
        match method {
            Method::Mount => {
                s.snapshot
                    .volumes
                    .iter_mut()
                    .find(|v| v.path == path)
                    .unwrap()
                    .mounts = vec![PathBuf::from(&s.snapshot.owner)];
            }
            Method::Unmount => {
                s.snapshot
                    .volumes
                    .iter_mut()
                    .find(|v| v.path == path)
                    .unwrap()
                    .mounts
                    .clear();
                if s.replace_after_unmount {
                    s.snapshot.drives[0].detected += 1;
                }
            }
            Method::PowerOff | Method::Eject => {
                s.snapshot.volumes.clear();
                s.snapshot.drives.clear();
            }
        }
        Ok(())
    }
}
fn fixture(root: &Path) -> Fake {
    let drive = Drive {
        path: "drive".into(),
        identity: "usb-serial".into(),
        detected: 42,
        media_detected: 43,
        label: "USB Disk".into(),
        removable: true,
        media_available: true,
        power_off: true,
        ..Default::default()
    };
    let volume = |index| Volume {
        path: format!("volume{index}"),
        drive: "drive".into(),
        device: format!("/dev/test{index}").into(),
        number: index,
        uuid: format!("uuid{index}"),
        label: format!("USB {index}"),
        filesystem: true,
        fs: "ext4".into(),
        size: 4096,
        mounts: vec![root.join(format!("disk{index}"))],
        ..Default::default()
    };
    Fake(Arc::new(Mutex::new(State {
        snapshot: Snapshot {
            owner: root.to_string_lossy().into(),
            drives: vec![drive],
            volumes: vec![volume(1), volume(2)],
        },
        ..Default::default()
    })))
}
#[test]
fn eject_unmounts_all_partitions_and_never_powers_off_after_a_busy_or_changed_volume() {
    let tmp = tempfile::tempdir().unwrap();
    for failure in [0, 1, 2] {
        let mut fake = fixture(tmp.path());
        let plan = fake
            .snapshot()
            .unwrap()
            .plan("volume1", Kind::Eject)
            .unwrap();
        {
            let mut s = fake.0.lock().unwrap();
            if failure == 1 {
                s.fail = Some(("volume2".into(), Method::Unmount));
            }
            s.replace_after_unmount = failure == 2;
        }
        let report = execute(&mut fake, plan);
        let s = fake.0.lock().unwrap();
        if failure == 0 {
            assert!(report.error.is_none());
            assert_eq!(
                s.calls,
                [
                    ("volume1".into(), Method::Unmount),
                    ("volume2".into(), Method::Unmount),
                    ("drive".into(), Method::PowerOff)
                ]
            );
            assert_eq!(report.disconnected.len(), 2);
        } else {
            assert!(report.error.is_some());
            assert_eq!(report.disconnected, [tmp.path().join("disk1")]);
            assert!(!s.calls.iter().any(|(_, m)| *m == Method::PowerOff));
        }
    }
}
#[test]
fn stale_owner_replug_new_partition_and_system_siblings_block_requests_before_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    for change in 0..5 {
        let mut fake = fixture(tmp.path());
        let plan = fake
            .snapshot()
            .unwrap()
            .plan("volume1", Kind::Eject)
            .unwrap();
        {
            let mut s = fake.0.lock().unwrap();
            match change {
                0 => s.snapshot.owner = "other-owner".into(),
                1 => s.snapshot.drives[0].detected += 1,
                2 => s.snapshot.volumes[0].uuid = "replacement".into(),
                3 => {
                    let mut v = s.snapshot.volumes[0].clone();
                    v.path = "new-partition".into();
                    s.snapshot.volumes.push(v);
                }
                _ => s.snapshot.volumes[1].system = true,
            }
        }
        assert!(execute(&mut fake, plan).error.is_some());
        assert!(fake.0.lock().unwrap().calls.is_empty());
    }
    let fake = fixture(tmp.path());
    let mut s = fake.0.lock().unwrap();
    s.snapshot.drives[0].sibling = "reader".into();
    let mut sibling = s.snapshot.drives[0].clone();
    sibling.path = "sibling".into();
    s.snapshot.drives.push(sibling);
    let mut v = s.snapshot.volumes[0].clone();
    v.drive = "sibling".into();
    v.path = "protected".into();
    v.mounts = vec!["/".into()];
    s.snapshot.volumes.push(v);
    assert!(s.snapshot.plan("volume1", Kind::Eject).is_err());
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(6);
    loop {
        app.tick();
        if app.devices.ready && app.devices.working.is_none() && !app.loading() {
            break;
        }
        assert!(
            Instant::now() < end,
            "{} {:?}",
            app.notice,
            app.devices.error
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    app.tick();
}
fn render(app: &mut App) {
    let mut term = Terminal::new(TestBackend::new(34, 14)).unwrap();
    term.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
}
fn click(app: &mut App, control: DeviceControl) {
    render(app);
    let r=app.hits.iter().find_map(|(r,t)|matches!(t,Target::DeviceControl {control:c,..} if std::mem::discriminant(c)==std::mem::discriminant(&control)).then_some(*r)).unwrap();
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
#[test]
fn shared_mouse_controls_open_unmount_and_confirm_eject_at_minimum_size() {
    let tmp = tempfile::tempdir().unwrap();
    for index in [1, 2] {
        std::fs::create_dir(tmp.path().join(format!("disk{index}"))).unwrap();
    }
    let fake = fixture(tmp.path());
    let mut app = App::new(
        tmp.path().into(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    assert!(!app.devices.ready);
    app.configure_devices(Box::new(fake.clone()));
    settle(&mut app);
    app.action(Action::Devices);
    click(&mut app, DeviceControl::Open);
    settle(&mut app);
    assert!(app.dialog.is_none());
    assert_eq!(app.tab().cwd, tmp.path().join("disk1"));
    app.action(Action::Devices);
    click(&mut app, DeviceControl::Unmount);
    settle(&mut app);
    assert_eq!(app.tab().cwd, tmp.path());
    click(&mut app, DeviceControl::Eject);
    assert!(app.devices_view.confirm.is_some());
    let calls = fake.0.lock().unwrap().calls.len();
    click(&mut app, DeviceControl::Back);
    assert_eq!(fake.0.lock().unwrap().calls.len(), calls);
    click(&mut app, DeviceControl::Eject);
    click(&mut app, DeviceControl::Confirm);
    settle(&mut app);
    assert!(fake.0.lock().unwrap().snapshot.drives.is_empty());
    assert!(tmp.path().join("disk1").is_dir());
    let mut settings = Settings::default();
    settings.normalize().unwrap();
    assert_eq!(settings.shortcut(Action::Devices), "Ctrl+Alt+M");
}
#[test]
fn stale_control_and_cancelled_open_do_not_retarget_current_tab() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("disk1")).unwrap();
    let fake = fixture(tmp.path());
    let mut app = App::new(
        tmp.path().into(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.configure_devices(Box::new(fake.clone()));
    settle(&mut app);
    app.action(Action::Devices);
    render(&mut app);
    let hits = app.hits.clone();
    app.open_devices();
    app.hits = hits;
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(
                t,
                Target::DeviceControl {
                    control: DeviceControl::Open,
                    ..
                }
            )
            .then_some(*r)
        })
        .unwrap();
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    assert!(app.devices.working.is_none());
    click(&mut app, DeviceControl::Open);
    app.cancel_dialog();
    settle(&mut app);
    assert_eq!(app.tab().cwd, tmp.path());
}

#[test]
fn device_actions_and_file_mutations_cannot_overlap_and_hot_unplug_leaves_affected_tabs() {
    let tmp = tempfile::tempdir().unwrap();
    for index in [1, 2] {
        std::fs::create_dir(tmp.path().join(format!("disk{index}"))).unwrap();
    }
    let fake = fixture(tmp.path());
    let mut app = App::new(tmp.path().join("disk1"), Settings::default());
    app.panes[1].tabs[0].cwd = tmp.path().join("disk2");
    app.reload(1, 0);
    app.configure_devices(Box::new(fake.clone()));
    settle(&mut app);
    app.action(Action::Devices);
    app.busy = true;
    click(&mut app, DeviceControl::Unmount);
    assert!(fake.0.lock().unwrap().calls.is_empty());
    app.busy = false;
    app.cancel_dialog();
    app.devices.working = Some(1234);
    app.start_operation(basalgrim::fs::Operation::Create {
        parent: tmp.path().into(),
        name: "blocked.txt".into(),
        directory: false,
    });
    assert!(!app.busy && !tmp.path().join("blocked.txt").exists());
    app.action(Action::Quit);
    assert!(!app.quit);
    app.devices.working = None;
    fake.0.lock().unwrap().snapshot.volumes[0].mounts.clear();
    app.devices.refresh();
    let end = Instant::now() + Duration::from_secs(5);
    while app.tab().cwd != tmp.path() {
        app.tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.panes[1].tabs[0].cwd, tmp.path().join("disk2"));
    assert!(fake.0.lock().unwrap().calls.is_empty());
    settle(&mut app);
}

#[test]
fn system_volumes_are_hidden_and_an_empty_removable_drive_retains_eject() {
    let tmp = tempfile::tempdir().unwrap();
    let mut snapshot = fixture(tmp.path()).snapshot().unwrap();
    snapshot.volumes[0].system = true;
    snapshot.volumes[1].ignored = true;
    assert!(snapshot.devices().iter().all(|d| d.volume.is_none()));
    assert!(snapshot.plan("drive", Kind::Eject).is_err());
    snapshot.volumes.clear();
    snapshot.drives[0].media_available = false;
    assert_eq!(snapshot.devices()[0].state(), "Нет носителя");
    assert!(snapshot.plan("drive", Kind::Open).is_err());
    assert!(snapshot.plan("drive", Kind::Eject).is_ok());
}

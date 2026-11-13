use basalgrim::{
    extensions::{self, Context, Event, Job, Selection, Service, Store},
    journal::bytes,
    operations::Cancellation,
};
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
    },
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(root: &Path, script: &str) -> (Store, std::path::PathBuf) {
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("basalgrim-plugin.json"),r#"{"api":1,"id":"fixture","name":"Проверка JSON","version":"1","executable":"run.py","commands":[{"id":"inspect","name":"Проверить файл","contexts":["file","selection"],"extensions":["json"]},{"id":"folder","name":"Папка","contexts":["background"]}],"preview":["json"],"events":["directory_changed","operation_finished"]}"#).unwrap();
    fs::write(
        source.join("run.py"),
        format!("#!/usr/bin/python3\n{script}"),
    )
    .unwrap();
    fs::set_permissions(source.join("run.py"), fs::Permissions::from_mode(0o700)).unwrap();
    (
        Store {
            root: root.join("extensions"),
        },
        source,
    )
}
const SCRIPT: &str = r#"import json,sys,os
r=json.load(sys.stdin)
root=os.fsdecode(bytes(r['context']['cwd']))
with open(os.path.join(root,'seen-'+r['hook']+'.json'),'w') as f: json.dump(r,f)
print(json.dumps({'message':'Готово','text':'CUSTOM PREVIEW\nsecond\u001b[31m'}))
"#;

#[test]
fn timed_out_preview_reaps_its_process_group() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, source) = fixture(
        tmp.path(),
        "import subprocess,time,pathlib,json,sys\nr=json.load(sys.stdin)\nchild=subprocess.Popen(['/usr/bin/sleep','60'])\npathlib.Path('child.pid').write_text(str(child.pid))\ntime.sleep(60)\n",
    );
    let p = enabled(&store, &source);
    let file = tmp.path().join("data.json");
    fs::write(&file, b"{}").unwrap();
    let start = Instant::now();
    let error =
        extensions::preview(&Arc::new(vec![p]), &file, &Cancellation::default()).unwrap_err();
    assert!(format!("{error:#}").contains("3 с"));
    assert!(start.elapsed() < Duration::from_secs(5));
    let pid = fs::read_to_string(tmp.path().join("child.pid")).unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let status = fs::read_to_string(format!("/proc/{pid}/stat"));
        if match &status {
            Err(_) => true,
            Ok(s) => s
                .split(") ")
                .nth(1)
                .is_some_and(|rest| rest.starts_with('Z')),
        } {
            break;
        }
        assert!(Instant::now() < end, "plugin child still running");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn enabled(store: &Store, source: &Path) -> extensions::Plugin {
    let cancel = Cancellation::default();
    store.install(source, &cancel).unwrap();
    let p = store.load(&cancel).unwrap().remove(0);
    assert!(!p.enabled);
    store.toggle(&p, &cancel).unwrap();
    store.load(&cancel).unwrap().remove(0)
}
fn context(root: &Path, file: &Path) -> Context {
    let m = fs::symlink_metadata(file).unwrap();
    Context {
        cwd: bytes(root),
        selected: vec![Selection {
            path: bytes(file),
            size: m.len(),
            directory: false,
            symlink: false,
            modified: m.modified().ok(),
        }],
    }
}
fn wait(service: &Service, id: u64) -> Result<String, String> {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        match service
            .events
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Event::Done { id: i, result } if i == id => return result,
            _ => {}
        }
    }
}
#[test]
fn opt_in_commands_preview_raw_paths_events_disable_remove_and_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, source) = fixture(tmp.path(), SCRIPT);
    let p = enabled(&store, &source);
    let root = tmp.path().join("files");
    fs::create_dir(&root).unwrap();
    let file = root.join(std::ffi::OsString::from_vec(
        b"raw-\xff;$(touch nope).json".to_vec(),
    ));
    fs::write(&file, b"{}").unwrap();
    let context = context(&root, &file);
    let service = Service::new(store.clone());
    assert!(service.send(1, Job::Reload, Cancellation::default()));
    wait(&service, 1).unwrap();
    assert!(service.send(
        2,
        Job::Run {
            plugin: p.clone(),
            command: "inspect".into(),
            context: context.clone()
        },
        Cancellation::default()
    ));
    assert_eq!(wait(&service, 2).unwrap(), "Готово");
    let request: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("seen-command.json")).unwrap()).unwrap();
    assert_eq!(
        request["context"]["selected"][0]["path"],
        serde_json::json!(bytes(&file))
    );
    let catalog = Arc::new(vec![p.clone()]);
    let preview = extensions::preview(&catalog, &file, &Cancellation::default())
        .unwrap()
        .unwrap();
    assert_eq!(preview.lines[0], "CUSTOM PREVIEW");
    assert!(!preview.lines.join("").contains('\x1b'));
    for hook in ["directory_changed", "operation_finished"] {
        service.notify(
            hook,
            Context {
                cwd: bytes(&root),
                selected: vec![],
            },
            Some("done".into()),
        );
        let end = Instant::now() + Duration::from_secs(4);
        while !root.join(format!("seen-{hook}.json")).exists() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    store.toggle(&p, &Cancellation::default()).unwrap();
    assert!(!store.load(&Cancellation::default()).unwrap()[0].enabled);
    assert!(extensions::preview(&catalog, &file, &Cancellation::default()).is_err());
    assert!(service.send(
        3,
        Job::Run {
            plugin: p.clone(),
            command: "inspect".into(),
            context
        },
        Cancellation::default()
    ));
    assert!(wait(&service, 3).is_err());
    store.remove(&p, &Cancellation::default()).unwrap();
    assert!(store.load(&Cancellation::default()).unwrap().is_empty());
    assert!(source.join("run.py").exists());
}
#[test]
fn tampered_packages_conflicts_symlinks_and_corrupt_registry_never_execute_or_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, source) = fixture(tmp.path(), SCRIPT);
    let cancel = Cancellation::default();
    let p = enabled(&store, &source);
    let invalid = store.root.join("broken");
    fs::create_dir(&invalid).unwrap();
    fs::write(invalid.join("basalgrim-plugin.json"), b"invalid manifest").unwrap();
    let catalog = store.load(&cancel).unwrap();
    assert_eq!(catalog.len(), 2);
    assert!(
        catalog
            .iter()
            .find(|p| p.manifest.id == "fixture")
            .unwrap()
            .enabled
    );
    let broken = catalog.iter().find(|p| p.manifest.id == "broken").unwrap();
    assert!(broken.error.is_some() && !broken.enabled);
    assert!(store.toggle(broken, &cancel).is_err());
    store.remove(broken, &cancel).unwrap();
    assert!(store.install(&source, &cancel).is_err());
    fs::write(p.directory.join("run.py"), "#!/bin/sh\nexit 0\n").unwrap();
    let changed = store.load(&cancel).unwrap().remove(0);
    assert!(!changed.enabled && changed.error.is_some());
    assert!(store.toggle(&p, &cancel).is_err());
    assert!(store.remove(&p, &cancel).is_err());
    store.remove(&changed, &cancel).unwrap();
    let outside = tmp.path().join("outside");
    fs::write(&outside, b"keep").unwrap();
    symlink(&outside, source.join("link")).unwrap();
    assert!(store.install(&source, &cancel).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    fs::remove_file(source.join("link")).unwrap();
    fs::write(store.root.join("registry.json"), b"foreign").unwrap();
    assert!(store.load(&cancel).is_err());
    assert!(store.install(&source, &cancel).is_err());
    assert_eq!(
        fs::read(store.root.join("registry.json")).unwrap(),
        b"foreign"
    );
}
#[test]
fn cancelled_and_flooding_preview_processes_are_bounded_and_stale_sources_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, source) = fixture(tmp.path(), "import time\ntime.sleep(60)\n");
    let p = enabled(&store, &source);
    let file = tmp.path().join("data.json");
    fs::write(&file, b"{}").unwrap();
    let catalog = Arc::new(vec![p.clone()]);
    let cancel = Cancellation::default();
    let other = cancel.clone();
    let started = Instant::now();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        other.cancel();
    });
    assert!(extensions::preview(&catalog, &file, &cancel).is_err());
    thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut c = context(tmp.path(), &file);
    fs::write(&file, b"changed").unwrap();
    assert!(c.validate().is_err());
    c.selected.clear();
    assert!(c.matches(&p.manifest.commands[1]));
    assert!(!c.matches(&p.manifest.commands[0]));
    let cancel = Cancellation::default();
    store.remove(&p, &cancel).unwrap();
    fs::write(
        source.join("run.py"),
        "#!/usr/bin/python3\nimport sys\nsys.stdout.write('x'*200000)\n",
    )
    .unwrap();
    let p = enabled(&store, &source);
    let result = extensions::preview(&Arc::new(vec![p]), &file, &cancel);
    assert!(result.is_err());
}

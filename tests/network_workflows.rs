use basalgrim::{
    app::{App, Dialog, EditKind, NetworkControl as C, NetworkField as F, Target},
    input::{Action, Input, Key, KeyPress},
    network::{self, Answer, Backend, Interaction, Location, Prompt},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct State {
    locations: Vec<Location>,
    accepted: usize,
    disconnects: usize,
}
struct Fake {
    state: Arc<Mutex<State>>,
    location: Location,
}
fn answer(io: &Interaction, id: u64) -> anyhow::Result<Answer> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        anyhow::ensure!(!io.cancel.is_cancelled(), "cancelled");
        if let Some((current, value)) = io.answer()
            && current == id
        {
            return Ok(value);
        }
        anyhow::ensure!(Instant::now() < end, "response timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
impl Backend for Fake {
    fn list(&mut self) -> anyhow::Result<Vec<Location>> {
        Ok(self.state.lock().unwrap().locations.clone())
    }
    fn connect(&mut self, _: &str, io: &Interaction) -> anyhow::Result<Location> {
        io.ask(
            1,
            Prompt::Question {
                message: "Unknown server key\nSHA256:test-fixture".into(),
                choices: vec!["Cancel".into(), "Trust fixture".into()],
            },
        )?;
        anyhow::ensure!(matches!(answer(io, 1)?, Answer::Choice(1)), "not trusted");
        io.ask(
            2,
            Prompt::Password {
                message: "Credentials for fixture".into(),
                user: "demo".into(),
                domain: "WORKGROUP".into(),
                need_user: true,
                need_password: true,
                need_domain: true,
                anonymous: false,
            },
        )?;
        let Answer::Credentials {
            user,
            password,
            domain,
            anonymous,
        } = answer(io, 2)?
        else {
            anyhow::bail!("wrong response");
        };
        anyhow::ensure!(
            user == "demo"
                && password.as_str() == "Пароль界e\u{301}"
                && domain == "WORKGROUP"
                && !anonymous,
            "credentials mismatch"
        );
        let mut state = self.state.lock().unwrap();
        state.accepted += 1;
        state.locations = vec![self.location.clone()];
        Ok(self.location.clone())
    }
    fn disconnect(&mut self, location: &Location, _: &Interaction) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap();
        anyhow::ensure!(state.locations.contains(location), "stale connection");
        state.disconnects += 1;
        state.locations.clear();
        Ok(())
    }
}
fn setup(root: &Path) -> (App, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let fake = Fake {
        state: state.clone(),
        location: Location {
            uri: "sftp://server/".into(),
            label: "Test server".into(),
            root: root.join("remote"),
            path: root.join("remote"),
            identity: "1".into(),
        },
    };
    std::fs::create_dir(&fake.location.root).unwrap();
    std::fs::write(fake.location.root.join("hello.txt"), "remote file").unwrap();
    let mut app = App::new(
        root.into(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    assert!(!app.network.ready);
    app.configure_network(move || Box::new(fake));
    wait(&mut app, |a| a.network.ready && !a.loading());
    (app, state)
}
fn wait(app: &mut App, ready: impl Fn(&App) -> bool) {
    let end = Instant::now() + Duration::from_secs(6);
    while !ready(app) {
        app.tick();
        assert!(
            Instant::now() < end,
            "{} {:?}",
            app.notice,
            app.network_view.error
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App) -> String {
    let mut t = Terminal::new(TestBackend::new(34, 14)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn click(app: &mut App, predicate: impl Fn(&C) -> bool) {
    render(app);
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| match t {
            Target::Network { control, .. } if predicate(control) => Some(*r),
            _ => None,
        })
        .unwrap();
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn start(app: &mut App) {
    app.action(Action::Network);
    click(app, |c| matches!(c, C::New));
    app.handle(Input::Text("sftp://server/".into()));
    click(app, |c| matches!(c, C::Submit));
    wait(app, |a| {
        matches!(a.network_view.prompt, Some((_, Prompt::Question { .. })))
    });
}

#[test]
fn minimum_mouse_auth_masks_password_preserves_file_clipboard_and_opens_ordinary_pane() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, state) = setup(tmp.path());
    start(&mut app);
    app.handle(Input::Key(KeyPress::plain(Key::Enter)));
    assert!(matches!(
        app.network_view.prompt,
        Some((1, Prompt::Question { .. }))
    ));
    let stale = app.hits.clone();
    click(&mut app, |c| matches!(c, C::Choice(1)));
    wait(&mut app, |a| {
        matches!(a.network_view.prompt, Some((2, Prompt::Password { .. })))
    });
    app.hits = stale;
    if let Some((r, _)) = app.hits.iter().find(|(_, t)| {
        matches!(
            t,
            Target::Network {
                control: C::Choice(0),
                ..
            }
        )
    }) {
        app.handle(Input::Click {
            x: r.x,
            y: r.y,
            double: false,
            toggle: false,
        });
    }
    assert!(matches!(app.network_view.prompt, Some((2, _))));
    click(&mut app, |c| matches!(c, C::Field(F::Password)));
    app.handle(Input::Text("Пароль界e\u{301}".into()));
    let image = render(&mut app);
    assert!(!image.contains("Пароль界"));
    assert!(image.contains("*********"), "{image}");
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Key(KeyPress::ctrl('c')));
    assert!(app.take_text_copy().is_none());
    click(&mut app, |c| matches!(c, C::Submit));
    wait(&mut app, |a| a.dialog.is_none() && !a.loading());
    assert_eq!(app.tab().cwd, tmp.path().join("remote"));
    assert_eq!(state.lock().unwrap().accepted, 1);
    assert!(app.network_view.password.is_empty());
    app.action(Action::CopyPath);
    assert_eq!(app.take_text_copy().unwrap().text, "sftp://server");
    app.action(Action::Network);
    click(&mut app, |c| matches!(c, C::Disconnect));
    assert!(app.network_view.confirm.is_some());
    click(&mut app, |c| matches!(c, C::ConfirmDisconnect));
    wait(&mut app, |a| a.network.working.is_none());
    assert_eq!(app.tab().cwd, tmp.path());
    assert_eq!(state.lock().unwrap().disconnects, 1);
}

#[test]
fn cancellation_rejects_late_auth_and_uri_errors_do_not_start_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, state) = setup(tmp.path());
    start(&mut app);
    app.cancel_dialog();
    wait(&mut app, |a| a.network.working.is_none());
    assert_eq!(app.tab().cwd, tmp.path());
    assert_eq!(state.lock().unwrap().accepted, 0);
    app.action(Action::Network);
    click(&mut app, |c| matches!(c, C::New));
    app.handle(Input::Text("sftp://demo:secret@server/".into()));
    click(&mut app, |c| matches!(c, C::Submit));
    assert!(app.network_view.error.as_ref().unwrap().contains("Пароль"));
    assert!(app.network.working.is_none());
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Network(F::Address),
            ..
        })
    ));
}

#[test]
fn network_uri_validation_and_raw_child_addresses() {
    for uri in [
        "sftp://server/folder",
        "smb://server/share",
        "dav://server:8080/folder",
        "davs://server/folder",
        "ftp://user@server/",
        "sftp://[::1]:22/",
        "sftp://server/name-%FF%20%23",
    ] {
        assert!(network::address(uri).is_ok(), "{uri}");
    }
    for uri in [
        "file:///etc",
        "https://server",
        "sftp:///folder",
        "sftp://u:p@server/",
        "ftp://server/?secret=x",
        "sftp://server/#fragment",
        "sftp://server/\n",
        "sftp://server/name%00bad",
        "sftp://name%0A@server/",
    ] {
        assert!(network::address(uri).is_err(), "{uri}");
    }
    use std::os::unix::ffi::OsStringExt;
    let l = Location {
        uri: "sftp://server/root/".into(),
        label: "Server".into(),
        root: "/mount".into(),
        path: "/mount".into(),
        identity: "1".into(),
    };
    let path = l
        .root
        .join(std::ffi::OsString::from_vec(b"name-\xff #".to_vec()));
    assert_eq!(
        l.address_for(&path).unwrap(),
        "sftp://server/root/name-%FF%20%23"
    );
    assert!(l.address_for(Path::new("/outside")).is_none());
}

#[test]
fn obsolete_directory_request_cannot_answer_a_server_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, state) = setup(tmp.path());
    start(&mut app);
    app.tab_mut().request += 1;
    click(&mut app, |c| matches!(c, C::Choice(1)));
    wait(&mut app, |a| a.network.working.is_none());
    assert!(app.dialog.is_none());
    assert_eq!(state.lock().unwrap().accepted, 0);
    assert_eq!(app.tab().cwd, tmp.path());
}

#[test]
fn protocol_button_keeps_prefix_when_typing_the_host() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, _) = setup(tmp.path());
    app.action(Action::Network);
    click(&mut app, |c| matches!(c, C::New));
    click(&mut app, |c| matches!(c, C::Scheme(1)));
    app.handle(Input::Text("server/share/".into()));
    assert!(matches!(&app.dialog, Some(Dialog::Edit {text, ..}) if text == "smb://server/share/"));
}

#[test]
fn connection_actions_revalidate_identity_and_wait_for_file_operations() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut app, state) = setup(tmp.path());
    let location = Location {
        uri: "sftp://server/".into(),
        label: "Server".into(),
        root: tmp.path().join("remote"),
        path: tmp.path().join("remote"),
        identity: "old".into(),
    };
    app.network.locations = vec![location.clone()];
    state.lock().unwrap().locations = vec![Location {
        identity: "new".into(),
        ..location
    }];
    app.action(Action::Network);
    click(&mut app, |c| matches!(c, C::Disconnect));
    click(&mut app, |c| matches!(c, C::ConfirmDisconnect));
    wait(&mut app, |a| a.network.working.is_none());
    assert_eq!(state.lock().unwrap().disconnects, 0);
    assert!(app.network_view.error.as_ref().unwrap().contains("stale"));
    app.busy = true;
    click(&mut app, |c| matches!(c, C::New));
    app.handle(Input::Text("ftp://server/".into()));
    click(&mut app, |c| matches!(c, C::Submit));
    assert!(app.network.working.is_none());
    assert!(
        app.network_view
            .error
            .as_ref()
            .unwrap()
            .contains("завершения")
    );
    app.busy = false;
}

#![cfg(feature = "tui")]
use basalgrim::{
    app::{App, Dialog, Target},
    input::{Action, Key, KeyPress},
    settings::Settings,
    tui_input::{
        self, Mouse,
        gpm::{Connection, Console},
    },
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    time::{Duration, Instant},
};

struct Peer {
    _root: tempfile::TempDir,
    socket: UnixStream,
    client: Connection,
}
impl Peer {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("gpmctl");
        let listener = UnixListener::bind(&path).unwrap();
        let mut client = Connection::open(&path, 7, rustix::process::getuid().as_raw()).unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert!(client.read(100, 32).unwrap().is_empty());
        let mut header = [0; 16];
        socket.read_exact(&mut header).unwrap();
        assert_eq!(u16::from_ne_bytes(header[0..2].try_into().unwrap()), 15);
        assert_eq!(&header[2..6], &[0; 4]);
        assert_eq!(u16::from_ne_bytes(header[6..8].try_into().unwrap()), 65535);
        assert_eq!(
            i32::from_ne_bytes(header[8..12].try_into().unwrap()),
            std::process::id() as i32
        );
        assert_eq!(i32::from_ne_bytes(header[12..16].try_into().unwrap()), 7);
        Self {
            _root: root,
            socket,
            client,
        }
    }
    // Explicit wire fields keep each independent peer event readable.
    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        app: &mut App,
        mouse: &mut Mouse,
        p: Position,
        kind: u32,
        buttons: u8,
        modifiers: u8,
        wheel: i16,
    ) {
        self.socket
            .write_all(&packet(
                p.x as i16 + 1,
                p.y as i16 + 1,
                kind,
                buttons,
                modifiers,
                wheel,
            ))
            .unwrap();
        for event in self.client.read(100, 32).unwrap() {
            mouse.handle(app, event);
        }
    }
    fn click(&mut self, app: &mut App, mouse: &mut Mouse, p: Position, modifiers: u8) {
        self.send(app, mouse, p, 4, 4, modifiers, 0);
        self.send(app, mouse, p, 8, 4, modifiers, 0);
    }
}
// Independent encoding of the GPM 1.20 C ABI: BBHhhhhiiihh, native endian.
fn packet(x: i16, y: i16, kind: u32, buttons: u8, modifiers: u8, wheel: i16) -> [u8; 28] {
    let mut b = [0; 28];
    b[0] = buttons;
    b[1] = modifiers;
    b[2..4].copy_from_slice(&7u16.to_ne_bytes());
    b[8..10].copy_from_slice(&x.to_ne_bytes());
    b[10..12].copy_from_slice(&y.to_ne_bytes());
    b[12..16].copy_from_slice(&kind.to_ne_bytes());
    b[26..28].copy_from_slice(&wheel.to_ne_bytes());
    b
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App) {
    Terminal::new(TestBackend::new(100, 32))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn entry(app: &App, index: usize) -> Position {
    app.hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t,Target::Entry{pane:0,index:i}if *i==index)
                .then_some(Position::new(r.x + 5, r.y))
        })
        .unwrap()
}

#[test]
fn fragmented_protocol_modifiers_wheel_margins_resize_bad_peer_and_disconnect() {
    let mut p = Peer::new();
    let data = packet(12, 8, 4 | 32, 4, 1 | 4 | 8, 0);
    for b in &data[..27] {
        p.socket.write_all(&[*b]).unwrap();
        assert!(p.client.read(34, 14).unwrap().is_empty());
    }
    p.socket.write_all(&data[27..]).unwrap();
    let events = p.client.read(34, 14).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].column, events[0].row), (11, 7));
    assert_eq!(events[0].kind, MouseEventKind::Down(MouseButton::Left));
    assert_eq!(
        events[0].modifiers,
        KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT
    );
    p.socket.write_all(&packet(12, 8, 4, 5, 0, 0)).unwrap();
    assert_eq!(
        p.client.read(34, 14).unwrap()[0].kind,
        MouseEventKind::Down(MouseButton::Right)
    );
    p.socket.write_all(&packet(12, 8, 8, 1, 0, 0)).unwrap();
    assert_eq!(
        p.client.read(34, 14).unwrap()[0].kind,
        MouseEventKind::Up(MouseButton::Right)
    );
    p.socket.write_all(&packet(1, 1, 1, 0, 0, -2)).unwrap();
    let events = p.client.read(34, 14).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.kind == MouseEventKind::ScrollDown));
    p.socket.write_all(&packet(1, 1, 4, 16, 0, 0)).unwrap();
    assert_eq!(
        p.client.read(34, 14).unwrap()[0].kind,
        MouseEventKind::ScrollUp
    );
    p.socket.write_all(&packet(35, 15, 4, 4, 0, 0)).unwrap();
    assert!(p.client.read(34, 14).unwrap().is_empty());
    p.socket.write_all(&packet(35, 15, 8, 4, 0, 0)).unwrap();
    let events = p.client.read(34, 14).unwrap();
    assert_eq!((events[0].column, events[0].row), (33, 13));
    let mut invalid = packet(1, 1, 4, 4, 0, 0);
    invalid[2] = 8;
    p.socket.write_all(&invalid).unwrap();
    assert!(p.client.read(34, 14).is_err());
    drop(p.socket);
    assert!(p.client.read(34, 14).is_err());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("peer");
    let _listener = UnixListener::bind(&path).unwrap();
    assert!(
        Connection::open(&path, 7, rustix::process::getuid().as_raw().wrapping_add(1)).is_err()
    );
    assert!(Connection::open(&path, 0, 0).is_err());
    let f = fs::File::open("/dev/ptmx").unwrap();
    assert!(Console::detect(&f, &f).is_none());
    let f = fs::File::open("/dev/null").unwrap();
    assert!(Console::detect(&f, &f).is_none());
}

#[test]
fn gpm_events_use_real_file_selection_menu_scroll_drag_and_double_click() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("folder");
    fs::create_dir(&dest).unwrap();
    for i in 0..60 {
        fs::write(temp.path().join(format!("file-{i:02}.txt")), "file").unwrap();
    }
    let mut app = App::new(
        temp.path().into(),
        Settings {
            grid: false,
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    render(&mut app);
    let mut peer = Peer::new();
    let mut mouse = Mouse::default();
    let first = entry(&app, 1);
    let second = entry(&app, 2);
    let folder = entry(&app, 0);
    peer.click(&mut app, &mut mouse, first, 4);
    peer.click(&mut app, &mut mouse, second, 4);
    assert_eq!(app.tab().marked.len(), 2);
    let marks = app.tab().marked.clone();
    let focus = app.tab().selected;
    peer.send(&mut app, &mut mouse, first, 1, 0, 0, -2);
    render(&mut app);
    assert!(app.tab().offset > 0);
    assert_eq!(app.tab().selected, focus);
    assert_eq!(app.tab().marked, marks);
    app.action(Action::Refresh);
    settle(&mut app);
    app.tab_mut().offset = 0;
    app.tab_mut().follow_selection = true;
    render(&mut app);
    peer.send(&mut app, &mut mouse, first, 4, 1, 0, 0);
    assert!(matches!(app.dialog, Some(Dialog::Commands(_))));
    assert_eq!(app.tab().marked, marks);
    app.cancel_dialog();
    render(&mut app);
    peer.send(&mut app, &mut mouse, first, 4, 4, 0, 0);
    peer.send(&mut app, &mut mouse, folder, 2, 4, 0, 0);
    assert!(app.dragging_files());
    mouse.reset(&mut app);
    assert!(!app.dragging_files());
    assert_eq!(app.tab().marked, marks);
    assert!(app.pointer.is_none());
    peer.send(&mut app, &mut mouse, first, 4, 4, 0, 0);
    peer.send(&mut app, &mut mouse, folder, 2, 4, 4, 0);
    peer.send(&mut app, &mut mouse, folder, 8, 4, 4, 0);
    settle(&mut app);
    assert!(dest.join("file-00.txt").is_file() && dest.join("file-01.txt").is_file());
    assert!(temp.path().join("file-00.txt").exists());
    render(&mut app);
    mouse.reset(&mut app);
    let folder = entry(&app, 0);
    peer.click(&mut app, &mut mouse, folder, 0);
    assert_eq!(app.tab().cwd, temp.path());
    peer.click(&mut app, &mut mouse, folder, 0);
    settle(&mut app);
    assert_eq!(app.tab().cwd, dest);
}

#[test]
fn unicode_text_and_shortcuts_are_separate_and_disabled_dnd_has_no_probe() {
    assert_eq!(
        tui_input::convert_key(KeyCode::Char('A')),
        Some(Key::Char('A'))
    );
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(temp.path().into(), Settings::default());
    settle(&mut app);
    app.action(Action::NewFile);
    for c in "Тест_ЁABC.txt".chars() {
        tui_input::key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    tui_input::key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut app);
    assert!(temp.path().join("Тест_ЁABC.txt").exists());
    for (ru, latin) in [('с', 'c'), ('м', 'v'), ('ч', 'x'), ('д', 'l'), ('й', 'q')] {
        assert_eq!(
            app.settings.action_for(KeyPress::ctrl(ru)),
            app.settings.action_for(KeyPress::ctrl(latin))
        );
    }
    let mut dnd = basalgrim::dnd::terminal::TerminalDnd::disabled();
    let mut out = vec![];
    dnd.flush(&mut out).unwrap();
    assert!(out.is_empty() && !dnd.supported());
}

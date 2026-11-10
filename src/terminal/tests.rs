use super::*;
use std::{fs, os::unix::ffi::OsStringExt};

pub(crate) fn isolated(directory: &Path) -> Terminal {
    let mut command = CommandBuilder::new("/bin/bash");
    command.args(["--noprofile", "--norc", "-i"]);
    command.env_clear();
    for (k, v) in [
        ("PATH", "/usr/bin:/bin"),
        ("TERM", "xterm-256color"),
        ("PS1", "TEST> "),
        ("LC_ALL", "C.UTF-8"),
    ] {
        command.env(k, v);
    }
    command.env("HOME", directory);
    let mut terminal = Terminal::default();
    terminal.start_command(directory.to_path_buf(), command);
    wait(&mut terminal, |t| text(t).contains("TEST>"));
    terminal
}
fn text(t: &Terminal) -> String {
    t.snapshot
        .text(Some((0, t.snapshot.cells.len().saturating_sub(1))))
}
fn wait(terminal: &mut Terminal, done: impl Fn(&Terminal) -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        terminal.poll();
        if done(terminal) {
            return;
        }
        assert!(
            Instant::now() < end,
            "terminal timeout: {}\n{}",
            terminal.snapshot.status,
            text(terminal)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn parser(rows: u16, cols: u16) -> vt100::Parser<Replies> {
    vt100::Parser::new_with_callbacks(nonzero(rows), nonzero(cols), HISTORY, Replies::default())
}

#[test]
fn vt_fragmented_unicode_colors_alternate_cursor_replies_and_bounded_history() {
    let mut p = parser(8, 40);
    for byte in "\x1b[31mПривет 界e\u{301}\x1b[0m\x1b[2;3H!".as_bytes() {
        p.process(&[*byte]);
    }
    assert_eq!(
        p.screen().cell(0, 0).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
    assert_eq!(p.screen().cell(0, 7).unwrap().contents(), "界");
    assert!(p.screen().cell(0, 8).unwrap().is_wide_continuation());
    assert_eq!(p.screen().cursor_position(), (1, 3));
    p.process(b"\x1b[6n\x1b[c");
    assert_eq!(p.callbacks().0, b"\x1b[2;4R\x1b[?1;2c");
    p.process(b"\x1b[?1049hALTERNATE\x1b[?2004h\x1b[?1h");
    assert!(p.screen().alternate_screen());
    assert!(p.screen().application_cursor());
    assert!(p.screen().bracketed_paste());
    p.process(b"\x1b[?1049l");
    assert!(p.screen().contents().contains("Привет"));
    // Long OSCs are bounded by vte's fixed 1024-byte buffer; never forwarded to the host.
    p.process(b"\x1b]52;c;");
    for _ in 0..1024 {
        p.process(&[b'A'; 1024]);
    }
    p.process(b"\x07");
    for _ in 0..(HISTORY + 500) {
        p.process(b"line\r\n");
    }
    assert_eq!(p.screen().scrollback_len(), HISTORY);
    p.screen_mut().set_scrollback(HISTORY);
    assert_eq!(p.screen().scrollback(), HISTORY);
    assert!(Snapshot::capture(&p, "", true, None).cursor.is_none());
    p.set_size(nonzero(4), nonzero(34));
    p.screen_mut().set_scrollback(HISTORY);
    let s = Snapshot::capture(&p, "", true, None);
    assert_eq!(s.cells.len(), 136);
    assert!(s.cursor.is_none());
}
#[test]
fn keys_and_paste_keep_terminal_semantics_without_executing_plain_multiline_paste() {
    assert!(paste(&vec![b'x'; INPUT_LIMIT + 1], true).is_err());
    assert_eq!(encode_key(KeyPress::ctrl('c'), false).unwrap(), b"\x03");
    assert_eq!(
        encode_key(KeyPress::plain(Key::Up), true).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        encode_key(
            KeyPress {
                ctrl: true,
                ..KeyPress::plain(Key::Left)
            },
            false
        )
        .unwrap(),
        b"\x1b[1;5D"
    );
    assert_eq!(encode_key(KeyPress::plain(Key::Tab), false).unwrap(), b"\t");
    assert_eq!(
        paste(b"one\ntwo\r\x1b[201~", false).unwrap(),
        b"one two [201~"
    );
    assert_eq!(
        paste(b"one\ntwo\x1b", true).unwrap(),
        b"\x1b[200~one\ntwo\x1b[201~"
    );
}
#[test]
fn real_pty_cwd_unicode_quoted_raw_paths_history_resize_and_auto_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = isolated(dir.path());
    let pid = t.snapshot.pid.unwrap();
    t.resize(10, 47);
    t.send(b"pwd > where; stty size > size\r");
    wait(&mut t, |_| {
        fs::read_to_string(dir.path().join("size")).is_ok_and(|s| s.trim() == "10 47")
    });
    assert_eq!(
        fs::read_to_string(dir.path().join("where")).unwrap().trim(),
        dir.path().to_str().unwrap()
    );
    let paths = vec![
        dir.path().join("проба ' $(touch WRONG).jpg"),
        dir.path()
            .join(std::ffi::OsString::from_vec(vec![b'x', 0xff])),
    ];
    t.send(b"python3 -c 'import sys,os;open(\"paths.bin\",\"wb\").write(b\"\\0\".join(os.fsencode(p) for p in sys.argv[1:]))' ");
    t.paths(&paths);
    t.send(b"\r");
    wait(&mut t, |_| dir.path().join("paths.bin").exists());
    assert_eq!(
        fs::read(dir.path().join("paths.bin")).unwrap(),
        paths
            .iter()
            .map(|p| p.as_os_str().as_bytes())
            .collect::<Vec<_>>()
            .join(&0)
    );
    assert!(!dir.path().join("WRONG").exists());
    assert!(quote_paths(&[PathBuf::from("newline\nfile")]).is_err());
    t.send(b"seq 1 2200; echo HISTORY_DONE\r");
    wait(&mut t, |t| {
        t.snapshot.history == HISTORY && text(t).contains("HISTORY_DONE")
    });
    assert_eq!(t.snapshot.history, HISTORY);
    t.scroll(100);
    wait(&mut t, |t| t.snapshot.scrollback == 100);
    t.resize(5, 34);
    wait(&mut t, |t| t.snapshot.rows == 5 && t.snapshot.cols == 34);
    t.send(b"exit 7\r");
    wait(&mut t, |t| !t.exists());
    assert_eq!(t.snapshot.exit_code, Some(7));
    assert!(!t.visible && !t.focused);
    assert!(!t.send(b"bad"));
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}
#[test]
fn interrupt_and_close_reap_foreground_job_and_reject_oversized_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = isolated(dir.path());
    let shell = t.snapshot.pid.unwrap();
    t.send(b"sh -c 'echo $$ > job; exec sleep 60'\r");
    wait(&mut t, |_| dir.path().join("job").exists());
    let job = fs::read_to_string(dir.path().join("job"))
        .unwrap()
        .trim()
        .to_string();
    t.key(KeyPress::ctrl('c'));
    wait(&mut t, |_| !Path::new(&format!("/proc/{job}")).exists());
    assert!(t.snapshot.running);
    assert!(!t.send(&vec![b'x'; INPUT_LIMIT + 1]));
    assert!(t.error.is_some());
    t.send(b"sh -c 'echo $$ > job2; exec sleep 60'\r");
    wait(&mut t, |_| dir.path().join("job2").exists());
    let job = fs::read_to_string(dir.path().join("job2"))
        .unwrap()
        .trim()
        .to_string();
    t.close();
    assert!(!Path::new(&format!("/proc/{shell}")).exists());
    // The shell reaps the terminated foreground job during the HUP grace period.
    assert!(!Path::new(&format!("/proc/{job}")).exists());
}
#[test]
fn flood_remains_responsive_and_launch_failure_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = isolated(dir.path());
    t.send(b"yes flood\r");
    wait(&mut t, |t| t.snapshot.history == HISTORY);
    t.resize(14, 34);
    wait(&mut t, |t| t.snapshot.cols == 34);
    let now = Instant::now();
    t.close();
    assert!(now.elapsed() < Duration::from_secs(2));
    t.start_command(dir.path().join("absent"), CommandBuilder::new("/bin/sh"));
    wait(&mut t, |t| !t.snapshot.running);
    assert!(t.snapshot.status.contains("Ошибка"));
    assert!(t.visible);
    assert_eq!(t.snapshot.exit_code, None);
}

#[test]
fn mouse_protocols_have_relative_coordinates_and_matching_release() {
    use vt100::MouseProtocolEncoding::*;
    assert_eq!(mouse_bytes(Sgr, 0, 2, 1, false).unwrap(), b"\x1b[<0;3;2M");
    assert_eq!(mouse_bytes(Sgr, 0, 2, 1, true).unwrap(), b"\x1b[<0;3;2m");
    assert_eq!(
        mouse_bytes(Default, 64, 2, 1, false).unwrap(),
        b"\x1b[M`#\""
    );
    assert!(mouse_bytes(Default, 0, 300, 0, false).is_none());
    assert!(std::str::from_utf8(&mouse_bytes(Utf8, 0, 300, 0, false).unwrap()).is_ok());
}

#[test]
fn exit_and_eof_close_hidden_or_selected_sessions_and_cancel_pending_paste() {
    let dir = tempfile::tempdir().unwrap();
    for (command, visible, selected) in [
        (b"exit\r".as_slice(), true, true),
        (b"\x04".as_slice(), false, false),
    ] {
        let mut t = isolated(dir.path());
        let pid = t.snapshot.pid.unwrap();
        t.send(command);
        t.visible = visible;
        t.selection = selected.then_some((0, 3));
        t.selecting = selected;
        t.paste = Some((42, t.revision));
        wait(&mut t, |t| !t.exists());
        assert_eq!(t.snapshot.exit_code, Some(0));
        assert!(!t.visible && !t.focused && !t.selecting);
        assert!(t.selection.is_none() && t.paste.is_none());
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(!t.poll(), "completion must only be reported once");
    }
}

#[test]
fn immediate_shell_exit_during_resize_closes_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    for _ in 0..8 {
        let mut t = Terminal::default();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "exit 3"]);
        command.env_clear();
        t.start_command(dir.path().to_path_buf(), command);
        t.resize(7, 32);
        wait(&mut t, |t| !t.exists());
        assert_eq!(t.snapshot.exit_code, Some(3));
        assert!(!t.visible);
    }
}

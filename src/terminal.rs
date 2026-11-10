//! One bounded PTY session. Only the worker touches the PTY or VT parser.
use crate::input::{Key, KeyPress};
use anyhow::{Context, Result, ensure};
use portable_pty::{CommandBuilder, PtySize};
use std::{
    collections::VecDeque,
    num::NonZeroU16,
    os::{
        fd::{AsRawFd, BorrowedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const HISTORY: usize = 2000;
const INPUT_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Focus,
    Hide,
    Expand,
    Paths,
    Paste,
    Copy,
    Interrupt,
    Restart,
    Close,
    Up,
    Down,
    Bottom,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<vt100::Cell>,
    pub cursor: Option<(u16, u16)>,
    pub scrollback: usize,
    pub history: usize,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub alternate: bool,
    pub mouse_mode: vt100::MouseProtocolMode,
    pub mouse_encoding: vt100::MouseProtocolEncoding,
    pub status: String,
    pub running: bool,
    /// Set only after wait reports the shell's termination, never on a launch/I/O error.
    pub exit_code: Option<u32>,
    pub pid: Option<u32>,
}
impl Snapshot {
    pub fn capture(
        parser: &vt100::Parser<Replies>,
        status: &str,
        running: bool,
        pid: Option<u32>,
    ) -> Self {
        let s = parser.screen();
        let (rows, cols) = s.size();
        Self {
            rows: rows.get(),
            cols: cols.get(),
            cells: (0..rows.get())
                .flat_map(|y| (0..cols.get()).map(move |x| s.cell(y, x).unwrap().clone()))
                .collect(),
            cursor: (!s.hide_cursor() && s.scrollback() == 0 && running)
                .then(|| s.cursor_position()),
            scrollback: s.scrollback(),
            history: s.scrollback_len(),
            application_cursor: s.application_cursor(),
            bracketed_paste: s.bracketed_paste(),
            alternate: s.alternate_screen(),
            mouse_mode: s.mouse_protocol_mode(),
            mouse_encoding: s.mouse_protocol_encoding(),
            status: status.into(),
            running,
            exit_code: None,
            pid,
        }
    }
    pub fn text(&self, selection: Option<(usize, usize)>) -> String {
        let Some((start, end)) = selection else {
            return String::new();
        };
        let (start, end) = (start.min(end), start.max(end));
        let mut lines = Vec::new();
        let mut line = String::new();
        for (index, cell) in self.cells.iter().enumerate().take(end + 1).skip(start) {
            if !cell.is_wide_continuation() {
                line.push_str(if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                });
            }
            if (index + 1) % usize::from(self.cols) == 0 {
                lines.push(line.trim_end().to_owned());
                line.clear();
            }
        }
        if !line.is_empty() {
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }
}

#[derive(Default)]
struct Commands {
    input: VecDeque<u8>,
    resize: Option<(u16, u16)>,
    scroll: isize,
    bottom: bool,
    interrupt: bool,
}
struct Runtime {
    commands: Arc<Mutex<Commands>>,
    output: Arc<Mutex<Option<Snapshot>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
pub struct Terminal {
    runtime: Option<Runtime>,
    pub snapshot: Snapshot,
    pub directory: PathBuf,
    pub visible: bool,
    pub focused: bool,
    pub expanded: bool,
    pub area: ratatui::layout::Rect,
    pub id: u64,
    pub revision: u64,
    pub selection: Option<(usize, usize)>,
    pub selecting: bool,
    pub mouse_pressed: bool,
    pub mouse_position: Option<(u16, u16)>,
    pub paste: Option<(u64, u64)>,
    pub error: Option<String>,
    size: (u16, u16),
}
impl Terminal {
    pub fn exists(&self) -> bool {
        self.runtime.is_some()
    }
    pub fn start(&mut self, directory: PathBuf) {
        self.start_command(directory, shell_command());
    }
    fn start_command(&mut self, directory: PathBuf, command: CommandBuilder) {
        self.close();
        self.id = self.id.wrapping_add(1);
        self.directory = directory.clone();
        self.visible = true;
        self.focused = true;
        self.snapshot = Snapshot {
            running: true,
            status: "Запуск оболочки…".into(),
            ..Default::default()
        };
        self.size = (12, 80);
        let commands = Arc::new(Mutex::new(Commands::default()));
        let output = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (cmd, out, cancel) = (commands.clone(), output.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("shell-pty".into())
            .spawn(move || {
                if let Err(error) = run(directory, command, cmd, &out, cancel) {
                    *out.lock().unwrap() = Some(Snapshot {
                        status: format!("Ошибка терминала: {error:#}"),
                        ..Default::default()
                    });
                }
            });
        match thread {
            Ok(thread) => {
                self.runtime = Some(Runtime {
                    commands,
                    output,
                    stop,
                    thread: Some(thread),
                })
            }
            Err(error) => {
                self.snapshot.running = false;
                self.snapshot.status = format!("Ошибка запуска: {error}");
            }
        }
    }
    pub fn invalidate_input(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.paste = None;
    }
    pub fn release_mouse(&mut self) {
        if self.mouse_pressed && self.snapshot.mouse_mode != vt100::MouseProtocolMode::None {
            let (x, y) = self.mouse_position.unwrap_or((self.area.x, self.area.y));
            let (x, y) = (
                x.saturating_sub(self.area.x)
                    .min(self.area.width.saturating_sub(1)),
                y.saturating_sub(self.area.y)
                    .min(self.area.height.saturating_sub(1)),
            );
            if let Some(bytes) = mouse_bytes(self.snapshot.mouse_encoding, 0, x, y, true) {
                self.send(&bytes);
            }
        }
        self.mouse_pressed = false;
        self.mouse_position = None;
    }
    pub fn close(&mut self) {
        self.invalidate_input();
        self.runtime = None;
        self.visible = false;
        self.focused = false;
        self.selection = None;
        self.selecting = false;
        self.mouse_pressed = false;
        self.mouse_position = None;
        self.error = None;
    }
    /// Returns true when the shell exited and its panel/session was closed.
    pub fn poll(&mut self) -> bool {
        let frame = self.runtime.as_ref().and_then(|runtime| {
            let mut output = runtime.output.lock().unwrap();
            // Selection freezes presentation, not lifecycle/error notifications.
            if self.selection.is_none() || output.as_ref().is_some_and(|frame| !frame.running) {
                output.take()
            } else {
                None
            }
        });
        if let Some(frame) = frame {
            let exited = frame.exit_code.is_some();
            self.snapshot = frame;
            if exited {
                // Drop/join the worker only after releasing its output lock.
                self.close();
                return true;
            }
        }
        false
    }
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = (rows.clamp(1, 256), cols.clamp(1, 512));
        if self.size != size {
            self.size = size;
            self.selection = None;
            self.selecting = false;
            self.invalidate_input();
            if let Some(runtime) = &self.runtime {
                runtime.commands.lock().unwrap().resize = Some(size);
            }
        }
    }
    pub fn send(&mut self, bytes: &[u8]) -> bool {
        self.invalidate_input();
        self.selection = None;
        self.selecting = false;
        let Some(runtime) = &self.runtime else {
            return false;
        };
        if !self.snapshot.running {
            return false;
        }
        let mut commands = runtime.commands.lock().unwrap();
        if bytes == b"\x03" {
            commands.input.clear();
            commands.interrupt = true;
            commands.bottom = true;
            self.error = None;
            return true;
        }
        if bytes.len() > INPUT_LIMIT.saturating_sub(commands.input.len()) {
            self.error = Some("Очередь ввода заполнена; повторите вставку меньшего текста".into());
            return false;
        }
        commands.input.extend(bytes);
        commands.bottom = true;
        self.error = None;
        true
    }
    pub fn paste_text(&mut self, text: &str) {
        self.paste_bytes(text.as_bytes());
    }
    fn paste_bytes(&mut self, bytes: &[u8]) {
        match paste(bytes, self.snapshot.bracketed_paste) {
            Ok(bytes) => {
                self.send(&bytes);
            }
            Err(error) => {
                self.invalidate_input();
                self.error = Some(error.to_string());
            }
        }
    }
    pub fn paths(&mut self, paths: &[PathBuf]) {
        match quote_paths(paths) {
            Ok(bytes) => {
                self.paste_bytes(&bytes);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    pub fn key(&mut self, key: KeyPress) {
        if let Some(bytes) = encode_key(key, self.snapshot.application_cursor) {
            self.send(&bytes);
        }
    }
    pub fn scroll(&mut self, delta: isize) {
        self.selection = None;
        self.selecting = false;
        if let Some(runtime) = &self.runtime {
            let mut cmd = runtime.commands.lock().unwrap();
            cmd.scroll = cmd
                .scroll
                .saturating_add(delta)
                .clamp(-(HISTORY as isize), HISTORY as isize);
        }
    }
    pub fn bottom(&mut self) {
        self.selection = None;
        self.selecting = false;
        if let Some(runtime) = &self.runtime {
            runtime.commands.lock().unwrap().bottom = true;
        }
    }
}

/// Quote bytes, not a lossy display string. Never submit a command or embedded controls.
pub fn quote_paths(paths: &[PathBuf]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for path in paths {
        let raw = path.as_os_str().as_bytes();
        ensure!(
            !raw.iter().any(|b| b.is_ascii_control()),
            "Путь содержит управляющий символ; вставка отменена"
        );
        ensure!(
            output.len().saturating_add(raw.len().saturating_mul(4) + 3) <= INPUT_LIMIT,
            "Слишком много путей для одной вставки"
        );
        output.push(b'\'');
        for &b in raw {
            if b == b'\'' {
                output.extend_from_slice(b"'\\''");
            } else {
                output.push(b);
            }
        }
        output.extend_from_slice(b"' ");
    }
    Ok(output)
}
fn paste(bytes: &[u8], bracketed: bool) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= INPUT_LIMIT - 12,
        "Текст слишком велик для одной вставки (максимум 256 КиБ)"
    );
    let mut output = Vec::with_capacity(bytes.len() + 12);
    if bracketed {
        output.extend_from_slice(b"\x1b[200~");
    }
    for &b in bytes {
        match b {
            b'\n' | b'\r' if !bracketed => output.push(b' '),
            b'\n' | b'\t' => output.push(b),
            b'\r' => output.push(b'\n'),
            0..=31 | 127 => {}
            _ => output.push(b),
        }
    }
    if bracketed {
        output.extend_from_slice(b"\x1b[201~");
    }
    Ok(output)
}

pub fn encode_key(key: KeyPress, application: bool) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(key.shift) + 2 * u8::from(key.alt) + 4 * u8::from(key.ctrl);
    let string = match key.key {
        Key::Char(c) => {
            if !key.ctrl && !key.alt {
                return None;
            } // Unicode arrives through Input::Text.
            let mut bytes = Vec::new();
            if key.alt {
                bytes.push(27);
            }
            if key.ctrl {
                let c = c.to_ascii_uppercase();
                bytes.push(match c {
                    ' ' | '@' | '2' => 0,
                    'A'..='_' => c as u8 & 31,
                    '?' | '8' => 127,
                    _ => return None,
                });
            } else {
                let c = if key.shift { c.to_ascii_uppercase() } else { c };
                bytes.extend_from_slice(c.to_string().as_bytes());
            }
            return Some(bytes);
        }
        Key::Enter => "\r".into(),
        Key::Escape => "\x1b".into(),
        Key::Backspace => if key.ctrl { "\x17" } else { "\x7f" }.into(),
        Key::Tab => if key.shift { "\x1b[Z" } else { "\t" }.into(),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let suffix = match key.key {
                Key::Up => 'A',
                Key::Down => 'B',
                Key::Right => 'C',
                Key::Left => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if modifier > 1 {
                format!("\x1b[1;{modifier}{suffix}")
            } else {
                format!("\x1b{}{suffix}", if application { 'O' } else { '[' })
            }
        }
        Key::F(n @ 1..=4) => {
            let suffix = char::from(b'P' + n - 1);
            if modifier > 1 {
                format!("\x1b[1;{modifier}{suffix}")
            } else {
                format!("\x1bO{suffix}")
            }
        }
        Key::Insert | Key::Delete | Key::PageUp | Key::PageDown | Key::F(5..=12) => {
            let number = match key.key {
                Key::Insert => 2,
                Key::Delete => 3,
                Key::PageUp => 5,
                Key::PageDown => 6,
                Key::F(n) => [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)],
                _ => unreachable!(),
            };
            if modifier > 1 {
                format!("\x1b[{number};{modifier}~")
            } else {
                format!("\x1b[{number}~")
            }
        }
        _ => return None,
    };
    let mut bytes = string.into_bytes();
    if key.alt
        && matches!(
            key.key,
            Key::Enter | Key::Backspace | Key::Tab | Key::Escape
        )
    {
        bytes.insert(0, 27);
    }
    Some(bytes)
}

#[derive(Default)]
pub struct Replies(Vec<u8>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if self.0.len() > 4096 || i2.is_some() {
            return;
        }
        let param = params.first().and_then(|v| v.first()).copied().unwrap_or(0);
        let reply = match (i1, c, param) {
            (None, 'n', 5) => "\x1b[0n".into(),
            (None, 'n', 6) => {
                let (y, x) = screen.cursor_position();
                format!("\x1b[{};{}R", y + 1, x + 1)
            }
            (None, 'c', 0) => "\x1b[?1;2c".into(),
            (Some(b'>'), 'c', 0) => "\x1b[>0;1;0c".into(),
            (None, 't', 18) => {
                let (rows, cols) = screen.size();
                format!("\x1b[8;{rows};{cols}t")
            }
            _ => return,
        };
        self.0.extend_from_slice(reply.as_bytes());
    }
}
fn nonzero(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap()
}
fn shell_command() -> CommandBuilder {
    let shell = std::env::var_os("SHELL")
        .filter(|s| Path::new(s).is_absolute())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-i");
    for key in [
        "COLUMNS",
        "LINES",
        "TMUX",
        "TMUX_PANE",
        "STY",
        "KITTY_WINDOW_ID",
        "KITTY_PID",
        "KITTY_LISTEN_ON",
        "WEZTERM_PANE",
        "WEZTERM_EXECUTABLE",
        "ITERM_SESSION_ID",
        "TERM_PROGRAM_VERSION",
        "LC_TERMINAL",
        "KONSOLE_VERSION",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Basalgrim");
    cmd
}
fn run(
    directory: PathBuf,
    cmd: CommandBuilder,
    commands: Arc<Mutex<Commands>>,
    output: &Mutex<Option<Snapshot>>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let pair = portable_pty::native_pty_system().openpty(PtySize {
        rows: 12,
        cols: 80,
        ..Default::default()
    })?;
    let fd = pair
        .master
        .as_raw_fd()
        .context("PTY descriptor unavailable")?;
    // SAFETY: master owns the descriptor for the whole worker, and is never moved/closed while borrowed.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
    // portable-pty falls back to HOME for a missing cwd. Pin the requested
    // directory first so neither a missing path nor a rename silently retargets it.
    let directory_fd = rustix::fs::open(
        &directory,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .with_context(|| format!("Недоступна папка {}", directory.display()))?;
    let mut cmd = cmd;
    let pinned = format!("/proc/self/fd/{}", directory_fd.as_raw_fd());
    ensure!(
        Path::new(&pinned).is_dir(),
        "Недоступен /proc/self/fd для привязки папки терминала"
    );
    cmd.cwd(pinned);
    cmd.env("PWD", &directory);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("Не удалось запустить оболочку")?;
    let pid = child.process_id();
    drop(pair.slave);
    let mut result = pump(&*pair.master, fd, &mut *child, commands, output, &stop);
    // A resize/read can race the final hangup. Prefer an observed shell exit
    // over that I/O error, but never confuse our own error cleanup with exit.
    if result.is_err()
        && let Ok(Some(status)) = child.try_wait()
    {
        *output.lock().unwrap() = Some(Snapshot {
            exit_code: Some(status.exit_code()),
            pid,
            ..Default::default()
        });
        result = Ok(());
    }
    // Close/quit stops the foreground command too, not merely the waiting shell.
    if child.try_wait().ok().flatten().is_none()
        && let Some(group) = pair.master.process_group_leader().filter(|p| *p > 1)
        && Some(group as u32) != pid
        && let Some(group) = rustix::process::Pid::from_raw(group)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    drop(pair.master); // HUP the controlling terminal and release the master.
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}
fn pump(
    master: &dyn portable_pty::MasterPty,
    fd: BorrowedFd<'_>,
    child: &mut dyn portable_pty::Child,
    commands: Arc<Mutex<Commands>>,
    output: &Mutex<Option<Snapshot>>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut parser =
        vt100::Parser::new_with_callbacks(nonzero(12), nonzero(80), HISTORY, Replies::default());
    let mut pending = VecDeque::new();
    let mut dirty = true;
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    let mut eof = false;
    let mut read = [0; 8192];
    while !stop.load(Ordering::Relaxed) {
        {
            let mut cmd = commands.lock().unwrap();
            if let Some((rows, cols)) = cmd.resize.take() {
                let scroll = parser.screen().scrollback();
                parser.set_size(nonzero(rows), nonzero(cols));
                parser.screen_mut().set_scrollback(scroll);
                master.resize(PtySize {
                    rows,
                    cols,
                    ..Default::default()
                })?;
                dirty = true;
            }
            if cmd.bottom || cmd.scroll != 0 {
                let offset = if cmd.bottom {
                    0
                } else {
                    parser
                        .screen()
                        .scrollback()
                        .saturating_add_signed(cmd.scroll)
                };
                parser.screen_mut().set_scrollback(offset);
                cmd.bottom = false;
                cmd.scroll = 0;
                dirty = true;
            }
            if cmd.interrupt {
                pending.clear();
                pending.push_back(3);
                cmd.interrupt = false;
            }
            // Never accumulate a second unbounded queue when the child stops reading.
            let available = INPUT_LIMIT.saturating_sub(pending.len());
            let count = available.min(cmd.input.len());
            pending.extend(cmd.input.drain(..count));
        }
        if !pending.is_empty() {
            match rustix::io::write(fd, pending.as_slices().0) {
                Ok(n) => {
                    pending.drain(..n);
                }
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {}
                Err(rustix::io::Errno::IO) => {
                    pending.clear();
                }
                Err(error) => return Err(error.into()),
            }
        }
        let started = Instant::now();
        // Bound per-turn work even if a program prints without pause.
        for _ in 0..16 {
            if eof {
                break;
            }
            match rustix::io::read(fd, &mut read) {
                Ok(0) | Err(rustix::io::Errno::IO) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    parser.process(&read[..n]);
                    dirty = true;
                }
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => break,
                Err(error) => return Err(error.into()),
            }
            if started.elapsed() >= Duration::from_millis(8) {
                break;
            }
        }
        let replies = std::mem::take(&mut parser.callbacks_mut().0);
        if pending.len() + replies.len() <= INPUT_LIMIT {
            pending.extend(replies);
        }
        if let Some(status) = child.try_wait()? {
            let mut frame = Snapshot::capture(
                &parser,
                &format!("Оболочка завершена · код {}", status.exit_code()),
                false,
                child.process_id(),
            );
            frame.exit_code = Some(status.exit_code());
            *output.lock().unwrap() = Some(frame);
            return Ok(());
        }
        if dirty && last_frame.elapsed() >= Duration::from_millis(30) {
            *output.lock().unwrap() =
                Some(Snapshot::capture(&parser, "", true, child.process_id()));
            dirty = false;
            last_frame = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;

/// Mouse coordinates are relative to the embedded viewport, never the outer terminal.
pub fn mouse_bytes(
    encoding: vt100::MouseProtocolEncoding,
    button: u8,
    x: u16,
    y: u16,
    release: bool,
) -> Option<Vec<u8>> {
    use vt100::MouseProtocolEncoding::*;
    let (x, y) = (x + 1, y + 1);
    match encoding {
        Sgr => {
            Some(format!("\x1b[<{button};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes())
        }
        Default if x <= 223 && y <= 223 => Some(vec![
            27,
            b'[',
            b'M',
            32 + if release { 3 } else { button },
            32 + x as u8,
            32 + y as u8,
        ]),
        Utf8 => Some(
            format!(
                "\x1b[M{}{}{}",
                char::from(32 + if release { 3 } else { button }),
                char::from_u32(32 + u32::from(x))?,
                char::from_u32(32 + u32::from(y))?
            )
            .into_bytes(),
        ),
        _ => None,
    }
}

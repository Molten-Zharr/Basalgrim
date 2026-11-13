//! Linux GPM 1.20 socket ABI (native endian), without libgpm's global state,
//! signal handlers or stdin reads. See upstream src/headers/gpm.h and liblow.c.
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use rustix::{
    fs::{fstat, major, minor},
    ioctl::{Getter, ioctl},
    net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
        sockopt::socket_peercred,
    },
};
use std::{
    io::{self, Read, Write},
    os::fd::{AsFd, OwnedFd},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

#[repr(C)]
#[derive(Clone, Copy)]
struct VtState {
    active: u16,
    signal: u16,
    state: u16,
}

pub struct Console {
    fd: OwnedFd,
    vc: u16,
}
impl Console {
    /// TERM, environment and a tty-like path alone do not prove a real VC.
    pub fn detect(input: &impl AsFd, output: &impl AsFd) -> Option<Self> {
        let a = fstat(input).ok()?;
        let b = fstat(output).ok()?;
        let vc = minor(a.st_rdev);
        if major(a.st_rdev) != 4 || !(1..=63).contains(&vc) || a.st_rdev != b.st_rdev {
            return None;
        }
        // Linux kd.h: KDGETMODE writes an int; vt.h: VT_GETSTATE writes vt_stat.
        let mode = unsafe { ioctl(input, Getter::<0x4b3b, i32>::new()) }.ok()?;
        let _state = unsafe { ioctl(input, Getter::<0x5603, VtState>::new()) }.ok()?;
        if mode != 0 {
            return None;
        }
        Some(Self {
            fd: rustix::io::fcntl_dupfd_cloexec(input, 0).ok()?,
            vc: vc as u16,
        })
    }
    fn active(&self) -> bool {
        let state = unsafe { ioctl(&self.fd, Getter::<0x5603, VtState>::new()) };
        let mode = unsafe { ioctl(&self.fd, Getter::<0x4b3b, i32>::new()) };
        let foreground = unsafe { ioctl(&self.fd, Getter::<0x540f, i32>::new()) };
        state.is_ok_and(|s| s.active == self.vc)
            && mode == Ok(0)
            && foreground == Ok(rustix::process::getpgrp().as_raw_nonzero().get())
    }
}

/// A bounded nonblocking connection. Public for independent protocol peers in
/// tests; the application only connects /dev/gpmctl for a verified Console.
pub struct Connection {
    socket: UnixStream,
    vc: u16,
    header: [u8; 16],
    sent: usize,
    packet: [u8; 28],
    received: usize,
    held: u8,
    deadline: Instant,
}
impl Connection {
    pub fn open(path: &Path, vc: u16, peer_uid: u32) -> io::Result<Self> {
        if !(1..=63).contains(&vc) {
            return Err(io::Error::other("Invalid virtual console"));
        }
        let fd = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            None,
        )?;
        // AF_UNIX connect is immediate or EAGAIN (full backlog). Do not wait on
        // the UI thread; reconnect later when the daemon can accept clients.
        connect(&fd, &SocketAddrUnix::new(path)?)?;
        if socket_peercred(&fd)?.uid.as_raw() != peer_uid {
            return Err(io::Error::other("Unexpected GPM daemon owner"));
        }
        let mut header = [0; 16];
        header[0..2].copy_from_slice(&15u16.to_ne_bytes()); // move, drag, down, up
        // defaultMask=0: consume events, do not also paste console selection.
        // minMod=0, maxMod=all: include Ctrl/Shift and preserve user keymap.
        header[6..8].copy_from_slice(&u16::MAX.to_ne_bytes());
        header[8..12].copy_from_slice(&(std::process::id() as i32).to_ne_bytes());
        header[12..16].copy_from_slice(&i32::from(vc).to_ne_bytes());
        Ok(Self {
            socket: fd.into(),
            vc,
            header,
            sent: 0,
            packet: [0; 28],
            received: 0,
            held: 0,
            deadline: Instant::now() + Duration::from_secs(1),
        })
    }
    pub fn read(&mut self, width: u16, height: u16) -> io::Result<Vec<MouseEvent>> {
        if self.sent < 16 {
            if Instant::now() > self.deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "GPM handshake timed out",
                ));
            }
            match self.socket.write(&self.header[self.sent..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "GPM closed")),
                Ok(n) => self.sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(vec![]),
                Err(e) => return Err(e),
            }
            if self.sent < 16 {
                return Ok(vec![]);
            }
        }
        let mut events = Vec::new();
        // Bound time and allocation even if a peer continuously sends packets.
        for _ in 0..64 {
            match self.socket.read(&mut self.packet[self.received..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "GPM disconnected",
                    ));
                }
                Ok(n) => {
                    self.received += n;
                    if self.received == 28 {
                        decode(
                            &self.packet,
                            self.vc,
                            width,
                            height,
                            &mut self.held,
                            &mut events,
                        )?;
                        self.received = 0;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(events)
    }
}
fn decode(
    p: &[u8; 28],
    vc: u16,
    width: u16,
    height: u16,
    held: &mut u8,
    out: &mut Vec<MouseEvent>,
) -> io::Result<()> {
    let short = |i| i16::from_ne_bytes([p[i], p[i + 1]]);
    let kind = u32::from_ne_bytes(p[12..16].try_into().unwrap());
    if u16::from_ne_bytes([p[2], p[3]]) != vc
        || !matches!(kind & 15, 1 | 2 | 4 | 8)
        || kind & !0x7ff != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid GPM event",
        ));
    }
    let (x, y) = (short(8), short(10));
    if width == 0 || height == 0 || x < 0 || y < 0 {
        return Ok(());
    }
    let outside = x == 0 || y == 0 || x as u16 > width || y as u16 > height;
    if outside && kind & 4 != 0 {
        return Ok(());
    } // never click a clamped edge control
    let mut modifiers = KeyModifiers::empty();
    if p[1] & (1 | 16 | 32) != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if p[1] & (4 | 64 | 128) != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    if p[1] & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    let event = |kind| MouseEvent {
        kind,
        column: (x.saturating_sub(1) as u16).min(width - 1),
        row: (y.saturating_sub(1) as u16).min(height - 1),
        modifiers,
    };
    let wheel = short(26);
    // Older mouse drivers can encode the wheel as virtual buttons instead.
    let wheel = if wheel != 0 {
        wheel
    } else if kind & 8 == 0 && p[0] & 16 != 0 {
        1
    } else if kind & 8 == 0 && p[0] & 32 != 0 {
        -1
    } else {
        0
    };
    if wheel != 0 {
        for _ in 0..wheel.unsigned_abs().min(8) {
            out.push(event(if wheel > 0 {
                MouseEventKind::ScrollUp
            } else {
                MouseEventKind::ScrollDown
            }));
        }
        return Ok(());
    }
    // Down contains all currently held buttons; Up contains only released
    // buttons. In particular, right-down while dragging left must cancel/open
    // the shared secondary-click action, not synthesize another left-down.
    let physical = p[0] & 7;
    let buttons = match kind & 15 {
        4 => {
            let changed = physical & !*held;
            *held = physical;
            if changed == 0 { physical } else { changed }
        }
        8 => {
            *held &= !physical;
            physical
        }
        _ => {
            *held = physical;
            physical
        }
    };
    let button = if buttons & 4 != 0 {
        Some(MouseButton::Left)
    } else if buttons & 1 != 0 {
        Some(MouseButton::Right)
    } else if buttons & 2 != 0 {
        Some(MouseButton::Middle)
    } else {
        None
    };
    let kind = match (kind & 15, button) {
        (1, _) => MouseEventKind::Moved,
        (2, Some(b)) => MouseEventKind::Drag(b),
        (4, Some(b)) => MouseEventKind::Down(b),
        (8, Some(b)) => MouseEventKind::Up(b),
        _ => return Ok(()),
    };
    out.push(event(kind));
    Ok(())
}

pub struct Runtime {
    console: Console,
    connection: Option<Connection>,
    next_attempt: Instant,
    active: bool,
    last_error: String,
}
#[derive(Default)]
pub struct Update {
    pub events: Vec<MouseEvent>,
    pub reset: bool,
    pub notice: Option<String>,
}
impl Runtime {
    pub fn new(console: Console) -> Self {
        Self {
            console,
            connection: None,
            next_attempt: Instant::now(),
            active: false,
            last_error: String::new(),
        }
    }
    pub fn connected(&self) -> bool {
        self.connection.is_some()
    }
    pub fn poll(&mut self, width: u16, height: u16) -> Update {
        self.poll_active(
            self.console.active(),
            Path::new("/dev/gpmctl"),
            0,
            width,
            height,
        )
    }
    fn poll_active(
        &mut self,
        active: bool,
        endpoint: &Path,
        uid: u32,
        width: u16,
        height: u16,
    ) -> Update {
        let mut update = Update::default();
        if self.active != active {
            update.reset = true;
            self.connection = None;
            self.next_attempt = Instant::now();
            self.active = active;
        }
        if !active {
            return update;
        }
        if self.connection.is_none() && Instant::now() >= self.next_attempt {
            self.next_attempt = Instant::now() + Duration::from_secs(3);
            match Connection::open(endpoint, self.console.vc, uid) {
                Ok(c) => {
                    self.connection = Some(c);
                    self.last_error.clear();
                }
                Err(e) => self.report(e, &mut update),
            }
        }
        if let Some(c) = &mut self.connection {
            match c.read(width, height) {
                Ok(events) => update.events = events,
                Err(e) => {
                    self.connection = None;
                    self.next_attempt = Instant::now() + Duration::from_secs(3);
                    update.reset = true;
                    self.report(e, &mut update);
                }
            }
        }
        update
    }
    fn report(&mut self, e: io::Error, update: &mut Update) {
        let message = format!("Мышь GPM недоступна: {e}. Клавиатура работает");
        if self.last_error != message {
            self.last_error = message.clone();
            update.notice = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, os::unix::net::UnixListener};

    #[test]
    fn inactive_vt_disconnect_and_daemon_restart_discard_old_gestures() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gpmctl");
        let listener = UnixListener::bind(&path).unwrap();
        let mut runtime = Runtime::new(Console {
            fd: File::open("/dev/null").unwrap().into(),
            vc: 3,
        });
        let uid = rustix::process::getuid().as_raw();
        assert!(!runtime.poll_active(false, &path, uid, 80, 25).reset);
        assert!(runtime.poll_active(true, &path, uid, 80, 25).reset);
        let (mut peer, _) = listener.accept().unwrap();
        let mut header = [0; 16];
        peer.read_exact(&mut header).unwrap();
        assert_eq!(i32::from_ne_bytes(header[12..16].try_into().unwrap()), 3);
        peer.write_all(&[4, 0, 3, 0, 0]).unwrap(); // partial old press
        assert!(
            runtime
                .poll_active(true, &path, uid, 80, 25)
                .events
                .is_empty()
        );
        assert!(runtime.poll_active(false, &path, uid, 80, 25).reset);
        assert!(!runtime.connected());
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
        assert!(runtime.poll_active(true, &path, uid, 34, 14).reset);
        let (mut peer, _) = listener.accept().unwrap();
        peer.read_exact(&mut header).unwrap();
        drop(peer);
        let update = runtime.poll_active(true, &path, uid, 34, 14);
        assert!(update.reset && update.events.is_empty() && update.notice.is_some());
        assert!(!runtime.connected());
        assert!(
            runtime
                .poll_active(true, &path, uid, 34, 14)
                .notice
                .is_none()
        );
        runtime.next_attempt = Instant::now();
        assert!(
            runtime
                .poll_active(true, &path, uid, 34, 14)
                .events
                .is_empty()
        );
        assert!(runtime.connected());
        let (mut peer, _) = listener.accept().unwrap();
        peer.read_exact(&mut header).unwrap();
        assert_eq!(header[0..8], [15, 0, 0, 0, 0, 0, 255, 255]);
    }
}

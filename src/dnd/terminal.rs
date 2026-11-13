//! Kitty OSC 72 transport. Negotiate explicitly; ordinary pasted text never authorizes file operations.
use super::{DropAction, Event, Request};
use crate::clipboard::files::{MAX_BYTES, URI_LIST};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ratatui::layout::Position;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, IsTerminal, Write},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
struct Packet {
    meta: BTreeMap<char, String>,
    payload: Vec<u8>,
}
impl Packet {
    fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= 8192, "OSC packet too large");
        let split = bytes.iter().position(|b| *b == b';').unwrap_or(bytes.len());
        ensure!(split <= 512, "OSC metadata too large");
        let mut meta = BTreeMap::new();
        for pair in std::str::from_utf8(&bytes[..split])?.split(':') {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("Malformed OSC metadata"))?;
            ensure!(
                key.len() == 1 && key.is_ascii() && value.is_ascii() && !value.is_empty(),
                "Malformed OSC field"
            );
            ensure!(
                meta.insert(key.chars().next().unwrap(), value.into())
                    .is_none(),
                "Duplicate OSC field"
            );
        }
        let payload = bytes.get(split + 1..).unwrap_or_default().to_vec();
        ensure!(payload.len() <= 4096, "OSC payload too large");
        Ok(Self { meta, payload })
    }
    fn num(&self, key: char) -> Option<i64> {
        let n = self.meta.get(&key)?.parse::<i64>().ok()?;
        (n >= i64::from(i32::MIN) && n <= i64::from(u32::MAX)).then_some(n)
    }
    fn kind(&self) -> Option<char> {
        self.meta.get(&'t').filter(|s| s.len() == 1)?.chars().next()
    }
}
struct Incoming {
    id: u64,
    index: usize,
    bytes: Vec<u8>,
    reading: Option<Instant>,
    dropped: bool,
}
struct Outgoing {
    id: u64,
    bytes: Arc<[u8]>,
    started: bool,
}
pub struct TerminalDnd {
    query: u32,
    wire: u32,
    enabled: bool,
    deadline: Instant,
    fragment: Option<Packet>,
    mimes: Vec<String>,
    incoming: Option<Incoming>,
    outgoing: Option<Outgoing>,
    serial: u64,
    gesture: Option<Position>,
    events: Vec<Event>,
    output: VecDeque<String>,
}
impl Default for TerminalDnd {
    fn default() -> Self {
        Self::new()
    }
}
impl TerminalDnd {
    pub fn new() -> Self {
        Self::with_probe(true)
    }
    pub fn disabled() -> Self {
        Self::with_probe(false)
    }
    fn with_probe(probe: bool) -> Self {
        let mut bytes = [0; 4];
        let local =
            std::env::var_os("SSH_CONNECTION").is_none() && std::env::var_os("SSH_TTY").is_none();
        let random = rustix::rand::getrandom(&mut bytes, rustix::rand::GetRandomFlags::empty())
            .ok()
            == Some(4);
        let id = (u32::from_ne_bytes(bytes) & 0x3fff_ffff).max(1);
        let mut this = Self {
            query: id,
            wire: id,
            enabled: false,
            deadline: Instant::now() + Duration::from_millis(600),
            fragment: None,
            mimes: vec![],
            incoming: None,
            outgoing: None,
            serial: 0,
            gesture: None,
            events: vec![],
            output: VecDeque::new(),
        };
        if probe && local && random {
            this.emit(&format!("t=q:i={id}"), "");
        }
        this
    }
    pub fn supported(&self) -> bool {
        self.enabled
    }
    pub fn take_gesture(&mut self) -> Option<Position> {
        self.gesture.take()
    }
    pub fn events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
    fn emit(&mut self, meta: &str, payload: &str) {
        self.output
            .push_back(format!("\x1b]72;{meta};{payload}\x1b\\"));
    }
    fn command(&mut self, meta: &str, payload: &str) {
        self.emit(&format!("{meta}:i={}", self.wire), payload);
    }
    fn register(&mut self) {
        self.command("t=a", URI_LIST);
        self.command("t=o:x=1", "");
    }
    fn rotate(&mut self) {
        if self.incoming.is_none() && self.outgoing.is_none() {
            self.wire = self.wire.wrapping_add(1).max(1);
            self.fragment = None;
            self.mimes.clear();
            self.register();
        }
    }
    pub fn flush(&mut self, output: &mut impl Write) -> io::Result<()> {
        // Bound one UI iteration; preserve command order and chunk framing.
        let mut sent = 0;
        while let Some(packet) = self.output.front() {
            output.write_all(packet.as_bytes())?;
            sent += packet.len();
            self.output.pop_front();
            if sent >= 64 * 1024 {
                break;
            }
        }
        output.flush()
    }
    pub fn tick(&mut self) {
        if self
            .incoming
            .as_ref()
            .and_then(|i| i.reading)
            .is_some_and(|t| t.elapsed() > Duration::from_secs(5))
            && let Some(i) = &mut self.incoming
        {
            i.reading = None;
            self.events.push(Event::Data {
                id: i.id,
                result: Err("Терминал не передал файлы вовремя".into()),
            });
        }
    }
    pub fn input(&mut self, bytes: &[u8]) {
        let Ok(mut packet) = Packet::parse(bytes) else {
            return;
        };
        if !self.enabled {
            if Instant::now() < self.deadline
                && packet.kind() == Some('q')
                && packet.num('i') == Some(i64::from(self.query))
            {
                self.enabled = true;
                self.register();
                self.events.push(Event::Ready(Ok(())));
            }
            return;
        }
        if packet.num('i') != Some(i64::from(self.wire)) {
            return;
        }
        let more = match packet.num('m') {
            None | Some(0) => false,
            Some(1) => true,
            _ => return,
        };
        let final_empty = packet.payload.is_empty();
        if let Some(mut first) = self.fragment.take() {
            if first.payload.len() + packet.payload.len() > MAX_BYTES.div_ceil(3) * 4 + 4096 {
                self.abort("Терминальное перетаскивание превышает 4 MiB");
                return;
            }
            first.payload.append(&mut packet.payload);
            packet = first;
        }
        if more {
            self.fragment = Some(packet);
            return;
        }
        self.packet(packet, final_empty);
    }
    fn abort(&mut self, error: &str) {
        if let Some(i) = self.incoming.take() {
            self.command("t=r:o=0", "");
            self.events.push(Event::Data {
                id: i.id,
                result: Err(error.into()),
            });
        }
        if let Some(o) = self.outgoing.take() {
            self.command("t=E:y=-1", "");
            self.events.push(Event::Sent {
                id: o.id,
                result: Err(error.into()),
            });
        }
        self.fragment = None;
        self.rotate();
    }
    fn packet(&mut self, p: Packet, final_empty: bool) {
        match p.kind() {
            Some('m' | 'M') => {
                let Some(x) = p.num('x') else {
                    return;
                };
                let Some(y) = p.num('y') else {
                    return;
                };
                if x == -1 && y == -1 {
                    if self.incoming.as_ref().is_some_and(|i| !i.dropped)
                        && let Some(i) = self.incoming.take()
                    {
                        self.events.push(Event::Leave { id: i.id });
                    }
                    return;
                }
                let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
                    return;
                };
                if !p.payload.is_empty() {
                    let Ok(list) = std::str::from_utf8(&p.payload) else {
                        return;
                    };
                    let values = list.split_ascii_whitespace().collect::<Vec<_>>();
                    if values.len() > 64 || values.iter().any(|m| m.len() > 256) {
                        self.abort("Слишком много MIME-типов");
                        return;
                    }
                    self.mimes = values.into_iter().map(str::to_owned).collect();
                } else if p.kind() == Some('M') {
                    self.abort("Терминал не указал MIME-типы сброса");
                    return;
                }
                if self.incoming.is_none() {
                    self.serial = self.serial.wrapping_add(1);
                    self.incoming = Some(Incoming {
                        id: self.serial.wrapping_mul(2) | 1,
                        index: 0,
                        bytes: vec![],
                        reading: None,
                        dropped: false,
                    });
                }
                let incoming = self.incoming.as_mut().unwrap();
                if incoming.dropped {
                    return;
                }
                incoming.index = self
                    .mimes
                    .iter()
                    .position(|m| m == URI_LIST)
                    .map_or(0, |n| n + 1);
                let allowed = if incoming.index == 0 {
                    0
                } else {
                    p.num('o').unwrap_or(0) as u32 & 3
                };
                self.events.push(Event::Hover {
                    id: incoming.id,
                    x: f64::from(x),
                    y: f64::from(y),
                    physical: false,
                    allowed,
                    preferred: if allowed == 1 {
                        DropAction::Copy
                    } else {
                        DropAction::Move
                    },
                });
                if p.kind() == Some('M') {
                    incoming.dropped = true;
                    self.events.push(Event::Drop { id: incoming.id });
                }
            }
            Some('r') => {
                let Some(i) = self.incoming.as_mut().filter(|i| {
                    i.dropped && i.reading.is_some() && p.num('x') == Some(i.index as i64)
                }) else {
                    return;
                };
                if p.num('X').is_some_and(|x| x != 0)
                    || p.num('y').is_some()
                    || p.num('Y').is_some()
                {
                    self.abort("Перетаскивание с другого компьютера не поддерживается");
                    return;
                }
                let decoded = STANDARD.decode(&p.payload).or_else(|_| {
                    base64::engine::general_purpose::STANDARD_NO_PAD.decode(&p.payload)
                });
                match decoded {
                    Ok(bytes) if i.bytes.len() + bytes.len() <= MAX_BYTES => {
                        i.bytes.extend_from_slice(&bytes)
                    }
                    _ => {
                        self.abort("Некорректные данные перетаскивания");
                        return;
                    }
                }
                if final_empty {
                    i.reading = None;
                    let result =
                        super::decode(Arc::from(i.bytes.as_slice())).map_err(|e| e.to_string());
                    self.events.push(Event::Data { id: i.id, result });
                }
            }
            Some('R') => self.abort("Терминал отклонил чтение перетаскивания"),
            Some('o') if self.outgoing.is_none() => {
                if let (Some(x), Some(y)) = (p.num('x'), p.num('y'))
                    && let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y))
                {
                    self.gesture = Some(Position::new(x, y));
                }
            }
            Some('E') => {
                if p.payload == b"OK" {
                    if let Some(outgoing) = &mut self.outgoing {
                        outgoing.started = true;
                    }
                } else {
                    self.abort("Терминал отменил перетаскивание");
                }
            }
            Some('e') if self.outgoing.as_ref().is_some_and(|o| o.started) => match p.num('x') {
                Some(4) => {
                    if let Some(o) = self.outgoing.take() {
                        self.events.push(Event::Sent {
                            id: o.id,
                            result: if p.num('y') == Some(1) {
                                Err("Перетаскивание отменено".into())
                            } else {
                                Ok(())
                            },
                        });
                        self.rotate();
                    }
                }
                Some(5) => {
                    if p.num('y') == Some(0) {
                        if let Some(o) = &self.outgoing {
                            let bytes = o.bytes.clone();
                            self.send_bytes("t=e:y=0", &bytes);
                        }
                    } else {
                        self.command("t=E:y=-1", "EINVAL");
                    }
                }
                _ => {}
            },
            Some('k') => self.abort("Передача содержимого на другой компьютер не поддерживается"),
            _ => {}
        }
    }
    fn send_bytes(&mut self, meta: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(3072) {
            self.command(&format!("{meta}:m=1"), &STANDARD.encode(chunk));
        }
        self.command(&format!("{meta}:m=0"), "");
    }
    pub fn request(&mut self, request: Request) {
        if !self.enabled {
            return;
        }
        match request {
            Request::Start { id, paths, copy } => match super::encode(paths) {
                Ok(bytes) => {
                    self.command(if copy { "t=o:o=1" } else { "t=o:o=3" }, URI_LIST);
                    self.send_bytes("t=p:x=0", &bytes);
                    self.command("t=P:x=-1", "");
                    self.outgoing = Some(Outgoing {
                        id,
                        bytes,
                        started: false,
                    });
                }
                Err(e) => self.events.push(Event::Sent {
                    id,
                    result: Err(e.to_string()),
                }),
            },
            Request::Accept { id, action } => {
                if self
                    .incoming
                    .as_ref()
                    .is_some_and(|i| i.id == id && !i.dropped)
                {
                    self.command(
                        &format!("t=m:o={}", action.map_or(0, DropAction::bit)),
                        URI_LIST,
                    );
                }
            }
            Request::Read { id } => {
                if let Some(i) = &mut self.incoming
                    && i.id == id
                    && i.dropped
                    && i.index > 0
                    && i.reading.is_none()
                {
                    i.reading = Some(Instant::now());
                    let index = i.index;
                    self.command(&format!("t=r:x={index}"), "");
                }
            }
            Request::Finish { id, action } => {
                if self.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.command(&format!("t=r:o={}", action.map_or(0, DropAction::bit)), "");
                    self.incoming = None;
                    self.rotate();
                }
            }
            Request::Cancel { id } => {
                if self.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.command("t=r:o=0", "");
                    self.incoming = None;
                }
                if self.outgoing.as_ref().is_some_and(|o| o.id == id) {
                    self.command("t=E:y=-1", "");
                    self.outgoing = None;
                }
                self.rotate();
            }
        }
    }
}
impl Drop for TerminalDnd {
    fn drop(&mut self) {
        if self.enabled && io::stdout().is_terminal() {
            let mut output = io::stdout().lock();
            if self.incoming.is_some() {
                let _ = write!(output, "\x1b]72;t=r:o=0:i={}\x1b\\", self.wire);
            }
            if self.outgoing.is_some() {
                let _ = write!(output, "\x1b]72;t=E:y=-1:i={}\x1b\\", self.wire);
            }
            let _ = write!(
                output,
                "\x1b]72;t=A:i={}\x1b\\\x1b]72;t=o:x=2:i={}\x1b\\",
                self.wire, self.wire
            );
            let _ = output.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn terminal() -> TerminalDnd {
        let mut t = TerminalDnd::new();
        t.query = 71;
        t.wire = 71;
        t.input(b"t=q:i=70;");
        assert!(!t.supported());
        t.input(b"t=q:i=71;");
        assert!(t.supported());
        t.events();
        t.output.clear();
        t
    }
    fn drop(t: &mut TerminalDnd) -> u64 {
        let wire = t.wire;
        t.input(format!("t=m:i={wire}:x=8:y=9:o=3;text/uri-list").as_bytes());
        let id = match t.events().pop().unwrap() {
            Event::Hover { id, .. } => id,
            _ => panic!(),
        };
        t.request(Request::Accept {
            id,
            action: Some(DropAction::Move),
        });
        t.input(format!("t=M:i={wire}:x=8:y=9:o=3;text/uri-list").as_bytes());
        assert!(
            t.events()
                .iter()
                .any(|e| matches!(e,Event::Drop {id:i} if *i==id))
        );
        t.request(Request::Read { id });
        id
    }
    #[test]
    fn negotiated_chunked_uri_data_waits_for_drop_and_keeps_completion_until_operation_finishes() {
        let mut t = terminal();
        let id = drop(&mut t);
        t.input(b"t=r:i=71:x=1:m=1;ZmlsZTovLy90bXAvYQ0K");
        assert!(t.events().is_empty());
        t.input(b"i=71:m=0;");
        match t.events().pop().unwrap() {
            Event::Data { id: i, result } => {
                assert_eq!(id, i);
                assert_eq!(result.unwrap(), vec![std::path::PathBuf::from("/tmp/a")]);
            }
            _ => panic!(),
        }
        assert!(t.incoming.is_some());
        t.request(Request::Finish {
            id,
            action: Some(DropAction::Move),
        });
        assert!(t.output.iter().any(|s| s.contains("t=r:o=2:i=71")));
        assert_ne!(t.wire, 71);
        t.input(b"t=r:i=71:x=1;mismatched-old-data");
        assert!(t.events().is_empty());
    }
    #[test]
    fn unsupported_remote_invalid_and_oversized_batches_never_yield_file_authority() {
        assert!(Packet::parse(b"t=m:t=M;i=1").is_err());
        assert!(Packet::parse(&vec![b'x'; 8193]).is_err());
        let mut t = terminal();
        let id = drop(&mut t);
        t.input(b"t=r:i=71:x=1:X=1;moved-from-another-machine");
        assert!(matches!(
            t.events().pop(),
            Some(Event::Data { result: Err(_), .. })
        ));
        t.request(Request::Finish { id, action: None });
        let mut t = terminal();
        drop(&mut t);
        t.input(b"t=r:i=71:x=1:m=1;bm90LWFuLXVyaQ==");
        t.input(b"i=71:m=0;");
        assert!(matches!(
            t.events().pop(),
            Some(Event::Data { result: Err(_), .. })
        ));
    }
    #[test]
    fn terminal_outgoing_offers_only_bounded_uri_data_and_never_deletes_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("source.txt");
        std::fs::write(&path, "kept").unwrap();
        let mut t = terminal();
        t.input(b"t=o:i=71:x=3:y=4;");
        assert_eq!(t.take_gesture(), Some(Position::new(3, 4)));
        t.request(Request::Start {
            id: 8,
            paths: vec![path.clone()],
            copy: false,
        });
        assert!(
            t.output
                .iter()
                .any(|s| s.contains("t=o:o=3:i=71;text/uri-list"))
        );
        assert!(t.output.iter().any(|s| s.contains("t=P:x=-1:i=71")));
        t.input(b"t=E:i=71;OK");
        t.input(b"t=e:i=71:x=5:y=0;");
        assert!(t.output.iter().any(|s| s.contains("t=e:y=0:m=1:i=71")));
        t.input(b"t=e:i=71:x=4:y=0;");
        assert!(matches!(
            t.events().pop(),
            Some(Event::Sent {
                id: 8,
                result: Ok(())
            })
        ));
        assert!(path.exists());
    }
}

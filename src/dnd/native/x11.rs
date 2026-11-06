use super::Backend;
use crate::{
    clipboard::files::{MAX_BYTES, URI_LIST},
    dnd::{self, DropAction, Event, Request},
};
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event as XEvent,
        xproto::{ConnectionExt as _, *},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const CHUNK: usize = 48 * 1024;
struct Incoming {
    id: u64,
    source: Window,
    accepted: Option<DropAction>,
    dropped: bool,
    time: u32,
    last_position: Instant,
}
struct Reading {
    window: Window,
    bytes: Vec<u8>,
    incr: bool,
    deadline: Instant,
}
struct Sending {
    window: Window,
    property: Atom,
    bytes: Arc<[u8]>,
    offset: usize,
    deadline: Instant,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    window: Window,
    proxy: Window,
    version: u32,
}
struct Outgoing {
    id: u64,
    bytes: Arc<[u8]>,
    target: Option<Target>,
    accepted: Option<DropAction>,
    pending: Option<Instant>,
    point: Option<(i16, i16, DropAction)>,
    dropped: Option<Instant>,
}
pub(super) struct X11 {
    conn: RustConnection,
    root: Window,
    gui: Window,
    window: Window,
    atoms: HashMap<&'static str, Atom>,
    incoming: Option<Incoming>,
    reading: Option<Reading>,
    outgoing: Option<Outgoing>,
    sends: Vec<Sending>,
    events: Vec<Event>,
    serial: u64,
}
impl X11 {
    pub fn connect(gui: Window) -> Result<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
        let mut atoms = HashMap::new();
        for name in [
            "XdndAware",
            "XdndProxy",
            "XdndEnter",
            "XdndPosition",
            "XdndStatus",
            "XdndLeave",
            "XdndDrop",
            "XdndFinished",
            "XdndSelection",
            "XdndActionCopy",
            "XdndActionMove",
            "XdndActionList",
            "TARGETS",
            "INCR",
            URI_LIST,
            "_BASALGRIM_DROP",
        ] {
            atoms.insert(
                name,
                conn.intern_atom(false, name.as_bytes())?.reply()?.atom,
            );
        }
        // The protocol proxy receives messages on our own connection, leaving winit's event queue alone.
        for w in [window, gui] {
            conn.change_property32(
                PropMode::REPLACE,
                w,
                atoms["XdndProxy"],
                AtomEnum::WINDOW,
                &[window],
            )?
            .check()?;
            conn.change_property32(
                PropMode::REPLACE,
                w,
                atoms["XdndAware"],
                AtomEnum::ATOM,
                &[5],
            )?
            .check()?;
        }
        conn.flush()?;
        Ok(Self {
            conn,
            root,
            gui,
            window,
            atoms,
            incoming: None,
            reading: None,
            outgoing: None,
            sends: vec![],
            events: vec![],
            serial: 0,
        })
    }
    fn message(
        &self,
        destination: Window,
        window: Window,
        kind: &str,
        data: [u32; 5],
    ) -> Result<()> {
        self.conn.send_event(
            false,
            destination,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, window, self.atoms[kind], data),
        )?;
        Ok(())
    }
    fn action_atom(&self, action: Option<DropAction>) -> Atom {
        match action {
            Some(DropAction::Copy) => self.atoms["XdndActionCopy"],
            Some(DropAction::Move) => self.atoms["XdndActionMove"],
            None => NONE,
        }
    }
    fn action(&self, atom: Atom) -> Option<DropAction> {
        if atom == self.atoms["XdndActionCopy"] {
            Some(DropAction::Copy)
        } else if atom == self.atoms["XdndActionMove"] {
            Some(DropAction::Move)
        } else {
            None
        }
    }
    fn finish_incoming(&mut self, action: Option<DropAction>) -> Result<()> {
        if let Some(incoming) = self.incoming.take() {
            if incoming.dropped {
                self.message(
                    incoming.source,
                    incoming.source,
                    "XdndFinished",
                    [
                        self.gui,
                        u32::from(action.is_some()),
                        self.action_atom(action),
                        0,
                        0,
                    ],
                )?;
            }
            self.events.push(Event::Leave { id: incoming.id });
        }
        if let Some(read) = self.reading.take() {
            let _ = self.conn.destroy_window(read.window);
        }
        Ok(())
    }
    fn source_done(&mut self, result: Result<(), String>) {
        if let Some(source) = self.outgoing.take() {
            if let Some(target) = source.target {
                let _ = self.message(
                    target.proxy,
                    target.window,
                    "XdndLeave",
                    [self.window, 0, 0, 0, 0],
                );
            }
            self.events.push(Event::Sent {
                id: source.id,
                result,
            });
            // Do not modify CLIPBOARD or delete paths. A file manager receiving URI-list performs the move.
            if self
                .conn
                .get_selection_owner(self.atoms["XdndSelection"])
                .ok()
                .and_then(|v| v.reply().ok())
                .is_some_and(|v| v.owner == self.window)
            {
                let _ =
                    self.conn
                        .set_selection_owner(NONE, self.atoms["XdndSelection"], CURRENT_TIME);
            }
        }
    }
    fn data(&mut self, result: Result<Vec<PathBuf>, String>) {
        if let Some(read) = self.reading.take() {
            let _ = self.conn.destroy_window(read.window);
        }
        if let Some(incoming) = &self.incoming {
            self.events.push(Event::Data {
                id: incoming.id,
                result,
            });
        }
    }
    fn read_property(&mut self, window: Window) -> Result<()> {
        let property = self
            .conn
            .get_property(
                true,
                window,
                self.atoms["_BASALGRIM_DROP"],
                AtomEnum::ANY,
                0,
                (MAX_BYTES / 4 + 1) as u32,
            )?
            .reply()?;
        ensure!(
            property.bytes_after == 0 && property.value.len() <= MAX_BYTES,
            "Список перетаскивания превышает 4 MiB"
        );
        let Some(read) = &mut self.reading else {
            return Ok(());
        };
        if property.type_ == self.atoms["INCR"] && !read.incr {
            ensure!(
                property.format == 32
                    && property
                        .value32()
                        .and_then(|mut v| v.next())
                        .is_some_and(|n| n as usize <= MAX_BYTES),
                "Некорректный размер INCR"
            );
            read.incr = true;
        } else {
            ensure!(
                property.type_ == self.atoms[URI_LIST] && property.format == 8,
                "Некорректный тип перетаскивания"
            );
            ensure!(
                read.bytes.len() + property.value.len() <= MAX_BYTES,
                "Список перетаскивания превышает 4 MiB"
            );
            let done = !read.incr || property.value.is_empty();
            read.bytes.extend_from_slice(&property.value);
            if done {
                let result =
                    dnd::decode(Arc::from(read.bytes.as_slice())).map_err(|e| e.to_string());
                self.data(result);
            }
        }
        Ok(())
    }
    fn event(&mut self, event: XEvent) -> Result<()> {
        match event {
            XEvent::ClientMessage(v) if v.format == 32 => {
                let d = v.data.as_data32();
                if v.type_ == self.atoms["XdndEnter"] {
                    if self.incoming.as_ref().is_some_and(|i| i.dropped) {
                        return Ok(());
                    }
                    self.finish_incoming(None)?;
                    let version = d[1] >> 24;
                    if !(3..=5).contains(&version) {
                        return Ok(());
                    }
                    let types = if d[1] & 1 != 0 {
                        self.conn
                            .get_property(
                                false,
                                d[0],
                                self.conn.intern_atom(false, b"XdndTypeList")?.reply()?.atom,
                                AtomEnum::ATOM,
                                0,
                                64,
                            )?
                            .reply()?
                            .value32()
                            .map(|v| v.collect())
                            .unwrap_or_default()
                    } else {
                        d[2..5].to_vec()
                    };
                    if types.contains(&self.atoms[URI_LIST]) {
                        if self
                            .conn
                            .change_window_attributes(
                                d[0],
                                &ChangeWindowAttributesAux::new().event_mask(
                                    EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE,
                                ),
                            )?
                            .check()
                            .is_err()
                        {
                            return Ok(());
                        }
                        self.serial = self.serial.wrapping_add(1);
                        self.incoming = Some(Incoming {
                            id: self.serial.wrapping_mul(2) | 1,
                            source: d[0],
                            accepted: None,
                            dropped: false,
                            time: CURRENT_TIME,
                            last_position: Instant::now(),
                        });
                    }
                } else if v.type_ == self.atoms["XdndPosition"] {
                    if let Some(incoming) = &mut self.incoming {
                        if incoming.source != d[0] || incoming.dropped {
                            self.message(d[0], d[0], "XdndStatus", [self.gui, 2, 0, 0, NONE])?;
                            return Ok(());
                        }
                        incoming.time = d[3];
                        incoming.last_position = Instant::now();
                        let id = incoming.id;
                        let x = (d[2] >> 16) as i16;
                        let y = d[2] as i16;
                        let point = self
                            .conn
                            .translate_coordinates(self.root, self.gui, x, y)?
                            .reply()?;
                        let preferred = self.action(d[4]);
                        self.events.push(Event::Hover {
                            id,
                            x: f64::from(point.dst_x),
                            y: f64::from(point.dst_y),
                            physical: true,
                            allowed: if preferred.is_some() { 3 } else { 0 },
                            preferred: preferred.unwrap_or(DropAction::Copy),
                        });
                    } else {
                        self.message(d[0], d[0], "XdndStatus", [self.gui, 2, 0, 0, NONE])?;
                    }
                } else if v.type_ == self.atoms["XdndDrop"] {
                    if let Some(incoming) = &mut self.incoming
                        && incoming.source == d[0]
                    {
                        incoming.dropped = true;
                        incoming.time = d[2];
                        self.events.push(Event::Drop { id: incoming.id });
                    }
                } else if v.type_ == self.atoms["XdndLeave"] {
                    if self
                        .incoming
                        .as_ref()
                        .is_some_and(|s| s.source == d[0] && !s.dropped)
                    {
                        self.finish_incoming(None)?;
                    }
                } else if v.type_ == self.atoms["XdndStatus"] {
                    let action = self.action(d[4]);
                    if let Some(source) = &mut self.outgoing
                        && source.target.is_some_and(|t| t.window == d[0])
                    {
                        source.pending = None;
                        source.accepted = action.filter(|_| d[1] & 1 != 0);
                    }
                } else if v.type_ == self.atoms["XdndFinished"]
                    && self.outgoing.as_ref().is_some_and(|s| {
                        s.dropped.is_some() && s.target.is_some_and(|t| t.window == d[0])
                    })
                {
                    self.source_done(
                        if d[1] & 1 != 0
                            || self
                                .outgoing
                                .as_ref()
                                .and_then(|s| s.target)
                                .is_some_and(|t| t.version < 5)
                        {
                            Ok(())
                        } else {
                            Err("Принимающее приложение отклонило перетаскивание".into())
                        },
                    );
                }
            }
            XEvent::SelectionNotify(v)
                if self
                    .reading
                    .as_ref()
                    .is_some_and(|r| r.window == v.requestor) =>
            {
                let result = if v.selection != self.atoms["XdndSelection"]
                    || v.target != self.atoms[URI_LIST]
                    || v.property != self.atoms["_BASALGRIM_DROP"]
                {
                    Err(anyhow::anyhow!("Источник не передал список файлов"))
                } else {
                    self.read_property(v.requestor)
                };
                if let Err(error) = result {
                    self.data(Err(error.to_string()));
                }
            }
            XEvent::PropertyNotify(v) => {
                if v.state == Property::NEW_VALUE
                    && v.atom == self.atoms["_BASALGRIM_DROP"]
                    && self
                        .reading
                        .as_ref()
                        .is_some_and(|r| r.window == v.window && r.incr)
                    && let Err(error) = self.read_property(v.window)
                {
                    self.data(Err(error.to_string()));
                }
                if v.state == Property::DELETE
                    && let Some(index) = self
                        .sends
                        .iter()
                        .position(|s| s.window == v.window && s.property == v.atom)
                {
                    let s = &mut self.sends[index];
                    let end = (s.offset + CHUNK).min(s.bytes.len());
                    let _ = self.conn.change_property8(
                        PropMode::REPLACE,
                        s.window,
                        s.property,
                        self.atoms[URI_LIST],
                        &s.bytes[s.offset..end],
                    );
                    let done = end == s.offset;
                    s.offset = end;
                    if done {
                        self.sends.swap_remove(index);
                    }
                }
            }
            XEvent::SelectionRequest(r) => {
                let property = if r.property == NONE {
                    r.target
                } else {
                    r.property
                };
                let mut served = false;
                if r.selection == self.atoms["XdndSelection"]
                    && let Some(source) = &self.outgoing
                {
                    if r.target == self.atoms["TARGETS"] {
                        self.conn.change_property32(
                            PropMode::REPLACE,
                            r.requestor,
                            property,
                            AtomEnum::ATOM,
                            &[self.atoms["TARGETS"], self.atoms[URI_LIST]],
                        )?;
                        served = true;
                    } else if r.target == self.atoms[URI_LIST] && self.sends.len() < 16 {
                        if source.bytes.len() > CHUNK {
                            self.conn.change_window_attributes(
                                r.requestor,
                                &ChangeWindowAttributesAux::new()
                                    .event_mask(EventMask::PROPERTY_CHANGE),
                            )?;
                            self.conn.change_property32(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                self.atoms["INCR"],
                                &[source.bytes.len() as u32],
                            )?;
                            self.sends.push(Sending {
                                window: r.requestor,
                                property,
                                bytes: source.bytes.clone(),
                                offset: 0,
                                deadline: Instant::now() + Duration::from_secs(5),
                            });
                        } else {
                            self.conn.change_property8(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                r.target,
                                &source.bytes,
                            )?;
                        }
                        served = true;
                    }
                }
                self.conn.send_event(
                    false,
                    r.requestor,
                    EventMask::NO_EVENT,
                    SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: r.time,
                        requestor: r.requestor,
                        selection: r.selection,
                        target: r.target,
                        property: if served { property } else { NONE },
                    },
                )?;
            }
            XEvent::DestroyNotify(v) => {
                self.sends.retain(|s| s.window != v.window);
                if self.incoming.as_ref().is_some_and(|s| s.source == v.window) {
                    self.finish_incoming(None)?;
                }
                if self
                    .outgoing
                    .as_ref()
                    .and_then(|s| s.target)
                    .is_some_and(|t| t.window == v.window || t.proxy == v.window)
                {
                    self.source_done(Err("Принимающее окно закрыто".into()));
                }
            }
            XEvent::SelectionClear(v) if v.selection == self.atoms["XdndSelection"] => {
                self.source_done(Err("Перетаскивание заменено другой сессией".into()))
            }
            _ => {}
        }
        Ok(())
    }
    fn find_target(&self) -> Result<Option<Target>> {
        let mut window = self.root;
        for _ in 0..64 {
            let child = self.conn.query_pointer(window)?.reply()?.child;
            if child == NONE {
                break;
            }
            window = child;
            let aware = self
                .conn
                .get_property(false, window, self.atoms["XdndAware"], AtomEnum::ATOM, 0, 1)?
                .reply()?;
            if let Some(version) = aware
                .value32()
                .and_then(|mut v| v.next())
                .filter(|v| *v >= 3)
            {
                let proxy = self
                    .conn
                    .get_property(
                        false,
                        window,
                        self.atoms["XdndProxy"],
                        AtomEnum::WINDOW,
                        0,
                        1,
                    )?
                    .reply()?
                    .value32()
                    .and_then(|mut v| v.next())
                    .unwrap_or(window);
                let valid = if proxy != window {
                    self.conn
                        .get_property(
                            false,
                            proxy,
                            self.atoms["XdndProxy"],
                            AtomEnum::WINDOW,
                            0,
                            1,
                        )?
                        .reply()?
                        .value32()
                        .and_then(|mut v| v.next())
                        == Some(proxy)
                } else {
                    true
                };
                return Ok(Some(Target {
                    window,
                    proxy: if valid { proxy } else { window },
                    version: version.min(5),
                }));
            }
        }
        Ok(None)
    }
    fn poll_source(&mut self) -> Result<()> {
        let Some(mut source) = self.outgoing.take() else {
            return Ok(());
        };
        if let Some(dropped) = source.dropped {
            let timeout = dropped.elapsed() > Duration::from_secs(120);
            self.outgoing = Some(source);
            if timeout {
                self.source_done(Err("Приложение не завершило перетаскивание".into()));
            }
            return Ok(());
        }
        let pointer = self.conn.query_pointer(self.root)?.reply()?;
        let pressed = pointer.mask.contains(KeyButMask::BUTTON1);
        let action = if pointer.mask.contains(KeyButMask::CONTROL) {
            DropAction::Copy
        } else {
            DropAction::Move
        };
        let target = self.find_target()?;
        if source.target != target {
            if let Some(old) = source.target {
                self.message(
                    old.proxy,
                    old.window,
                    "XdndLeave",
                    [self.window, 0, 0, 0, 0],
                )?;
            }
            source.target = target;
            source.accepted = None;
            source.pending = None;
            source.point = None;
            if let Some(t) = target {
                self.conn.change_window_attributes(
                    t.window,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                )?;
                self.message(
                    t.proxy,
                    t.window,
                    "XdndEnter",
                    [self.window, t.version << 24, self.atoms[URI_LIST], 0, 0],
                )?;
            }
        }
        if source
            .pending
            .is_some_and(|t| t.elapsed() > Duration::from_millis(900))
        {
            source.pending = None;
            source.accepted = None;
            if !pressed {
                self.outgoing = Some(source);
                self.source_done(Err("Приложение не ответило на перетаскивание".into()));
                return Ok(());
            }
        }
        let point = (pointer.root_x, pointer.root_y, action);
        if let Some(t) = target
            && source.pending.is_none()
            && source.point != Some(point)
        {
            let packed = (u32::from(point.0 as u16) << 16) | u32::from(point.1 as u16);
            self.message(
                t.proxy,
                t.window,
                "XdndPosition",
                [
                    self.window,
                    0,
                    packed,
                    CURRENT_TIME,
                    self.action_atom(Some(action)),
                ],
            )?;
            source.pending = Some(Instant::now());
            source.point = Some(point);
        }
        if !pressed && source.pending.is_none() {
            if let Some(t) = target.filter(|_| source.accepted.is_some()) {
                self.message(
                    t.proxy,
                    t.window,
                    "XdndDrop",
                    [self.window, 0, CURRENT_TIME, 0, 0],
                )?;
                source.dropped = Some(Instant::now());
            } else {
                self.outgoing = Some(source);
                self.source_done(Err("Перетаскивание отменено".into()));
                return Ok(());
            }
        }
        self.outgoing = Some(source);
        Ok(())
    }
}
use std::path::PathBuf;
impl Backend for X11 {
    fn pump(&mut self) -> Result<Vec<Event>> {
        for _ in 0..256 {
            let Some(event) = self.conn.poll_for_event()? else {
                break;
            };
            self.event(event)?;
        }
        if self
            .reading
            .as_ref()
            .is_some_and(|r| Instant::now() > r.deadline)
        {
            self.data(Err("Источник перетаскивания не ответил вовремя".into()));
        }
        if self
            .incoming
            .as_ref()
            .is_some_and(|i| !i.dropped && i.last_position.elapsed() > Duration::from_secs(5))
            && !self
                .conn
                .query_pointer(self.root)?
                .reply()?
                .mask
                .contains(KeyButMask::BUTTON1)
        {
            self.finish_incoming(None)?;
        }
        self.sends.retain(|s| Instant::now() < s.deadline);
        self.poll_source()?;
        self.conn.flush()?;
        Ok(std::mem::take(&mut self.events))
    }
    fn request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::Start { id, paths, .. } => {
                self.source_done(Err("Предыдущее перетаскивание отменено".into()));
                let result = (|| -> Result<Arc<[u8]>> {
                    let bytes = dnd::encode(paths)?;
                    ensure!(
                        self.conn
                            .query_pointer(self.root)?
                            .reply()?
                            .mask
                            .contains(KeyButMask::BUTTON1),
                        "Кнопка мыши уже отпущена"
                    );
                    Ok(bytes)
                })();
                match result {
                    Ok(bytes) => {
                        self.conn
                            .set_selection_owner(
                                self.window,
                                self.atoms["XdndSelection"],
                                CURRENT_TIME,
                            )?
                            .check()?;
                        self.conn.change_property32(
                            PropMode::REPLACE,
                            self.window,
                            self.atoms["XdndActionList"],
                            AtomEnum::ATOM,
                            &[self.atoms["XdndActionMove"], self.atoms["XdndActionCopy"]],
                        )?;
                        self.outgoing = Some(Outgoing {
                            id,
                            bytes,
                            target: None,
                            accepted: None,
                            pending: None,
                            point: None,
                            dropped: None,
                        });
                    }
                    Err(error) => self.events.push(Event::Sent {
                        id,
                        result: Err(error.to_string()),
                    }),
                }
            }
            Request::Accept { id, action } => {
                if let Some(incoming) = &mut self.incoming
                    && incoming.id == id
                    && !incoming.dropped
                {
                    incoming.accepted = action;
                    let source = incoming.source;
                    self.message(
                        source,
                        source,
                        "XdndStatus",
                        [
                            self.gui,
                            2 | u32::from(action.is_some()),
                            0,
                            0,
                            self.action_atom(action),
                        ],
                    )?;
                }
            }
            Request::Read { id } => {
                if let Some(incoming) = &self.incoming
                    && incoming.id == id
                    && incoming.dropped
                    && incoming.accepted.is_some()
                    && self.reading.is_none()
                {
                    if self
                        .conn
                        .get_selection_owner(self.atoms["XdndSelection"])?
                        .reply()?
                        .owner
                        != incoming.source
                    {
                        self.events.push(Event::Data {
                            id,
                            result: Err("Источник перетаскивания изменился".into()),
                        });
                        return Ok(());
                    }
                    let window = self.conn.generate_id()?;
                    self.conn
                        .create_window(
                            0,
                            window,
                            self.root,
                            0,
                            0,
                            1,
                            1,
                            0,
                            WindowClass::INPUT_ONLY,
                            0,
                            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                        )?
                        .check()?;
                    self.conn.convert_selection(
                        window,
                        self.atoms["XdndSelection"],
                        self.atoms[URI_LIST],
                        self.atoms["_BASALGRIM_DROP"],
                        incoming.time,
                    )?;
                    self.reading = Some(Reading {
                        window,
                        bytes: Vec::new(),
                        incr: false,
                        deadline: Instant::now() + Duration::from_secs(5),
                    });
                }
            }
            Request::Finish { id, action } => {
                if self.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.finish_incoming(action)?;
                }
            }
            Request::Cancel { id } => {
                if self.outgoing.as_ref().is_some_and(|s| s.id == id) {
                    self.source_done(Err("Перетаскивание отменено".into()));
                }
                if self.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.finish_incoming(None)?;
                }
            }
        }
        self.conn.flush()?;
        Ok(())
    }
}
impl Drop for X11 {
    fn drop(&mut self) {
        let _ = self.finish_incoming(None);
        self.source_done(Err("Окно закрыто".into()));
        if self
            .conn
            .get_property(
                false,
                self.gui,
                self.atoms["XdndProxy"],
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()
            .and_then(|v| v.reply().ok())
            .and_then(|v| v.value32().and_then(|mut i| i.next()))
            == Some(self.window)
        {
            let _ = self.conn.delete_property(self.gui, self.atoms["XdndProxy"]);
        }
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

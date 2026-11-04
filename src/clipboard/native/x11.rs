use super::Backend;
use crate::clipboard::files::{self, Formats, MAX_BYTES};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME, NONE,
    connection::{Connection, RequestConnection},
    protocol::{
        Event,
        xfixes::{self, ConnectionExt as _, SelectionEventMask},
        xproto::{ConnectionExt as _, *},
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
const CHUNK: usize = 48 * 1024;
const TIMEOUT: Duration = Duration::from_millis(900);
struct Send {
    window: Window,
    property: Atom,
    target: Atom,
    bytes: Arc<[u8]>,
    offset: usize,
    started: Instant,
}
pub(super) struct X11 {
    conn: RustConnection,
    window: Window,
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    property: Atom,
    timestamp: Atom,
    atoms: HashMap<String, Atom>,
    formats: Formats,
    owner: Window,
    version: u64,
    cached: Option<Vec<String>>,
    sends: Vec<Send>,
}
impl X11 {
    pub fn connect() -> Result<Self> {
        let (conn, screen) = x11rb::connect(None).context("Нет доступного X11 файлового буфера")?;
        let window = conn.generate_id()?;
        conn.create_window(
            0,
            window,
            conn.setup().roots[screen].root,
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
        let atom = |name: &str| -> Result<Atom> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let clipboard = atom("CLIPBOARD")?;
        let targets = atom("TARGETS")?;
        let incr = atom("INCR")?;
        let property = atom("_BASALGRIM_FILE_SELECTION")?;
        let timestamp = atom("TIMESTAMP")?;
        let mut atoms = HashMap::new();
        for mime in
            files::READ_FORMATS
                .into_iter()
                .chain([files::TEXT, "text/plain", "UTF8_STRING"])
        {
            atoms.insert(mime.to_owned(), atom(mime)?);
        }
        // XFixes catches replacement by the same owner as well as owner-window changes.
        if conn
            .extension_information(xfixes::X11_EXTENSION_NAME)?
            .is_some()
        {
            conn.xfixes_query_version(5, 0)?.reply()?;
            conn.xfixes_select_selection_input(
                window,
                clipboard,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        }
        conn.flush()?;
        Ok(Self {
            conn,
            window,
            clipboard,
            targets,
            incr,
            property,
            timestamp,
            atoms,
            formats: Formats::new(),
            owner: NONE,
            version: 0,
            cached: None,
            sends: Vec::new(),
        })
    }
    fn changed(&mut self, owner: Window) {
        self.owner = owner;
        self.version = self.version.wrapping_add(1);
        self.cached = None;
    }
    fn event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::XfixesSelectionNotify(v) if v.selection == self.clipboard => {
                self.changed(v.owner)
            }
            Event::SelectionRequest(r) => {
                let property = if r.property == NONE {
                    r.target
                } else {
                    r.property
                };
                let served = (|| -> Result<bool> {
                    if r.selection != self.clipboard || self.owner != self.window {
                        return Ok(false);
                    }
                    if r.target == self.targets {
                        let mut offered = vec![self.targets, self.timestamp];
                        offered.extend(
                            self.formats
                                .keys()
                                .filter_map(|key| self.atoms.get(key).copied()),
                        );
                        self.conn
                            .change_property32(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                AtomEnum::ATOM,
                                &offered,
                            )?
                            .check()?;
                    } else if r.target == self.timestamp {
                        self.conn
                            .change_property32(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                AtomEnum::INTEGER,
                                &[CURRENT_TIME],
                            )?
                            .check()?;
                    } else {
                        let Some(bytes) = self
                            .atoms
                            .iter()
                            .find(|(_, atom)| **atom == r.target)
                            .and_then(|(name, _)| self.formats.get(name))
                            .cloned()
                        else {
                            return Ok(false);
                        };
                        if bytes.len() > CHUNK {
                            ensure!(self.sends.len() < 16, "Слишком много запросов буфера");
                            self.conn
                                .change_window_attributes(
                                    r.requestor,
                                    &ChangeWindowAttributesAux::new().event_mask(
                                        EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY,
                                    ),
                                )?
                                .check()?;
                            self.conn
                                .change_property32(
                                    PropMode::REPLACE,
                                    r.requestor,
                                    property,
                                    self.incr,
                                    &[bytes.len() as u32],
                                )?
                                .check()?;
                            self.sends.push(Send {
                                window: r.requestor,
                                property,
                                target: r.target,
                                bytes,
                                offset: 0,
                                started: Instant::now(),
                            });
                        } else {
                            self.conn
                                .change_property8(
                                    PropMode::REPLACE,
                                    r.requestor,
                                    property,
                                    r.target,
                                    &bytes,
                                )?
                                .check()?;
                        }
                    }
                    Ok(true)
                })()
                .unwrap_or(false);
                let notify = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: r.time,
                    requestor: r.requestor,
                    selection: r.selection,
                    target: r.target,
                    property: if served { property } else { NONE },
                };
                let _ = self
                    .conn
                    .send_event(false, r.requestor, EventMask::NO_EVENT, notify);
            }
            Event::PropertyNotify(p) if p.state == Property::DELETE => {
                if let Some(index) = self
                    .sends
                    .iter()
                    .position(|s| s.window == p.window && s.property == p.atom)
                {
                    let s = &mut self.sends[index];
                    let end = (s.offset + CHUNK).min(s.bytes.len());
                    let result = self.conn.change_property8(
                        PropMode::REPLACE,
                        s.window,
                        s.property,
                        s.target,
                        &s.bytes[s.offset..end],
                    );
                    let done = end == s.offset || result.is_err();
                    s.offset = end;
                    if done {
                        self.sends.swap_remove(index);
                    }
                }
            }
            Event::DestroyNotify(d) => self.sends.retain(|s| s.window != d.window),
            _ => {}
        }
        self.sends
            .retain(|s| s.started.elapsed() < Duration::from_secs(3));
        self.conn.flush()?;
        Ok(())
    }
    fn receive(&mut self, target: Atom, deadline: Instant) -> Result<Option<GetPropertyReply>> {
        let requestor = self.conn.generate_id()?;
        self.conn
            .create_window(
                0,
                requestor,
                self.window,
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
        let result = self.receive_into(requestor, target, deadline);
        let _ = self.conn.destroy_window(requestor);
        let _ = self.conn.flush();
        result
    }
    fn receive_into(
        &mut self,
        requestor: Window,
        target: Atom,
        deadline: Instant,
    ) -> Result<Option<GetPropertyReply>> {
        let version = self.version;
        self.conn.convert_selection(
            requestor,
            self.clipboard,
            target,
            self.property,
            CURRENT_TIME,
        )?;
        self.conn.flush()?;
        let mut incremental: Option<GetPropertyReply> = None;
        loop {
            ensure!(
                Instant::now() < deadline,
                "Владелец X11 буфера не ответил вовремя"
            );
            ensure!(
                self.version == version,
                "Владелец буфера изменился во время чтения"
            );
            let Some(event) = self.conn.poll_for_event()? else {
                thread::sleep(Duration::from_millis(2));
                continue;
            };
            match event {
                Event::SelectionNotify(n) if n.requestor == requestor && n.target == target => {
                    if n.property == NONE {
                        return Ok(None);
                    }
                    let reply = self
                        .conn
                        .get_property(
                            true,
                            requestor,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            (MAX_BYTES / 4 + 1) as u32,
                        )?
                        .reply()?;
                    ensure!(
                        reply.bytes_after == 0 && reply.value.len() <= MAX_BYTES,
                        "Файловый буфер превышает 4 MiB"
                    );
                    if reply.type_ != self.incr {
                        return Ok(Some(reply));
                    }
                    incremental = Some(GetPropertyReply {
                        type_: target,
                        format: 8,
                        value: Vec::new(),
                        ..reply
                    });
                    self.conn.flush()?;
                }
                Event::PropertyNotify(p)
                    if p.window == requestor
                        && p.atom == self.property
                        && p.state == Property::NEW_VALUE
                        && incremental.is_some() =>
                {
                    let reply = self
                        .conn
                        .get_property(
                            true,
                            requestor,
                            self.property,
                            AtomEnum::ANY,
                            0,
                            (MAX_BYTES / 4 + 1) as u32,
                        )?
                        .reply()?;
                    let accumulated = incremental.as_mut().unwrap();
                    ensure!(
                        reply.bytes_after == 0
                            && accumulated.value.len() + reply.value.len() <= MAX_BYTES,
                        "Файловый буфер превышает 4 MiB"
                    );
                    ensure!(
                        reply.format == 8 && reply.type_ == target,
                        "Некорректный формат X11 INCR"
                    );
                    if reply.value.is_empty() {
                        return Ok(incremental);
                    }
                    accumulated.value.extend(reply.value);
                    self.conn.flush()?;
                }
                event => self.event(event)?,
            }
        }
    }
}
impl Backend for X11 {
    fn update(&mut self, formats: Formats) {
        self.formats = formats;
        self.version = self.version.wrapping_add(1);
    }
    fn pump(&mut self) -> Result<()> {
        while let Some(event) = self.conn.poll_for_event()? {
            self.event(event)?;
        }
        let owner = self
            .conn
            .get_selection_owner(self.clipboard)?
            .reply()?
            .owner;
        if owner != self.owner {
            self.changed(owner);
        }
        Ok(())
    }
    fn offer(&mut self) -> Result<(u64, Vec<String>, bool)> {
        self.pump()?;
        if self.owner == self.window {
            return Ok((self.version, self.formats.keys().cloned().collect(), true));
        }
        if self.owner == NONE {
            return Ok((self.version, Vec::new(), false));
        }
        if self.cached.is_none() {
            let reply = self
                .receive(self.targets, Instant::now() + TIMEOUT)
                .ok()
                .flatten();
            self.cached = Some(
                reply
                    .and_then(|r| r.value32().map(|atoms| atoms.collect::<Vec<_>>()))
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|atom| {
                        self.atoms
                            .iter()
                            .find(|(_, a)| **a == atom)
                            .map(|(name, _)| name.clone())
                    })
                    .collect(),
            );
        }
        Ok((self.version, self.cached.clone().unwrap_or_default(), false))
    }
    fn write(&mut self, formats: Formats) -> Result<()> {
        self.formats = formats;
        self.conn
            .set_selection_owner(self.window, self.clipboard, CURRENT_TIME)?
            .check()?;
        ensure!(
            self.conn
                .get_selection_owner(self.clipboard)?
                .reply()?
                .owner
                == self.window,
            "Не удалось получить X11 буфер"
        );
        self.changed(self.window);
        Ok(())
    }
    fn read(&mut self) -> Result<Formats> {
        let (version, types, ours) = self.offer()?;
        if ours {
            return Ok(self.formats.clone());
        }
        let mut result = Formats::new();
        let deadline = Instant::now() + TIMEOUT;
        for mime in files::READ_FORMATS {
            if !types.iter().any(|s| s == mime) {
                continue;
            }
            if let Some(reply) = self.receive(self.atoms[mime], deadline)? {
                ensure!(reply.format == 8, "Некорректный тип файлового буфера");
                result.insert(mime.into(), reply.value.into());
            }
        }
        self.pump()?;
        if self.version != version {
            bail!("Файловый буфер изменился во время чтения")
        }
        Ok(result)
    }
}

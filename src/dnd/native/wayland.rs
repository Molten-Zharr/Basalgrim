use super::Backend;
use crate::{
    clipboard::files::{MAX_BYTES, URI_LIST},
    dnd::{self, DropAction, Event, Request},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::Arc,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    protocol::{
        wl_callback, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
        wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface,
    },
};

struct Offer {
    proxy: wl_data_offer::WlDataOffer,
    mimes: Vec<String>,
    allowed: u32,
    action: Option<DropAction>,
}
struct Incoming {
    id: u64,
    offer: ObjectId,
    serial: u32,
    x: f64,
    y: f64,
    accepted: Option<DropAction>,
    dropped: bool,
}
struct Reading {
    file: File,
    bytes: Vec<u8>,
    deadline: Instant,
}
struct Sending {
    file: File,
    bytes: Arc<[u8]>,
    offset: usize,
    deadline: Instant,
}
struct Source {
    id: u64,
    proxy: wl_data_source::WlDataSource,
}
#[derive(Default)]
struct State {
    registry: Option<wl_registry::WlRegistry>,
    manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    device: Option<wl_data_device::WlDataDevice>,
    origin: Option<wl_surface::WlSurface>,
    focused: bool,
    button: Option<u32>,
    offers: HashMap<ObjectId, Offer>,
    incoming: Option<Incoming>,
    reading: Option<Reading>,
    source: Option<Source>,
    sends: Vec<Sending>,
    events: Vec<Event>,
    serial: u64,
    synced: bool,
    ignore_drag: bool,
}
impl State {
    fn hover(&mut self) {
        if let Some(i) = &self.incoming
            && !i.dropped
            && let Some(o) = self.offers.get(&i.offer)
        {
            self.events.push(Event::Hover {
                id: i.id,
                x: i.x,
                y: i.y,
                physical: false,
                allowed: if o.mimes.iter().any(|m| m == URI_LIST) {
                    o.allowed
                } else {
                    0
                },
                preferred: o.action.unwrap_or(DropAction::Move),
            });
        }
    }
    fn finish(&mut self, action: Option<DropAction>) {
        if let Some(i) = self.incoming.take() {
            if let Some(offer) = self.offers.remove(&i.offer) {
                if i.dropped && i.accepted.is_some() && action.is_some() && offer.action.is_some() {
                    offer.proxy.finish();
                }
                offer.proxy.destroy();
            }
            self.events.push(Event::Leave { id: i.id });
        }
        self.reading = None;
    }
    fn sent(&mut self, result: Result<(), String>) {
        if let Some(source) = self.source.take() {
            source.proxy.destroy();
            self.events.push(Event::Sent {
                id: source.id,
                result,
            });
        }
    }
    fn data(&mut self, result: Result<Vec<std::path::PathBuf>, String>) {
        self.reading = None;
        if let Some(incoming) = &self.incoming {
            self.events.push(Event::Data {
                id: incoming.id,
                result,
            });
        }
    }
    fn io(&mut self) {
        self.sends.retain_mut(|s| {
            if Instant::now() > s.deadline {
                return false;
            }
            loop {
                match s.file.write(&s.bytes[s.offset..]) {
                    Ok(0) => return false,
                    Ok(n) => {
                        s.offset += n;
                        if s.offset == s.bytes.len() {
                            return false;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return false,
                }
            }
        });
        if let Some(read) = &mut self.reading {
            let mut chunk = [0; 8192];
            let result = loop {
                if Instant::now() > read.deadline {
                    break Some(Err("Источник перетаскивания не ответил вовремя".into()));
                }
                match read.file.read(&mut chunk) {
                    Ok(0) => {
                        break Some(
                            dnd::decode(Arc::from(read.bytes.as_slice()))
                                .map_err(|e| e.to_string()),
                        );
                    }
                    Ok(n) => {
                        if read.bytes.len() + n > MAX_BYTES {
                            break Some(Err("Список перетаскивания превышает 4 MiB".into()));
                        }
                        read.bytes.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break None,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => break Some(Err(e.to_string())),
                }
            };
            if let Some(result) = result {
                self.data(result);
            }
        }
    }
}
pub(super) struct Wayland {
    queue: EventQueue<State>,
    conn: Connection,
    state: State,
    _anchor: Arc<dyn Send + Sync>,
}
fn nonblock(fd: &impl AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}
impl Wayland {
    pub fn connect(display: usize, surface: usize, anchor: Arc<dyn Send + Sync>) -> Result<Self> {
        // SAFETY: the anchored GUI window owns both foreign objects and outlives our queue and connection.
        let backend =
            unsafe { wayland_client::backend::Backend::from_foreign_display(display as *mut _) };
        let conn = Connection::from_backend(backend);
        let origin = wl_surface::WlSurface::from_id(&conn, unsafe {
            ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface as *mut _)?
        })?;
        let queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut this = Self {
            queue,
            conn,
            state: State {
                origin: Some(origin),
                ..Default::default()
            },
            _anchor: anchor,
        };
        this.state.registry = Some(this.conn.display().get_registry(&qh, ()));
        this.sync()?;
        let manager = this
            .state
            .manager
            .as_ref()
            .context("Нет wl_data_device_manager v3 для перетаскивания")?;
        let seat = this
            .state
            .seat
            .as_ref()
            .context("Нет Wayland seat для перетаскивания")?;
        this.state.device = Some(manager.get_data_device(seat, &qh, ()));
        this.sync()?;
        Ok(this)
    }
    fn sync(&mut self) -> Result<()> {
        self.state.synced = false;
        self.conn.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + Duration::from_millis(900);
        while !self.state.synced {
            ensure!(Instant::now() < deadline, "Wayland не ответил вовремя");
            self.wait()?;
        }
        Ok(())
    }
    fn wait(&mut self) -> Result<()> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.conn.flush()?;
        if let Some(guard) = self.queue.prepare_read() {
            let mut fds = [rustix::event::PollFd::new(
                &self.conn,
                rustix::event::PollFlags::IN,
            )];
            rustix::event::poll(
                &mut fds,
                Some(&rustix::time::Timespec {
                    tv_sec: 0,
                    tv_nsec: 1_000_000,
                }),
            )?;
            if !fds[0].revents().is_empty() {
                match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        self.state.io();
        Ok(())
    }
}
impl Backend for Wayland {
    fn pump(&mut self) -> Result<Vec<Event>> {
        self.wait()?;
        Ok(std::mem::take(&mut self.state.events))
    }
    fn request(&mut self, request: Request) -> Result<()> {
        match request {
            Request::Start { id, paths, copy } => {
                let result = (|| -> Result<()> {
                    self.sync()?;
                    ensure!(
                        self.state.source.is_none(),
                        "Перетаскивание уже выполняется"
                    );
                    let serial = self.state.button.context("Кнопка мыши уже отпущена")?;
                    ensure!(self.state.focused, "Указатель покинул исходное окно");
                    let bytes = dnd::encode(paths)?;
                    let source = self
                        .state
                        .manager
                        .as_ref()
                        .unwrap()
                        .create_data_source(&self.queue.handle(), bytes);
                    source.offer(URI_LIST.into());
                    source.set_actions(if copy {
                        wl_data_device_manager::DndAction::Copy
                    } else {
                        wl_data_device_manager::DndAction::Copy
                            | wl_data_device_manager::DndAction::Move
                    });
                    self.state.device.as_ref().unwrap().start_drag(
                        Some(&source),
                        self.state.origin.as_ref().unwrap(),
                        None,
                        serial,
                    );
                    self.state.source = Some(Source { id, proxy: source });
                    self.conn.flush()?;
                    Ok(())
                })();
                if let Err(error) = result {
                    self.state.events.push(Event::Sent {
                        id,
                        result: Err(error.to_string()),
                    });
                }
            }
            Request::Accept { id, action } => {
                if let Some(i) = &mut self.state.incoming
                    && i.id == id
                    && !i.dropped
                    && let Some(offer) = self.state.offers.get(&i.offer)
                {
                    i.accepted = action;
                    let flags = wl_data_device_manager::DndAction::from_bits_truncate(
                        action.map_or(0, DropAction::bit),
                    );
                    let supported = if action.is_some() {
                        wl_data_device_manager::DndAction::from_bits_truncate(offer.allowed & 3)
                    } else {
                        flags
                    };
                    offer.proxy.set_actions(supported, flags);
                    offer
                        .proxy
                        .accept(i.serial, action.map(|_| URI_LIST.into()));
                }
            }
            Request::Read { id } => {
                if let Some(i) = &self.state.incoming
                    && i.id == id
                    && i.dropped
                    && i.accepted.is_some()
                    && let Some(offer) = self.state.offers.get(&i.offer)
                    && self.state.reading.is_none()
                {
                    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
                    nonblock(&read)?;
                    offer.proxy.receive(URI_LIST.into(), write.as_fd());
                    drop(write);
                    self.state.reading = Some(Reading {
                        file: File::from(read),
                        bytes: vec![],
                        deadline: Instant::now() + Duration::from_secs(5),
                    });
                }
            }
            Request::Finish { id, action } => {
                if self.state.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.state.finish(action);
                }
            }
            Request::Cancel { id } => {
                if self.state.incoming.as_ref().is_some_and(|i| i.id == id) {
                    self.state.finish(None);
                    self.state.ignore_drag = true;
                }
                if self.state.source.as_ref().is_some_and(|s| s.id == id) {
                    self.state.sent(Err("Перетаскивание отменено".into()));
                }
            }
        }
        self.conn.flush()?;
        Ok(())
    }
}
impl Drop for Wayland {
    fn drop(&mut self) {
        self.state.finish(None);
        self.state.sent(Err("Окно закрыто".into()));
        for (_, offer) in self.state.offers.drain() {
            offer.proxy.destroy();
        }
        if let Some(device) = self.state.device.take() {
            device.release();
        }
        if let Some(pointer) = self.state.pointer.take() {
            pointer.release();
        }
        if let Some(keyboard) = self.state.keyboard.take() {
            keyboard.release();
        }
        if let Some(seat) = self.state.seat.take() {
            seat.release();
        }
        let _ = self.conn.flush();
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        r: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_data_device_manager" if version >= 3 => {
                    state.manager = Some(r.bind(name, 3, qh, ()))
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(r.bind(name, version.min(7), qh, ()))
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(c),
        } = event
        {
            if c.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            } else if !c.contains(wl_seat::Capability::Keyboard)
                && let Some(keyboard) = state.keyboard.take()
            {
                keyboard.release();
            }
            if c.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            } else if !c.contains(wl_seat::Capability::Pointer) {
                if let Some(p) = state.pointer.take() {
                    p.release();
                }
                state.focused = false;
                state.button = None;
            }
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key: 1,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            state.sent(Err("Перетаскивание отменено".into()));
            if state.incoming.as_ref().is_some_and(|i| !i.dropped) {
                state.finish(None);
                state.ignore_drag = true;
            }
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => {
                state.focused = state
                    .origin
                    .as_ref()
                    .is_some_and(|s| s.id() == surface.id())
            }
            wl_pointer::Event::Leave { .. } => {
                state.focused = false;
                state.button = None;
            }
            wl_pointer::Event::Button {
                serial,
                button: 0x110,
                state: WEnum::Value(pressed),
                ..
            } => {
                state.button = if pressed == wl_pointer::ButtonState::Pressed && state.focused {
                    Some(serial)
                } else {
                    None
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                if state.offers.len() < 32 {
                    state.offers.insert(
                        id.id(),
                        Offer {
                            proxy: id,
                            mimes: vec![],
                            allowed: 1,
                            action: None,
                        },
                    );
                } else {
                    id.destroy();
                }
            }
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id,
            } => {
                if state.incoming.as_ref().is_some_and(|i| i.dropped) {
                    if let Some(offer) = id {
                        offer.accept(serial, None);
                        if let Some(o) = state.offers.remove(&offer.id()) {
                            o.proxy.destroy();
                        }
                    }
                    state.ignore_drag = true;
                    return;
                }
                state.ignore_drag = false;
                state.finish(None);
                if let Some(offer) = id {
                    if state
                        .origin
                        .as_ref()
                        .is_some_and(|s| s.id() == surface.id())
                        && state.offers.contains_key(&offer.id())
                    {
                        state.serial = state.serial.wrapping_add(1);
                        state.incoming = Some(Incoming {
                            id: state.serial.wrapping_mul(2) | 1,
                            offer: offer.id(),
                            serial,
                            x,
                            y,
                            accepted: None,
                            dropped: false,
                        });
                        state.hover();
                    } else {
                        if let Some(o) = state.offers.remove(&offer.id()) {
                            o.proxy.destroy();
                        }
                    }
                }
            }
            wl_data_device::Event::Motion { x, y, .. } if !state.ignore_drag => {
                if let Some(i) = &mut state.incoming {
                    i.x = x;
                    i.y = y;
                    state.hover();
                }
            }
            wl_data_device::Event::Drop => {
                if state.ignore_drag {
                    return;
                }
                // Publish the compositor's final action before capturing the destination.
                state.hover();
                if let Some(i) = &mut state.incoming {
                    i.dropped = true;
                    state.events.push(Event::Drop { id: i.id });
                }
            }
            wl_data_device::Event::Leave => {
                if state.ignore_drag {
                    state.ignore_drag = false;
                } else if state.incoming.as_ref().is_some_and(|i| !i.dropped) {
                    state.finish(None);
                }
            }
            wl_data_device::Event::Selection { id: Some(offer) } => {
                if let Some(o) = state.offers.remove(&offer.id()) {
                    o.proxy.destroy();
                }
            }
            _ => {}
        }
    }
    wayland_client::event_created_child!(State,wl_data_device::WlDataDevice,[0=> (wl_data_offer::WlDataOffer,())]);
}
impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        p: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(o) = state.offers.get_mut(&p.id()) {
            match event {
                wl_data_offer::Event::Offer { mime_type }
                    if mime_type.len() <= 256 && o.mimes.len() < 64 =>
                {
                    o.mimes.push(mime_type)
                }
                wl_data_offer::Event::SourceActions {
                    source_actions: WEnum::Value(a),
                } => {
                    o.allowed = a.bits() & 3;
                    state.hover();
                }
                wl_data_offer::Event::Action {
                    dnd_action: WEnum::Value(a),
                } => {
                    o.action = match a.bits() {
                        1 => Some(DropAction::Copy),
                        2 => Some(DropAction::Move),
                        _ => None,
                    };
                    state.hover();
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_data_source::WlDataSource, Arc<[u8]>> for State {
    fn event(
        state: &mut Self,
        p: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        bytes: &Arc<[u8]>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let fd: OwnedFd = fd;
                if mime_type == URI_LIST && state.sends.len() < 16 && nonblock(&fd).is_ok() {
                    state.sends.push(Sending {
                        file: File::from(fd),
                        bytes: bytes.clone(),
                        offset: 0,
                        deadline: Instant::now() + Duration::from_secs(5),
                    });
                }
            }
            wl_data_source::Event::Cancelled
                if state
                    .source
                    .as_ref()
                    .is_some_and(|s| s.proxy.id() == p.id()) =>
            {
                state.sent(Err("Перетаскивание отменено".into()))
            }
            wl_data_source::Event::DndFinished
                if state
                    .source
                    .as_ref()
                    .is_some_and(|s| s.proxy.id() == p.id()) =>
            {
                state.sent(Ok(()))
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.synced = true;
    }
}
wayland_client::delegate_noop!(State:ignore wl_data_device_manager::WlDataDeviceManager);

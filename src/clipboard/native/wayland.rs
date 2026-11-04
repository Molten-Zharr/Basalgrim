use super::Backend;
use crate::clipboard::files::{self, Formats, MAX_BYTES};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    backend::ObjectId,
    protocol::{
        wl_callback, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
        wl_keyboard, wl_pointer, wl_registry, wl_seat,
    },
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer, ext_data_control_source_v1 as ext_source,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer, zwlr_data_control_source_v1 as wlr_source,
};
const TIMEOUT: Duration = Duration::from_millis(900);
type Content = Arc<Mutex<Formats>>;

#[derive(Clone)]
enum Offer {
    Core(wl_data_offer::WlDataOffer),
    Ext(ext_offer::ExtDataControlOfferV1),
    Wlr(wlr_offer::ZwlrDataControlOfferV1),
}
impl Offer {
    fn id(&self) -> ObjectId {
        match self {
            Self::Core(v) => v.id(),
            Self::Ext(v) => v.id(),
            Self::Wlr(v) => v.id(),
        }
    }
    fn receive(&self, mime: String, fd: std::os::fd::BorrowedFd<'_>) {
        match self {
            Self::Core(v) => v.receive(mime, fd),
            Self::Ext(v) => v.receive(mime, fd),
            Self::Wlr(v) => v.receive(mime, fd),
        }
    }
    fn destroy(&self) {
        match self {
            Self::Core(v) => v.destroy(),
            Self::Ext(v) => v.destroy(),
            Self::Wlr(v) => v.destroy(),
        }
    }
}
enum Device {
    Core(wl_data_device::WlDataDevice),
    Ext(ext_device::ExtDataControlDeviceV1),
    Wlr(wlr_device::ZwlrDataControlDeviceV1),
}
enum Source {
    Core(wl_data_source::WlDataSource),
    Ext(ext_source::ExtDataControlSourceV1),
    Wlr(wlr_source::ZwlrDataControlSourceV1),
}
impl Source {
    fn id(&self) -> ObjectId {
        match self {
            Self::Core(v) => v.id(),
            Self::Ext(v) => v.id(),
            Self::Wlr(v) => v.id(),
        }
    }
    fn offer(&self, mime: String) {
        match self {
            Self::Core(v) => v.offer(mime),
            Self::Ext(v) => v.offer(mime),
            Self::Wlr(v) => v.offer(mime),
        }
    }
    fn destroy(&self) {
        match self {
            Self::Core(v) => v.destroy(),
            Self::Ext(v) => v.destroy(),
            Self::Wlr(v) => v.destroy(),
        }
    }
}
struct Sending {
    file: File,
    bytes: Arc<[u8]>,
    offset: usize,
    start: Instant,
}
#[derive(Default)]
struct State {
    registry: Option<wl_registry::WlRegistry>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    core: Option<wl_data_device_manager::WlDataDeviceManager>,
    ext: Option<ext_manager::ExtDataControlManagerV1>,
    wlr: Option<wlr_manager::ZwlrDataControlManagerV1>,
    seats: Vec<wl_seat::WlSeat>,
    device: Option<Device>,
    source: Option<Source>,
    content: Option<Content>,
    offers: HashMap<ObjectId, (Offer, Vec<String>)>,
    selection: Option<Offer>,
    sends: Vec<Sending>,
    version: u64,
    focused: bool,
    serial: Option<u32>,
    sync: bool,
    alive: bool,
    borrowed: bool,
}
impl State {
    fn selection(&mut self, offer: Option<Offer>) {
        self.version = self.version.wrapping_add(1);
        self.selection = offer;
        let selected = self.selection.as_ref().map(Offer::id);
        self.offers.retain(|id, (offer, _)| {
            let keep = Some(id) == selected.as_ref();
            if !keep {
                offer.destroy();
            }
            keep
        });
    }
    fn offered(&mut self, offer: Offer) {
        // A malicious peer must not grow an unbounded collection of MIME offers.
        if self.offers.len() < 32 {
            self.offers.insert(offer.id(), (offer, Vec::new()));
        } else {
            offer.destroy();
        }
    }
    fn mime(&mut self, id: ObjectId, mime: String) {
        if mime.len() <= 256
            && let Some((_, types)) = self.offers.get_mut(&id)
            && types.len() < 64
            && !types.contains(&mime)
        {
            types.push(mime);
        }
    }
    fn cancelled(&mut self, id: ObjectId) {
        if self.source.as_ref().is_some_and(|s| s.id() == id) {
            self.source.take().unwrap().destroy();
            self.content = None;
        }
    }
    fn send(&mut self, fd: OwnedFd, mime: String, formats: &Formats) {
        if self.sends.len() >= 16 {
            return;
        }
        if let Some(bytes) = formats.get(&mime)
            && nonblock(&fd).is_ok()
        {
            self.sends.push(Sending {
                file: File::from(fd),
                bytes: bytes.clone(),
                offset: 0,
                start: Instant::now(),
            });
        }
    }
    fn flush_sends(&mut self) {
        self.sends.retain_mut(|send| {
            if send.start.elapsed() > Duration::from_secs(3) {
                return false;
            }
            loop {
                match send.file.write(&send.bytes[send.offset..]) {
                    Ok(0) => return false,
                    Ok(n) => {
                        send.offset += n;
                        if send.offset == send.bytes.len() {
                            return false;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return true,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return false,
                }
            }
        });
    }
}
fn nonblock(fd: &impl AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}
pub(super) struct Wayland {
    // Field drop order keeps the display owner alive until the borrowed backend is gone.
    queue: EventQueue<State>,
    conn: Connection,
    state: State,
    _anchor: Option<Arc<dyn Send + Sync>>,
}
impl Wayland {
    pub fn connect(borrowed: Option<(usize, Arc<dyn Send + Sync>)>) -> Result<Self> {
        let (conn, anchor) = match borrowed {
            Some((pointer, anchor)) => {
                // SAFETY: anchor owns the GUI window/display, and outlives connection/queue/state.
                let backend = unsafe {
                    wayland_client::backend::Backend::from_foreign_display(pointer as *mut _)
                };
                (Connection::from_backend(backend), Some(anchor))
            }
            None => (
                Connection::connect_to_env().context("Нет доступного Wayland файлового буфера")?,
                None,
            ),
        };
        let queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut this = Self {
            queue,
            conn,
            state: State {
                borrowed: anchor.is_some(),
                alive: true,
                ..Default::default()
            },
            _anchor: anchor,
        };
        this.state.registry = Some(this.conn.display().get_registry(&qh, ()));
        this.sync()?;
        let seat = this
            .state
            .seats
            .first()
            .context("Нет Wayland seat для буфера")?;
        this.state.device = Some(if this.state.borrowed {
            Device::Core(
                this.state
                    .core
                    .as_ref()
                    .context("Нет wl_data_device_manager")?
                    .get_data_device(seat, &qh, ()),
            )
        } else if let Some(manager) = &this.state.ext {
            Device::Ext(manager.get_data_device(seat, &qh, ()))
        } else {
            Device::Wlr(
                this.state
                    .wlr
                    .as_ref()
                    .context("Терминалу нужен ext/wlr data-control; используется внутренний буфер")?
                    .get_data_device(seat, &qh, ()),
            )
        });
        this.sync()?;
        Ok(this)
    }
    fn sync(&mut self) -> Result<()> {
        self.state.sync = false;
        self.conn.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + TIMEOUT;
        while !self.state.sync {
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
                    tv_nsec: 2_000_000,
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
        self.state.flush_sends();
        ensure!(self.state.alive, "Wayland отключил файловый буфер");
        Ok(())
    }
}
impl Backend for Wayland {
    fn update(&mut self, formats: Formats) {
        if let Some(content) = &self.state.content {
            *content.lock().unwrap() = formats;
            self.state.version = self.state.version.wrapping_add(1);
        }
    }
    fn synchronize(&mut self) -> Result<()> {
        self.sync()
    }
    fn pump(&mut self) -> Result<()> {
        self.wait()
    }
    fn offer(&mut self) -> Result<(u64, Vec<String>, bool)> {
        self.queue.dispatch_pending(&mut self.state)?;
        let types = self
            .state
            .selection
            .as_ref()
            .and_then(|o| self.state.offers.get(&o.id()))
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        Ok((self.state.version, types, self.state.source.is_some()))
    }
    fn write(&mut self, formats: Formats) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        while self.state.borrowed && (!self.state.focused || self.state.serial.is_none()) {
            ensure!(
                Instant::now() < deadline,
                "Для записи в Wayland буфер окно должно иметь фокус"
            );
            self.wait()?;
        }
        let qh = self.queue.handle();
        let content = Arc::new(Mutex::new(formats.clone()));
        let source = match self.state.device.as_ref().unwrap() {
            Device::Core(_) => Source::Core(
                self.state
                    .core
                    .as_ref()
                    .unwrap()
                    .create_data_source(&qh, content.clone()),
            ),
            Device::Ext(_) => Source::Ext(
                self.state
                    .ext
                    .as_ref()
                    .unwrap()
                    .create_data_source(&qh, content.clone()),
            ),
            Device::Wlr(_) => Source::Wlr(
                self.state
                    .wlr
                    .as_ref()
                    .unwrap()
                    .create_data_source(&qh, content.clone()),
            ),
        };
        for mime in formats.keys() {
            source.offer(mime.clone());
        }
        match (self.state.device.as_ref().unwrap(), &source) {
            (Device::Core(d), Source::Core(s)) => {
                d.set_selection(Some(s), self.state.serial.unwrap())
            }
            (Device::Ext(d), Source::Ext(s)) => d.set_selection(Some(s)),
            (Device::Wlr(d), Source::Wlr(s)) => d.set_selection(Some(s)),
            _ => unreachable!(),
        }
        if let Some(old) = self.state.source.replace(source) {
            old.destroy();
        }
        self.state.content = Some(content);
        self.sync()?;
        Ok(())
    }
    fn read(&mut self) -> Result<Formats> {
        if self.state.borrowed {
            ensure!(
                self.state.focused,
                "Для чтения Wayland буфера окно должно иметь фокус"
            );
        }
        let Some(offer) = self.state.selection.clone() else {
            return Ok(Formats::new());
        };
        let version = self.state.version;
        let types = self
            .state
            .offers
            .get(&offer.id())
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        let deadline = Instant::now() + TIMEOUT;
        let mut result = Formats::new();
        for mime in files::READ_FORMATS {
            if !types.iter().any(|m| m == mime) {
                continue;
            }
            let (reader, writer) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
            nonblock(&reader)?;
            offer.receive(mime.into(), writer.as_fd());
            drop(writer);
            self.conn.flush()?;
            let mut reader = File::from(reader);
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                ensure!(
                    Instant::now() < deadline,
                    "Владелец Wayland буфера не ответил вовремя"
                );
                ensure!(
                    self.state.version == version,
                    "Буфер изменился во время чтения"
                );
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        ensure!(
                            bytes.len() + n <= MAX_BYTES,
                            "Файловый буфер превышает 4 MiB"
                        );
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => self.wait()?,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e.into()),
                }
            }
            result.insert(mime.into(), bytes.into());
        }
        self.wait()?;
        ensure!(
            self.state.version == version,
            "Буфер изменился во время чтения"
        );
        Ok(result)
    }
}

impl Drop for Wayland {
    fn drop(&mut self) {
        if let Some(source) = self.state.source.take() {
            source.destroy();
        }
        for (_, (offer, _)) in self.state.offers.drain() {
            offer.destroy();
        }
        if let Some(device) = self.state.device.take() {
            match device {
                Device::Core(d) => {
                    if d.version() >= 2 {
                        d.release();
                    }
                }
                Device::Ext(d) => d.destroy(),
                Device::Wlr(d) => d.destroy(),
            }
        }
        if let Some(keyboard) = self.state.keyboard.take()
            && keyboard.version() >= 3
        {
            keyboard.release();
        }
        if let Some(pointer) = self.state.pointer.take()
            && pointer.version() >= 3
        {
            pointer.release();
        }
        for seat in self.state.seats.drain(..) {
            if seat.version() >= 5 {
                seat.release();
            }
        }
        if let Some(manager) = self.state.ext.take() {
            manager.destroy();
        }
        if let Some(manager) = self.state.wlr.take() {
            manager.destroy();
        }
        // wl_registry and wl_data_device_manager have no protocol destructor.
        self.state.registry.take();
        let _ = self.conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
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
                "wl_data_device_manager" => {
                    state.core = Some(registry.bind(name, version.min(3), qh, ()))
                }
                "ext_data_control_manager_v1" => state.ext = Some(registry.bind(name, 1, qh, ())),
                "zwlr_data_control_manager_v1" => {
                    state.wlr = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "wl_seat" if state.seats.is_empty() => {
                    state
                        .seats
                        .push(registry.bind(name, version.min(7), qh, ()))
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
        if state.borrowed
            && let wl_seat::Event::Capabilities {
                capabilities: WEnum::Value(c),
            } = event
        {
            if c.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            } else if !c.contains(wl_seat::Capability::Keyboard)
                && let Some(keyboard) = state.keyboard.take()
            {
                if keyboard.version() >= 3 {
                    keyboard.release();
                }
                state.focused = false;
                state.serial = None;
            }
            if c.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            } else if !c.contains(wl_seat::Capability::Pointer)
                && let Some(pointer) = state.pointer.take()
                && pointer.version() >= 3
            {
                pointer.release();
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
        match event {
            wl_keyboard::Event::Enter { serial, .. } => {
                state.focused = true;
                state.serial = Some(serial);
            }
            wl_keyboard::Event::Leave { .. } => state.focused = false,
            wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => state.serial = Some(serial),
            _ => {}
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
        if let wl_pointer::Event::Button { serial, .. } = event {
            state.serial = Some(serial);
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
        state.sync = true;
    }
}
macro_rules! offers {
    ($proxy:ty,$event:path,$variant:ident) => {
        impl Dispatch<$proxy, ()> for State {
            fn event(
                state: &mut Self,
                p: &$proxy,
                event: $event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $event as E;
                if let E::Offer { mime_type } = event {
                    state.mime(p.id(), mime_type);
                }
            }
        }
    };
}
offers!(wl_data_offer::WlDataOffer, wl_data_offer::Event, Core);
offers!(ext_offer::ExtDataControlOfferV1, ext_offer::Event, Ext);
offers!(wlr_offer::ZwlrDataControlOfferV1, wlr_offer::Event, Wlr);
macro_rules! devices {
    ($proxy:ty,$event:path,$offer:ty,$variant:ident)=>{
        impl Dispatch<$proxy,()> for State {
            fn event(state:&mut Self,_:&$proxy,event:$event,_:&(),_:&Connection,_:&QueueHandle<Self>) {
                use $event as E;
                match event {
                    E::DataOffer {id}=>state.offered(Offer::$variant(id)),
                    E::Selection {id}=>state.selection(id.map(Offer::$variant)),
                    E::Finished=>state.alive=false,_=>{},
                }
            }
            wayland_client::event_created_child!(State,$proxy,[0=>($offer,())]);
        }
    }
}
// Core selection has a different field name from data-control.
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
            wl_data_device::Event::DataOffer { id } => state.offered(Offer::Core(id)),
            wl_data_device::Event::Selection { id } => state.selection(id.map(Offer::Core)),
            _ => {}
        }
    }
    wayland_client::event_created_child!(State,wl_data_device::WlDataDevice,[0=>(wl_data_offer::WlDataOffer,())]);
}
devices!(
    ext_device::ExtDataControlDeviceV1,
    ext_device::Event,
    ext_offer::ExtDataControlOfferV1,
    Ext
);
devices!(
    wlr_device::ZwlrDataControlDeviceV1,
    wlr_device::Event,
    wlr_offer::ZwlrDataControlOfferV1,
    Wlr
);
macro_rules! sources {
    ($proxy:ty,$event:path) => {
        impl Dispatch<$proxy, Content> for State {
            fn event(
                state: &mut Self,
                p: &$proxy,
                event: $event,
                formats: &Content,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $event as E;
                match event {
                    E::Send { mime_type, fd } => {
                        state.send(fd, mime_type, &formats.lock().unwrap())
                    }
                    E::Cancelled => state.cancelled(p.id()),
                    _ => {}
                }
            }
        }
    };
}
sources!(wl_data_source::WlDataSource, wl_data_source::Event);
sources!(ext_source::ExtDataControlSourceV1, ext_source::Event);
sources!(wlr_source::ZwlrDataControlSourceV1, wlr_source::Event);
wayland_client::delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);
wayland_client::delegate_noop!(State: ignore ext_manager::ExtDataControlManagerV1);
wayland_client::delegate_noop!(State: ignore wlr_manager::ZwlrDataControlManagerV1);

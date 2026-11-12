use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use zbus::{
    blocking::{Connection, connection::Builder},
    fdo::{RequestNameFlags, RequestNameReply},
};

pub enum Event {
    Batch(Result<Batch, String>),
}
enum Command {
    Request(Request),
}
#[derive(Clone)]
struct Endpoint {
    tx: SyncSender<Command>,
}
impl Endpoint {
    fn send(&self, kind: Kind, uris: Vec<String>, startup: String) -> zbus::fdo::Result<()> {
        let request = Request::from_uris(kind, &uris, startup)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        self.tx.try_send(Command::Request(request)).map_err(|_| {
            zbus::fdo::Error::LimitsExceeded("Очередь запросов Basalgrim заполнена".into())
        })
    }
}
#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl Endpoint {
    fn show_folders(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        self.send(Kind::Folders, uris, startup_id)
    }
    fn show_items(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        self.send(Kind::Items, uris, startup_id)
    }
    fn show_item_properties(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        self.send(Kind::Properties, uris, startup_id)
    }
}
struct Launcher(Endpoint);
#[zbus::interface(name = "org.moltenzharr.Basalgrim")]
impl Launcher {
    async fn claim_file_manager(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        match connection
            .request_name_with_flags(FILE_MANAGER, RequestNameFlags::DoNotQueue.into())
            .await
        {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(()),
            Ok(_) | Err(zbus::Error::NameTaken) => Err(zbus::fdo::Error::Failed(
                "FileManager1 занят другим менеджером".into(),
            )),
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
    }
    async fn release_file_manager(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        connection
            .release_name(FILE_MANAGER)
            .await
            .map(|_| ())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    fn open(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
        self.0.send(Kind::Open, uris, startup_id)
    }
}
pub struct Runtime {
    connection: Option<Connection>,
    integration_tx: SyncSender<Result<String, String>>,
    pub integrations: Receiver<Result<String, String>>,
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    pub status: String,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn claim(connection: &Connection, name: &str) -> Result<bool> {
    match connection.request_name_with_flags(name, RequestNameFlags::DoNotQueue.into()) {
        Ok(reply) => Ok(matches!(
            reply,
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner
        )),
        Err(zbus::Error::NameTaken) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

impl Runtime {
    /// None means the launcher successfully forwarded its request to the existing GUI.
    pub fn start(
        gui: bool,
        forward: bool,
        file_manager: bool,
        initial: Option<Request>,
    ) -> Result<Option<Self>> {
        Self::start_at(None, gui, forward, file_manager, initial)
    }
    /// Explicit private-bus injection for tests; never fall back to another bus.
    pub fn start_at(
        address: Option<String>,
        gui: bool,
        forward: bool,
        file_manager: bool,
        initial: Option<Request>,
    ) -> Result<Option<Self>> {
        let (tx, commands) = mpsc::sync_channel(8);
        let (events, rx) = mpsc::sync_channel(8);
        let (ready, startup) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let endpoint = Endpoint { tx: tx.clone() };
        std::thread::Builder::new().name("desktop".into()).spawn(move || {
            let mut connection = None;
            let mut primary = false;
            let mut status = "Рабочий стол: отдельный терминальный экземпляр".to_string();
            if gui || forward || file_manager {
                let result = (|| -> Result<bool> {
                    let conn = if let Some(address) = &address { Builder::address(address.as_str())? } else { Builder::session()? }.method_timeout(Duration::from_secs(2))
                        .serve_at(OBJECT, endpoint.clone())?.serve_at(OBJECT, Launcher(endpoint))?.build()?;
                    primary = claim(&conn, BUS)?;
                    if !primary && forward {
                        if file_manager {
                            conn.call_method(Some(BUS), OBJECT, Some(BUS), "ClaimFileManager", &())?;
                        }
                        let request = initial.clone().unwrap_or(Request { kind: Kind::Open, paths: vec![], startup_id: String::new() });
                        let uris = request.paths.iter().map(|p| crate::clipboard::files::uri(p)).collect::<Result<Vec<_>>>()?;
                        conn.call_method(Some(BUS), OBJECT, Some(if request.kind == Kind::Open { BUS } else { FILE_MANAGER }), request.kind.method(), &(uris, request.startup_id))?;
                        return Ok(true);
                    }
                    if primary {
                        let default = install::is_default();
                        status = if (file_manager || default) && claim(&conn, FILE_MANAGER)? {
                            "FileManager1 подключён".into()
                        } else if file_manager || default {
                            "FileManager1 занят другим менеджером; перезапустите сеанс рабочего стола".into()
                        } else { "Basalgrim готов принимать запросы ярлыка".into() };
                    } else { status = "Другой экземпляр Basalgrim принимает запросы рабочего стола".into(); }
                    connection = Some(conn);
                    Ok(false)
                })();
                match result {
                    Ok(true) => { let _ = ready.send(Ok(None)); return; }
                    Err(e) if forward || file_manager => { let _ = ready.send(Err(format!("Рабочий стол: {e:#}"))); return; }
                    Err(e) => status = format!("D-Bus недоступен: {e:#}"),
                    _ => {}
                }
            }
            if ready.send(Ok(Some((status, if primary { connection.clone() } else { None })))).is_err() { return; }
            let mut next = initial.map(Command::Request);
            while !worker_stop.load(Ordering::Relaxed) {
                let command = next.take().or_else(|| commands.recv_timeout(Duration::from_millis(50)).ok());
                let Some(command) = command else { continue; };
                let event = match command {
                    Command::Request(request) => Event::Batch(resolve(request).map_err(|e| format!("{e:#}"))),

                };
                let mut event = event;
                loop {
                    match events.try_send(event) {
                        Ok(()) => break,
                        Err(mpsc::TrySendError::Full(e)) => event = e,
                        Err(_) => return,
                    }
                    if worker_stop.load(Ordering::Relaxed) { return; }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        })?;
        let (integration_tx, integrations) = mpsc::sync_channel(1);
        match startup.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(Some((status, connection)))) => Ok(Some(Self {
                connection,
                integration_tx,
                integrations,
                events: rx,
                stop,
                status,
            })),
            Ok(Ok(None)) => Ok(None),
            other => {
                stop.store(true, Ordering::Relaxed);
                anyhow::bail!(
                    "{}",
                    match other {
                        Ok(Err(e)) => e,
                        _ => "Истекло время подключения рабочего стола".into(),
                    }
                );
            }
        }
    }
    pub fn install(&self, config: Config, action: InstallAction) -> bool {
        let connection = self.connection.clone();
        let tx = self.integration_tx.clone();
        std::thread::Builder::new().name("desktop-install".into()).spawn(move || {
            let result = integrate(&config, action).and_then(|message| {
                if let Some(conn) = &connection {
                    if action == InstallAction::Default && !claim(conn, FILE_MANAGER)? {
                        return Ok(format!("{message}. FileManager1 занят; перезапустите сеанс рабочего стола"));
                    }
                    if action == InstallAction::Restore { let _ = conn.release_name(FILE_MANAGER); }
                }
                if connection.is_none() && let Some(warning) = notify_registration(action) {
                    return Ok(format!("{message}. {warning}"));
                }
                Ok(message)
            }).map_err(|e| format!("{e:#}"));
            let _ = tx.try_send(result);
        }).is_ok()
    }
}

/// Notify an existing primary after CLI registration; never activate or replace another manager.
pub fn notify_registration(action: InstallAction) -> Option<String> {
    if action == InstallAction::Install {
        return None;
    }
    let result = (|| -> Result<()> {
        let conn = Builder::session()?
            .method_timeout(Duration::from_secs(2))
            .build()?;
        let proxy = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        if !proxy.name_has_owner(BUS.try_into()?)? {
            return Ok(());
        }
        conn.call_method(
            Some(BUS),
            OBJECT,
            Some(BUS),
            if action == InstallAction::Default {
                "ClaimFileManager"
            } else {
                "ReleaseFileManager"
            },
            &(),
        )?;
        Ok(())
    })();
    result
        .err()
        .map(|e| format!("Служба текущего сеанса не обновлена: {e}"))
}

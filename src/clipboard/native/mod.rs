//! Desktop selection service. All protocol I/O runs off the rendering/input thread.
mod wayland;
mod x11;
use super::files::{self, Formats};
use anyhow::{Result, bail};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

pub use super::service::*;
trait Backend {
    fn pump(&mut self) -> Result<()>;
    fn synchronize(&mut self) -> Result<()> {
        self.pump()
    }
    fn offer(&mut self) -> Result<(u64, Vec<String>, bool)>;
    fn write(&mut self, formats: Formats) -> Result<()>;
    // Revise data without claiming ownership again, even if another app wins a race.
    fn update(&mut self, formats: Formats);
    fn read(&mut self, formats: &[&str]) -> Result<Formats>;
}
pub struct Runtime {
    tx: Option<mpsc::SyncSender<Request>>,
    pub events: mpsc::Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    latest_read: Arc<AtomicU64>,
    latest_text_read: Arc<AtomicU64>,
}
impl Runtime {
    fn spawn(make: impl FnOnce() -> Result<Box<dyn Backend>> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        let (events, received) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let latest_read = Arc::new(AtomicU64::new(0));
        let worker_read = latest_read.clone();
        let latest_text_read = Arc::new(AtomicU64::new(0));
        let worker_text_read = latest_text_read.clone();
        let worker = thread::spawn(move || {
            let mut backend = match make() {
                Ok(v) => v,
                Err(e) => {
                    let _ = events.send(Event::Ready(Err(e.to_string())));
                    return;
                }
            };
            let mut random = [0; 16];
            if rustix::rand::getrandom(&mut random, rustix::rand::GetRandomFlags::empty()).ok()
                != Some(16)
            {
                let _ = events.send(Event::Ready(Err(
                    "Не удалось создать идентификатор буфера".into()
                )));
                return;
            }
            let nonce = random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let _ = events.send(Event::Ready(Ok(())));
            let mut owned: Option<(u64, bool)> = None;
            let mut previous = None;
            loop {
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(e) = backend.pump() {
                    let _ = events.send(Event::Ready(Err(e.to_string())));
                    break;
                }
                match backend.offer() {
                    Ok((version, types, ours)) => {
                        if !ours {
                            owned = None;
                        }
                        let offer = Offer {
                            version,
                            files: types
                                .iter()
                                .any(|s| s == files::GNOME || s == files::URI_LIST),
                            local: owned.map(|v| v.0),
                        };
                        if previous.as_ref() != Some(&offer) {
                            previous = Some(offer.clone());
                            if events.send(Event::Offer(offer)).is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = events.send(Event::Ready(Err(e.to_string())));
                        break;
                    }
                }
                let request = match rx.recv_timeout(Duration::from_millis(15)) {
                    Ok(v) => v,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let event = match request {
                    Request::Write {
                        id,
                        generation,
                        payload,
                        expected,
                    } => {
                        // Never overwrite a later foreign selection, a newer copy, or our own text.
                        let result = (|| -> Result<()> {
                            if let Some(expected) = expected
                                && (owned != Some((expected, true)) || !backend.offer()?.2)
                            {
                                return Ok(());
                            }
                            let is_files = matches!(payload, Payload::Files(_));
                            let formats = match payload {
                                Payload::Files(list) => files::encode(&list, &nonce, generation)?,
                                Payload::Text(text) => {
                                    anyhow::ensure!(
                                        text.len() <= files::MAX_BYTES,
                                        "Текстовый буфер превышает 4 MiB"
                                    );
                                    files::encode_text(text, &nonce, generation, "text")
                                }
                            };
                            if expected.is_some() {
                                backend.update(formats);
                            } else {
                                backend.write(formats)?;
                            }
                            owned = Some((generation, is_files));
                            Ok(())
                        })()
                        .map_err(|e| e.to_string());
                        Event::Written { id, result }
                    }
                    Request::Read { id } => {
                        let result = (|| -> Result<Incoming> {
                            anyhow::ensure!(
                                worker_read.load(Ordering::Relaxed) == id,
                                "Чтение буфера отменено"
                            );
                            backend.synchronize()?;
                            let (version, _, ours) = backend.offer()?;
                            if ours && let Some((generation, _)) = owned {
                                return Ok(Incoming::Local(generation));
                            }
                            let formats = backend.read(&files::READ_FORMATS)?;
                            anyhow::ensure!(
                                backend.offer()?.0 == version,
                                "Буфер изменился во время чтения; повторите вставку"
                            );
                            if let Some(generation) = files::local_generation(&formats, &nonce) {
                                return Ok(Incoming::Local(generation));
                            }
                            use std::hash::{Hash, Hasher};
                            let mut hash = std::collections::hash_map::DefaultHasher::new();
                            for (mime, bytes) in &formats {
                                mime.hash(&mut hash);
                                bytes.hash(&mut hash);
                            }
                            Ok(Incoming::External {
                                stamp: (version, hash.finish()),
                                files: files::decode(&formats)?,
                            })
                        })()
                        .map_err(|e| e.to_string());
                        Event::Read { id, result }
                    }
                    Request::ReadText { id } => {
                        let result = (|| -> Result<String> {
                            anyhow::ensure!(
                                worker_text_read.load(Ordering::Relaxed) == id,
                                "Чтение текста отменено"
                            );
                            backend.synchronize()?;
                            let (version, types, _) = backend.offer()?;
                            let mime = [files::TEXT, "UTF8_STRING", "text/plain"]
                                .into_iter()
                                .find(|mime| types.iter().any(|s| s == mime))
                                .ok_or_else(|| anyhow::anyhow!("В буфере нет текста"))?;
                            let formats = backend.read(&[mime])?;
                            anyhow::ensure!(
                                worker_text_read.load(Ordering::Relaxed) == id
                                    && backend.offer()?.0 == version,
                                "Буфер или редактор изменились во время чтения"
                            );
                            let bytes = formats
                                .get(mime)
                                .ok_or_else(|| anyhow::anyhow!("В буфере нет текста"))?;
                            anyhow::ensure!(
                                bytes.len() <= 64 * 1024,
                                "Текст буфера превышает 64 KiB"
                            );
                            Ok(std::str::from_utf8(bytes)?.to_owned())
                        })()
                        .map_err(|e| e.to_string());
                        Event::Text { id, result }
                    }
                };
                if events.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            tx: Some(tx),
            events: received,
            worker: Some(worker),
            stop,
            latest_read,
            latest_text_read,
        }
    }
    pub fn terminal() -> Self {
        Self::spawn(|| {
            if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
            {
                bail!("В удалённом терминале используется внутренний файловый буфер");
            }
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                match wayland::Wayland::connect(None) {
                    Ok(backend) => return Ok(Box::new(backend)),
                    Err(error) if std::env::var_os("DISPLAY").is_none() => return Err(error),
                    Err(_) => {} // A local XWayland clipboard bridge may still be available.
                }
            }
            Ok(Box::new(x11::X11::connect()?))
        })
    }
    #[cfg(feature = "guitui")]
    pub fn gui<W: raw_window_handle::HasDisplayHandle + Send + Sync + 'static>(
        window: Arc<W>,
    ) -> Self {
        Self::spawn(move || {
            use raw_window_handle::RawDisplayHandle;
            match window.display_handle()?.as_raw() {
                RawDisplayHandle::Wayland(handle) => {
                    // Keep the native window/display alive until after the foreign connection drops.
                    let pointer = handle.display.as_ptr() as usize;
                    Ok(
                        Box::new(wayland::Wayland::connect(Some((pointer, window)))?)
                            as Box<dyn Backend>,
                    )
                }
                RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                    Ok(Box::new(x11::X11::connect()?))
                }
                _ => bail!("Файловый буфер этого оконного бэкенда не поддерживается"),
            }
        })
    }
    pub fn submit(&self, request: Request) -> Result<(), mpsc::TrySendError<Request>> {
        if let Request::Read { id } = &request {
            self.latest_read.store(*id, Ordering::Relaxed);
        }
        if let Request::ReadText { id } = &request {
            self.latest_text_read.store(*id, Ordering::Relaxed);
        }
        self.tx.as_ref().unwrap().try_send(request)
    }
    pub fn cancel_reads(&self) {
        self.latest_read.store(0, Ordering::Relaxed);
    }
    pub fn cancel_text_reads(&self) {
        self.latest_text_read.store(0, Ordering::Relaxed);
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

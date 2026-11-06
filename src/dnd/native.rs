//! Native GUI drag transport. Protocol waits and bounded URI transfers stay off the UI thread.
mod wayland;
mod x11;
use super::{Event, Request};
use anyhow::Result;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

trait Backend {
    fn pump(&mut self) -> Result<Vec<Event>>;
    fn request(&mut self, request: Request) -> Result<()>;
}
pub struct Runtime {
    tx: mpsc::SyncSender<Request>,
    pub events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn gui<
        W: raw_window_handle::HasDisplayHandle
            + raw_window_handle::HasWindowHandle
            + Send
            + Sync
            + 'static,
    >(
        window: Arc<W>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(32);
        let (events, received) = mpsc::sync_channel(128);
        let stop = Arc::new(AtomicBool::new(false));
        let halted = stop.clone();
        let worker = thread::spawn(move || {
            let make = || -> Result<Box<dyn Backend>> {
                use raw_window_handle::{RawDisplayHandle as D, RawWindowHandle as W};
                match (
                    window.display_handle()?.as_raw(),
                    window.window_handle()?.as_raw(),
                ) {
                    (D::Wayland(display), W::Wayland(surface)) => {
                        Ok(Box::new(wayland::Wayland::connect(
                            display.display.as_ptr() as usize,
                            surface.surface.as_ptr() as usize,
                            window,
                        )?))
                    }
                    (D::Xlib(_), W::Xlib(handle)) => {
                        Ok(Box::new(x11::X11::connect(handle.window as u32)?))
                    }
                    (D::Xcb(_), W::Xcb(handle)) => {
                        Ok(Box::new(x11::X11::connect(handle.window.get())?))
                    }
                    _ => anyhow::bail!("Перетаскивание этого оконного бэкенда не поддерживается"),
                }
            };
            let mut backend = match make() {
                Ok(b) => b,
                Err(e) => {
                    let _ = events.try_send(Event::Ready(Err(e.to_string())));
                    wake();
                    return;
                }
            };
            let _ = events.try_send(Event::Ready(Ok(())));
            wake();
            while !halted.load(Ordering::Relaxed) {
                let result = (|| -> Result<()> {
                    while let Ok(request) = rx.try_recv() {
                        backend.request(request)?;
                    }
                    for event in backend.pump()? {
                        if events.try_send(event).is_err() {
                            anyhow::bail!("Окно не отвечает на перетаскивание");
                        }
                        wake();
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = events.try_send(Event::Ready(Err(error.to_string())));
                    wake();
                    break;
                }
                thread::sleep(Duration::from_millis(8));
            }
        });
        Self {
            tx,
            events: received,
            stop,
            worker: Some(worker),
        }
    }
    pub fn submit(&self, request: Request) -> bool {
        self.tx.try_send(request).is_ok()
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

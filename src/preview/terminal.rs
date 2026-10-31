//! Latest-frame encoding never blocks the terminal input/render loop.
use super::{
    PreviewSlot,
    images::{PreviewKey, PreviewRasterizer},
};
use crate::icons::terminal::TerminalGraphics;
use ratatui::Frame;
use ratatui_image::{Image, protocol::Protocol};
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

struct Request {
    id: u64,
    key: PreviewKey,
    slot: PreviewSlot,
    graphics: TerminalGraphics,
}
struct Encoded {
    id: u64,
    key: PreviewKey,
    result: Result<Protocol, String>,
}
pub struct TerminalPreview {
    graphics: TerminalGraphics,
    requests: SyncSender<Request>,
    results: Receiver<Encoded>,
    serial: u64,
    current: Option<(u64, PreviewKey)>,
    protocol: Option<Protocol>,
    failed: bool,
}
impl TerminalPreview {
    pub fn new(graphics: TerminalGraphics) -> Self {
        let (requests, input) = mpsc::sync_channel::<Request>(1);
        let (send, results) = mpsc::channel();
        thread::spawn(move || {
            let mut rasterizer = PreviewRasterizer::default();
            while let Ok(mut request) = input.recv() {
                for newer in input.try_iter() {
                    request = newer;
                }
                let result = (|| {
                    let image = rasterizer
                        .image(
                            &request.slot,
                            request.graphics.cell_width,
                            request.graphics.cell_height,
                        )?
                        .clone();
                    request.graphics.encode(image, request.slot.area.as_size())
                })()
                .map_err(|e: anyhow::Error| e.to_string());
                if send
                    .send(Encoded {
                        id: request.id,
                        key: request.key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            graphics,
            requests,
            results,
            serial: 0,
            current: None,
            protocol: None,
            failed: false,
        }
    }

    fn accept(&mut self, encoded: Encoded) {
        if self.current != Some((encoded.id, encoded.key)) {
            return;
        }
        match encoded.result {
            Ok(protocol) => self.protocol = Some(protocol),
            Err(_) => self.failed = true,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, slot: Option<&PreviewSlot>) -> bool {
        let Some(slot) = slot else {
            self.current = None;
            self.protocol = None;
            while self.results.try_recv().is_ok() {}
            return true;
        };
        let key = PreviewKey::new(slot, self.graphics.cell_width, self.graphics.cell_height);
        if self.current.as_ref().map(|(_, key)| *key) != Some(key) {
            self.current = None;
            self.protocol = None;
            self.serial += 1;
            let request = Request {
                id: self.serial,
                key,
                slot: slot.clone(),
                graphics: self.graphics,
            };
            match self.requests.try_send(request) {
                Ok(()) => self.current = Some((self.serial, key)),
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
            }
        }
        while let Ok(encoded) = self.results.try_recv() {
            self.accept(encoded);
        }
        if self.failed {
            return false;
        }
        if let Some(protocol) = &self.protocol {
            frame.render_widget(Image::new(protocol), slot.area);
        }
        true
    }

    pub fn resize_cells(&mut self, width: u16, height: u16) {
        if width == 0
            || height == 0
            || (width, height) == (self.graphics.cell_width, self.graphics.cell_height)
        {
            return;
        }
        self.graphics.cell_width = width;
        self.graphics.cell_height = height;
        self.current = None;
        self.protocol = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{icons::IconBackend, preview::ImagePreview};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use std::{sync::Arc, time::Duration};

    #[test]
    fn pending_encodings_cannot_reappear_after_resize_hiding_or_a_new_request() {
        let mut preview = TerminalPreview::new(TerminalGraphics {
            backend: IconBackend::Kitty,
            cell_width: 8,
            cell_height: 16,
            tmux: false,
        });
        let slot = PreviewSlot {
            image: Arc::new(
                ImagePreview::new(2, 1, vec![240, 20, 30, 255, 240, 20, 30, 255]).unwrap(),
            ),
            area: Rect::new(1, 1, 8, 4),
            background: [0; 3],
        };
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        terminal
            .draw(|f| assert!(preview.render(f, Some(&slot))))
            .unwrap();
        let old = preview
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        preview.resize_cells(10, 20);
        preview.accept(old);
        assert!(preview.protocol.is_none());
        terminal
            .draw(|f| assert!(preview.render(f, Some(&slot))))
            .unwrap();
        let old = preview
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let old_id = old.id;
        let old_key = old.key;
        terminal.draw(|f| assert!(preview.render(f, None))).unwrap();
        preview.accept(old);
        assert!(preview.protocol.is_none());
        terminal
            .draw(|f| assert!(preview.render(f, Some(&slot))))
            .unwrap();
        preview.accept(Encoded {
            id: old_id,
            key: old_key,
            result: Err("obsolete".into()),
        });
        assert!(!preview.failed);
        let current = preview
            .results
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        preview.accept(current);
        assert!(preview.protocol.is_some());
    }
}

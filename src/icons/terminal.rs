//! Terminal probing and cached image encoding. No graphics protocol means symbols.
use super::{
    IconBackend, IconSlot,
    images::{IconKey, IconRasterizer},
};
use image::DynamicImage;
use ratatui::{Frame, layout::Size};
use ratatui_image::{
    Image,
    protocol::{Protocol, iterm2::Iterm2, kitty::Kitty, sixel::Sixel},
};
use std::{
    collections::HashMap,
    io::{self, IsTerminal, Read, Write},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct TerminalGraphics {
    pub backend: IconBackend,
    pub cell_width: u16,
    pub cell_height: u16,
    pub tmux: bool,
}
impl TerminalGraphics {
    pub(crate) fn encode(self, image: image::RgbaImage, area: Size) -> anyhow::Result<Protocol> {
        use std::sync::atomic::{AtomicU32, Ordering};
        // Icons and previews share an ID space, including concurrently encoded frames.
        static SERIAL: AtomicU32 = AtomicU32::new(1);
        let id = SERIAL
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(std::process::id() << 16)
            .max(1);
        let image = DynamicImage::ImageRgba8(image);
        Ok(match self.backend {
            IconBackend::Kitty => Protocol::Kitty(Kitty::new(image, area, id, self.tmux, false)?),
            IconBackend::Sixel => Protocol::Sixel(Sixel::new(image, area, self.tmux)?),
            IconBackend::Iterm2 => Protocol::ITerm2(Iterm2::new(image, area, self.tmux)?),
            _ => anyhow::bail!("No terminal graphics protocol"),
        })
    }
}

/// No reader thread survives this bounded probe. Mouse capture starts afterwards.
#[cfg(target_os = "linux")]
fn probe() -> Option<TerminalGraphics> {
    use ratatui_image::picker::{
        ProtocolType,
        cap_parser::{Parser, QueryStdioOptions, Response},
    };
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let tmux = std::env::var("TERM").is_ok_and(|value| value.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|value| value == "tmux");
    let alternative = std::env::var("TERM_PROGRAM").is_ok_and(|value| {
        ["iTerm", "WezTerm", "mintty", "Tabby", "rio"]
            .iter()
            .any(|name| value.contains(name))
    }) || std::env::var("LC_TERMINAL").is_ok_and(|value| value.contains("iTerm"));
    let blacklist = ["WEZTERM_EXECUTABLE", "KONSOLE_VERSION"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()));
    let query = Parser::query(
        tmux,
        QueryStdioOptions {
            blacklist_protocols: if blacklist {
                vec![ProtocolType::Kitty, ProtocolType::Sixel]
            } else {
                Vec::new()
            },
            ..Default::default()
        },
    );
    let mut output = io::stdout().lock();
    output.write_all(query.as_bytes()).ok()?;
    output.flush().ok()?;
    drop(output);
    let input = io::stdin();
    let mut reader = input.lock();
    let mut parser = Parser::new();
    let mut backend = IconBackend::Symbols;
    let mut cells = None;
    let mut status = false;
    let deadline = Instant::now() + Duration::from_millis(500);
    while !status && Instant::now() < deadline {
        let timeout: Timespec = deadline
            .saturating_duration_since(Instant::now())
            .try_into()
            .ok()?;
        let mut fds = [PollFd::new(&input, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout)).ok()? == 0 {
            break;
        }
        let mut bytes = [0; 256];
        let count = reader.read(&mut bytes).ok()?;
        if count == 0 {
            break;
        }
        for byte in &bytes[..count] {
            for response in parser.push(char::from(*byte)) {
                match response {
                    Response::Kitty => backend = IconBackend::Kitty,
                    Response::Sixel if backend != IconBackend::Kitty => {
                        backend = IconBackend::Sixel
                    }
                    Response::CellSize(Some(size)) => cells = Some(size),
                    Response::Status => status = true,
                    _ => {}
                }
            }
        }
    }
    if backend == IconBackend::Symbols && alternative && status {
        backend = IconBackend::Iterm2;
    }
    let (cell_width, cell_height) = cells.or_else(|| {
        let size = crossterm::terminal::window_size().ok()?;
        (size.columns > 0 && size.rows > 0)
            .then(|| (size.width / size.columns, size.height / size.rows))
    })?;
    (backend.supports_images() && cell_width > 0 && cell_height > 0).then_some(TerminalGraphics {
        backend,
        cell_width,
        cell_height,
        tmux,
    })
}

#[cfg(not(target_os = "linux"))]
fn probe() -> Option<TerminalGraphics> {
    None
}

struct Request {
    generation: u64,
    id: u64,
    key: IconKey,
    area: Size,
    graphics: TerminalGraphics,
}

struct Encoded {
    generation: u64,
    id: u64,
    key: IconKey,
    result: Result<Protocol, String>,
}
struct Cached {
    id: u64,
    protocol: Option<Protocol>,
}
pub struct TerminalIcons {
    graphics: TerminalGraphics,
    cache: HashMap<IconKey, Cached>,
    requests: SyncSender<Request>,
    results: Receiver<Encoded>,
    generation: u64,
    serial: u64,
    failed: bool,
}
impl TerminalIcons {
    /// Called in the alternate screen, before mouse/focus events and the input loop.
    pub fn detect() -> Option<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return None;
        }
        Self::new(probe()?)
    }

    pub fn new(graphics: TerminalGraphics) -> Option<Self> {
        if !matches!(
            graphics.backend,
            IconBackend::Kitty | IconBackend::Sixel | IconBackend::Iterm2
        ) || graphics.cell_width == 0
            || graphics.cell_height == 0
        {
            return None;
        }
        let (requests, input) = mpsc::sync_channel::<Request>(32);
        let (send, results) = mpsc::channel();
        thread::spawn(move || {
            let mut rasterizer = IconRasterizer::default();
            for request in input {
                let result = (|| {
                    let image = rasterizer.image(request.key)?.clone();
                    request.graphics.encode(image, request.area)
                })()
                .map_err(|error: anyhow::Error| error.to_string());
                if send
                    .send(Encoded {
                        generation: request.generation,
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
        Some(Self {
            graphics,
            cache: HashMap::new(),
            requests,
            results,
            generation: 0,
            serial: 0,
            failed: false,
        })
    }

    pub fn backend(&self) -> IconBackend {
        self.graphics.backend
    }
    pub fn graphics(&self) -> TerminalGraphics {
        self.graphics
    }

    fn poll(&mut self) {
        while let Ok(result) = self.results.try_recv() {
            self.accept(result);
        }
    }
    fn accept(&mut self, result: Encoded) {
        if result.generation != self.generation {
            return;
        }
        let Some(cached) = self
            .cache
            .get_mut(&result.key)
            .filter(|cached| cached.id == result.id)
        else {
            return;
        };
        match result.result {
            Ok(protocol) => cached.protocol = Some(protocol),
            Err(_) => self.failed = true,
        }
    }

    /// Returns false after an encoding error so the caller can restore symbols.
    pub fn render(&mut self, frame: &mut Frame, slots: &[IconSlot]) -> bool {
        self.poll();
        if self.failed {
            return false;
        }
        for slot in slots {
            let key = IconKey::new(
                slot,
                u32::from(self.graphics.cell_width),
                u32::from(self.graphics.cell_height),
            );
            if let Some(cached) = self.cache.get(&key) {
                if let Some(protocol) = &cached.protocol {
                    frame.render_widget(Image::new(protocol), slot.area);
                }
                continue;
            }
            if self.cache.len() >= 128 {
                self.cache.clear();
                self.generation += 1;
            }
            self.serial += 1;
            let request = Request {
                generation: self.generation,
                id: self.serial,
                key,
                area: slot.area.as_size(),
                graphics: self.graphics,
            };
            match self.requests.try_send(request) {
                Ok(()) => {
                    self.cache.insert(
                        key,
                        Cached {
                            id: self.serial,
                            protocol: None,
                        },
                    );
                }
                Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.failed = true;
                    return false;
                }
            }
        }
        true
    }

    pub fn resize_cells(&mut self, width: u16, height: u16) {
        if width == 0
            || height == 0
            || (self.graphics.cell_width, self.graphics.cell_height) == (width, height)
        {
            return;
        }
        self.graphics.cell_width = width;
        self.graphics.cell_height = height;
        self.cache.clear();
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::IconKind;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use std::time::Instant;

    #[test]
    fn terminals_without_graphics_do_not_use_halfblock_images() {
        assert!(
            TerminalIcons::new(TerminalGraphics {
                backend: IconBackend::Symbols,
                cell_width: 8,
                cell_height: 16,
                tmux: false
            })
            .is_none()
        );
    }

    #[test]
    fn graphics_protocols_encode_in_worker_and_survive_cell_resize() {
        for (protocol, marker) in [
            (IconBackend::Kitty, "\u{10EEEE}"),
            (IconBackend::Sixel, "\x1bP"),
            (IconBackend::Iterm2, "\x1b]1337;"),
        ] {
            let mut icons = TerminalIcons::new(TerminalGraphics {
                backend: protocol,
                cell_width: 10,
                cell_height: 20,
                tmux: false,
            })
            .unwrap();
            let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
            let slots = [
                IconSlot {
                    kind: IconKind::Code,
                    area: Rect::new(1, 1, 6, 2),
                    foreground: [248, 248, 242],
                    accent: [255, 82, 46],
                    background: [0, 0, 0],
                },
                IconSlot {
                    kind: IconKind::Code,
                    area: Rect::new(10, 1, 6, 2),
                    foreground: [248, 248, 242],
                    accent: [255, 82, 46],
                    background: [0, 0, 0],
                },
            ];
            for cell in [(10, 20), (8, 16)] {
                icons.resize_cells(cell.0, cell.1);
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    terminal
                        .draw(|frame| {
                            assert!(icons.render(frame, &slots));
                        })
                        .unwrap();
                    let buffer = terminal.backend().buffer();
                    if buffer[(1, 1)].symbol().contains(marker)
                        && buffer[(10, 1)].symbol().contains(marker)
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline, "No {protocol:?} image rendered");
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    #[test]
    fn late_encodings_from_previous_generation_are_ignored() {
        let mut icons = TerminalIcons::new(TerminalGraphics {
            backend: IconBackend::Sixel,
            cell_width: 10,
            cell_height: 20,
            tmux: false,
        })
        .unwrap();
        let slot = IconSlot {
            kind: IconKind::File,
            area: Rect::new(0, 0, 6, 2),
            foreground: [248, 248, 242],
            accent: [255, 82, 46],
            background: [0, 0, 0],
        };
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        terminal
            .draw(|frame| {
                icons.render(frame, &[slot]);
            })
            .unwrap();
        icons.resize_cells(8, 16);
        let result = icons.results.recv_timeout(Duration::from_secs(5)).unwrap();
        icons.accept(result);
        assert!(icons.cache.is_empty());
        assert!(!icons.failed);
    }
}

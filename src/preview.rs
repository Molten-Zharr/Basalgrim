//! Bounded image previews shared by the native and terminal frontends.
use ratatui::layout::Rect;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[cfg(any(feature = "guitui", feature = "tui"))]
mod decode;
#[cfg(any(feature = "guitui", feature = "tui"))]
pub use decode::read;
#[cfg(any(feature = "guitui", feature = "tui"))]
pub mod images;
#[cfg(feature = "tui")]
pub mod terminal;

/// Only a downscaled RGBA frame survives decoding. Cloning a preview shares it.
#[derive(Debug)]
pub struct ImagePreview {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    id: u64,
}
impl ImagePreview {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            width > 0
                && height > 0
                && width <= 1024
                && height <= 1024
                && u64::from(width) * u64::from(height) * 4 == pixels.len() as u64,
            "Invalid preview dimensions"
        );
        static SERIAL: AtomicU64 = AtomicU64::new(1);
        Ok(Self {
            width,
            height,
            pixels,
            id: SERIAL.fetch_add(1, Ordering::Relaxed),
        })
    }
    pub fn id(&self) -> u64 {
        self.id
    }
}

#[derive(Clone, Debug)]
pub struct PreviewSlot {
    pub image: Arc<ImagePreview>,
    pub area: Rect,
    pub background: [u8; 3],
}
impl PartialEq for PreviewSlot {
    fn eq(&self, other: &Self) -> bool {
        self.image.id == other.image.id
            && self.area == other.area
            && self.background == other.background
    }
}
impl Eq for PreviewSlot {}

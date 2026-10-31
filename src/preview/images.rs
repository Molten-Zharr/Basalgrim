//! One cached, aspect-preserving frame, including the theme background.
use super::PreviewSlot;
use anyhow::{Context, Result, ensure};
use image::{
    Rgba, RgbaImage,
    imageops::{self, FilterType},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewKey {
    image: u64,
    width: u32,
    height: u32,
    background: [u8; 3],
}
impl PreviewKey {
    pub fn new(slot: &PreviewSlot, cell_width: u16, cell_height: u16) -> Self {
        Self {
            image: slot.image.id(),
            width: u32::from(slot.area.width) * u32::from(cell_width),
            height: u32::from(slot.area.height) * u32::from(cell_height),
            background: slot.background,
        }
    }
}

#[derive(Default)]
pub struct PreviewRasterizer {
    cached: Option<(PreviewKey, RgbaImage)>,
}
impl PreviewRasterizer {
    pub fn image(
        &mut self,
        slot: &PreviewSlot,
        cell_width: u16,
        cell_height: u16,
    ) -> Result<&RgbaImage> {
        let key = PreviewKey::new(slot, cell_width, cell_height);
        if self.cached.as_ref().map(|(key, _)| *key) != Some(key) {
            self.cached = Some((key, rasterize(slot, key)?));
        }
        Ok(&self.cached.as_ref().unwrap().1)
    }
}

fn rasterize(slot: &PreviewSlot, key: PreviewKey) -> Result<RgbaImage> {
    ensure!(
        key.width > 0
            && key.height > 0
            && key.width <= 4096
            && key.height <= 4096
            && u64::from(key.width) * u64::from(key.height) <= 4 * 1024 * 1024,
        "Preview viewport too large"
    );
    let source = RgbaImage::from_raw(
        slot.image.width,
        slot.image.height,
        slot.image.pixels.clone(),
    )
    .context("Invalid image preview buffer")?;
    let scale =
        (key.width as f64 / source.width() as f64).min(key.height as f64 / source.height() as f64);
    let width = (source.width() as f64 * scale).round().max(1.0) as u32;
    let height = (source.height() as f64 * scale).round().max(1.0) as u32;
    let resized = imageops::resize(&source, width, height, FilterType::Triangle);
    let [r, g, b] = key.background;
    let mut canvas = RgbaImage::from_pixel(key.width, key.height, Rgba([r, g, b, 255]));
    imageops::overlay(
        &mut canvas,
        &resized,
        i64::from((key.width - width) / 2),
        i64::from((key.height - height) / 2),
    );
    Ok(canvas)
}

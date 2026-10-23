//! Small, embedded SVG assets rasterized at the backend's physical pixel size.
use super::{IconKind, IconSlot};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use resvg::{tiny_skia, usvg};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconKey {
    kind: IconKind,
    width: u32,
    height: u32,
    foreground: [u8; 3],
    accent: [u8; 3],
    background: [u8; 3],
}
impl IconKey {
    pub fn new(slot: &IconSlot, cell_width: u32, cell_height: u32) -> Self {
        Self {
            kind: slot.kind,
            width: u32::from(slot.area.width) * cell_width,
            height: u32::from(slot.area.height) * cell_height,
            foreground: slot.foreground,
            accent: slot.accent,
            background: slot.background,
        }
    }
}

#[derive(Default)]
pub struct IconRasterizer {
    cache: HashMap<IconKey, RgbaImage>,
}
impl IconRasterizer {
    pub fn image(&mut self, key: IconKey) -> Result<&RgbaImage> {
        if !self.cache.contains_key(&key) {
            if self.cache.len() >= 128 {
                self.cache.clear();
            }
            self.cache.insert(key, rasterize(key)?);
        }
        Ok(&self.cache[&key])
    }
}

fn hex(rgb: [u8; 3]) -> String {
    crate::preferences::color_hex(rgb)
}
fn blend(a: [u8; 3], b: [u8; 3], amount: u16) -> [u8; 3] {
    std::array::from_fn(|i| {
        ((u16::from(a[i]) * amount + u16::from(b[i]) * (100 - amount)) / 100) as u8
    })
}
pub fn rasterize(key: IconKey) -> Result<RgbaImage> {
    ensure!(
        (1..=2048).contains(&key.width) && (1..=2048).contains(&key.height),
        "Invalid icon dimensions"
    );
    let template = if key.kind == IconKind::Folder {
        include_str!("../../assets/icons/folder.svg")
    } else {
        include_str!("../../assets/icons/file.svg")
    };
    let symbol = match key.kind {
        IconKind::Folder => "",
        IconKind::File | IconKind::Document => "<path d=\"M27 36h17M27 42h17M27 48h11\"/>",
        IconKind::Code => "<path d=\"m29 36-5 6 5 6m14-12 5 6-5 6m-5-13-4 14\"/>",
        IconKind::Archive => "<path d=\"M34 31v5h5v5h-5v5h5v6M32 46h9v7h-9Z\"/>",
        IconKind::Image => {
            "<path d=\"m24 49 8-9 6 6 5-6 5 9Z\"/><circle cx=\"42\" cy=\"36\" r=\"2\"/>"
        }
        IconKind::Audio => {
            "<path d=\"M33 46V35l11-2v11M33 35l11-2\"/><ellipse cx=\"29\" cy=\"47\" rx=\"4\" ry=\"3\"/><ellipse cx=\"40\" cy=\"45\" rx=\"4\" ry=\"3\"/>"
        }
        IconKind::Video => "<path d=\"m31 35 12 7-12 7Z\"/>",
        IconKind::Application => {
            "<rect x=\"25\" y=\"34\" width=\"22\" height=\"16\" rx=\"2\"/><path d=\"M25 39h22m-17-3h1m4 0h1m-5 7 3 3-3 2m7 0h4\"/>"
        }
        IconKind::Pdf => {
            "<path d=\"M28 49c6-8 10-20 7-17-4 4 3 14 11 15 6 1-14-6-20 0-5 6 4 3 8-1\"/>"
        }
        IconKind::Font => "<path d=\"m27 49 8-15 8 15M30 44h10M25 49h5m10 0h5\"/>",
        IconKind::Link => {
            "<path d=\"m33 46-3 3a5 5 0 0 1-7-7l5-5a5 5 0 0 1 7 0m4 2 3-3a5 5 0 0 1 7 7l-5 5a5 5 0 0 1-7 0m-6-1 10-10\"/>"
        }
    };
    let svg = template
        .replace("@PAPER@", &hex(blend(key.foreground, key.background, 14)))
        .replace("@FOLD@", &hex(blend(key.accent, key.background, 55)))
        .replace("@OUTLINE@", &hex(blend(key.foreground, key.background, 65)))
        .replace("@ACCENT@", &hex(key.accent))
        .replace("@INK@", &hex(key.background))
        .replace("@SYMBOL@", symbol);
    let tree =
        usvg::Tree::from_str(&svg, &usvg::Options::default()).context("Invalid bundled icon")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(key.width, key.height).context("Cannot allocate icon")?;
    let [r, g, b] = key.background;
    pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
    let size = key.width.min(key.height) as f32;
    let transform = tiny_skia::Transform::from_translate(
        (key.width as f32 - size) / 2.0,
        (key.height as f32 - size) / 2.0,
    )
    .pre_scale(size / 64.0, size / 64.0);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    RgbaImage::from_raw(key.width, key.height, pixmap.take()).context("Invalid icon buffer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn all_embedded_icons_render_at_different_sizes_and_backgrounds() {
        for kind in IconKind::ALL {
            for size in [8, 16, 32, 64] {
                for background in [[0, 0, 0], [239, 242, 246]] {
                    let slot = IconSlot {
                        kind,
                        area: Rect::new(0, 0, 1, 1),
                        foreground: [110, 120, 130],
                        accent: [255, 82, 46],
                        background,
                    };
                    let image = rasterize(IconKey::new(&slot, size, size)).unwrap();
                    assert_eq!(image.dimensions(), (size, size));
                    assert!(image.pixels().all(|pixel| pixel[3] == 255));
                    assert!(image.pixels().any(|pixel| pixel.0[..3] != background));
                    assert_eq!(&image.get_pixel(0, 0).0[..3], &background);
                }
            }
        }
    }
}

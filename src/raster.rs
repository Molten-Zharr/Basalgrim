//! Fixed-cell Fira Code rasterization, following Almavorn's shared GUI/TUI approach.
use crate::preferences::FontFace;
use ratatui::{buffer::Cell, text::Span};
use rustc_hash::FxHashSet;
use soft_ratatui::{EmbeddedTTF, RasterBackend, RgbPixmap, SoftBackend, rusttype::Font};

pub struct CellFont {
    inner: EmbeddedTTF,
    cell: RgbPixmap,
    blink_cells: FxHashSet<(u16, u16)>,
}
impl RasterBackend for CellFont {
    fn draw_cell(
        &mut self,
        x: u16,
        y: u16,
        cell: &Cell,
        always_redraw_list: &mut FxHashSet<(u16, u16)>,
        blinking_fast: bool,
        blinking_slow: bool,
        char_width: usize,
        char_height: usize,
        pixmap: &mut RgbPixmap,
    ) {
        let width = char_width * Span::raw(cell.symbol()).width().max(1);
        if self.cell.width < width || self.cell.height != char_height {
            self.cell = RgbPixmap::new(width, char_height);
        }
        self.blink_cells.clear();
        self.inner.draw_cell(
            0,
            0,
            cell,
            &mut self.blink_cells,
            blinking_fast,
            blinking_slow,
            width,
            char_height,
            &mut self.cell,
        );
        if self.blink_cells.is_empty() {
            always_redraw_list.remove(&(x, y));
        } else {
            always_redraw_list.insert((x, y));
        }
        let left = usize::from(x) * char_width;
        let top = usize::from(y) * char_height;
        let width = width.min(pixmap.width.saturating_sub(left));
        for row in 0..char_height.min(pixmap.height.saturating_sub(top)) {
            let source = row * self.cell.width * 3;
            let destination = ((top + row) * pixmap.width + left) * 3;
            pixmap.data[destination..destination + width * 3]
                .copy_from_slice(&self.cell.data[source..source + width * 3]);
        }
    }
}
pub fn backend(size: u16, columns: u16, rows: u16) -> SoftBackend<CellFont> {
    font_backend(FontFace::default(), size, columns, rows)
}

/// Composite onto a copy: the cached terminal raster remains clean between frames.
pub fn compose_frame(
    backend: &SoftBackend<CellFont>,
    slots: &[crate::icons::IconSlot],
    icons: &mut crate::icons::images::IconRasterizer,
    selection: Option<ratatui::layout::Rect>,
    accent: [u8; 3],
) -> anyhow::Result<Vec<u8>> {
    let mut rgb = compose_icons(backend, slots, icons)?;
    if let Some(area) = selection {
        let area = area.intersection(backend.buffer.area);
        if !area.is_empty() {
            let width = backend.get_pixmap_width();
            let left = usize::from(area.x) * backend.char_width;
            let right = usize::from(area.right()) * backend.char_width - 1;
            let top = usize::from(area.y) * backend.char_height;
            let bottom = usize::from(area.bottom()) * backend.char_height - 1;
            let mut pixel = |x: usize, y: usize| {
                let i = (y * width + x) * 3;
                rgb[i..i + 3].copy_from_slice(&accent);
            };
            for x in left..=right {
                pixel(x, top);
                pixel(x, bottom);
            }
            for y in top..=bottom {
                pixel(left, y);
                pixel(right, y);
            }
        }
    }
    Ok(rgb)
}

pub fn compose_icons(
    backend: &SoftBackend<CellFont>,
    slots: &[crate::icons::IconSlot],
    icons: &mut crate::icons::images::IconRasterizer,
) -> anyhow::Result<Vec<u8>> {
    let mut rgb = backend.get_pixmap_data().to_vec();
    let width = backend.get_pixmap_width();
    let height = backend.get_pixmap_height();
    for slot in slots {
        let image = icons.image(crate::icons::images::IconKey::new(
            slot,
            backend.char_width as u32,
            backend.char_height as u32,
        ))?;
        let left = usize::from(slot.area.x) * backend.char_width;
        let top = usize::from(slot.area.y) * backend.char_height;
        for y in 0..(image.height() as usize).min(height.saturating_sub(top)) {
            for x in 0..(image.width() as usize).min(width.saturating_sub(left)) {
                let pixel = image.get_pixel(x as u32, y as u32);
                let offset = ((top + y) * width + left + x) * 3;
                rgb[offset..offset + 3].copy_from_slice(&pixel.0[..3]);
            }
        }
    }
    Ok(rgb)
}
pub fn font_backend(face: FontFace, size: u16, columns: u16, rows: u16) -> SoftBackend<CellFont> {
    let bytes: &'static [u8] = match face {
        FontFace::FiraCode => include_bytes!("../assets/fonts/FiraCode-Regular.ttf"),
        FontFace::FiraCodeBold => include_bytes!("../assets/fonts/FiraCode-Bold.ttf"),
        FontFace::DejaVuMono => include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
    };
    let regular = Font::try_from_bytes(bytes).expect("Bundled font is valid");
    let bold = Font::try_from_bytes(include_bytes!("../assets/fonts/FiraCode-Bold.ttf"))
        .expect("Bundled Fira Code Bold is valid");
    let backend = SoftBackend::<EmbeddedTTF>::new(
        columns,
        rows,
        u32::from(size),
        regular,
        if face == FontFace::DejaVuMono {
            None
        } else {
            Some(bold)
        },
        None,
    );
    SoftBackend {
        raster_backend: CellFont {
            inner: backend.raster_backend,
            cell: RgbPixmap::new(backend.char_width, backend.char_height),
            blink_cells: FxHashSet::default(),
        },
        buffer: backend.buffer,
        cursor: backend.cursor,
        cursor_pos: backend.cursor_pos,
        cursor_config: backend.cursor_config,
        char_width: backend.char_width,
        char_height: backend.char_height,
        frame_count: backend.frame_count,
        blink_config: backend.blink_config,
        rgb_pixmap: backend.rgb_pixmap,
        always_redraw_list: backend.always_redraw_list,
        rendered_cursor: backend.rendered_cursor,
    }
}

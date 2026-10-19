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

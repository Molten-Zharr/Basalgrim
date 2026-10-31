use super::ImagePreview;
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

const MAX_INPUT: u64 = 32 * 1024 * 1024;
const MAX_DECODED: u64 = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;

pub struct Decoded {
    pub details: String,
    pub image: Option<Arc<ImagePreview>>,
}

/// Sniff the contents, including images with no extension. Return None for text.
pub fn read(
    path: &Path,
    file: &mut File,
    prefix: &[u8],
    graphics: bool,
) -> Option<Result<Decoded>> {
    let svg = path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("svg"))
        || std::str::from_utf8(prefix).is_ok_and(|s| {
            let s = s.trim_start();
            s.starts_with("<svg") || s.starts_with("<?xml") && s.contains("<svg")
        });
    let format = image::guess_format(prefix)
        .ok()
        .or_else(|| ImageFormat::from_path(path).ok());
    if !svg && format.is_none() {
        return None;
    }
    Some((|| {
        let limit = if svg { 1024 * 1024 } else { MAX_INPUT };
        ensure!(
            file.metadata()?.len() <= limit,
            "Предпросмотр ограничен: файл больше {} MiB",
            limit / (1024 * 1024)
        );
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= limit, "Превышен размер предпросмотра");
        if svg {
            return vector(&bytes, graphics);
        }
        let format = format.unwrap();
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(16_384);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(MAX_DECODED);
        reader.limits(limits);
        let mut decoder = reader
            .into_decoder()
            .context("Не удалось прочитать изображение")?;
        let (width, height) = decoder.dimensions();
        ensure!(
            width > 0
                && height > 0
                && u64::from(width) * u64::from(height) <= MAX_PIXELS
                && decoder.total_bytes() <= MAX_DECODED,
            "Предпросмотр ограничен: больше 16 мегапикселей / 64 MiB"
        );
        let orientation = decoder.orientation()?;
        use image::metadata::Orientation;
        let mut dimensions = if matches!(
            orientation,
            Orientation::Rotate90
                | Orientation::Rotate270
                | Orientation::Rotate90FlipH
                | Orientation::Rotate270FlipH
        ) {
            (height, width)
        } else {
            (width, height)
        };
        let image = if graphics {
            let mut image =
                DynamicImage::from_decoder(decoder).context("Повреждённое изображение")?;
            image.apply_orientation(orientation);
            dimensions = (image.width(), image.height());
            let image = if image.width() > 1024 || image.height() > 1024 {
                image.thumbnail(1024, 1024)
            } else {
                image
            };
            let rgba = image.into_rgba8();
            Some(Arc::new(ImagePreview::new(
                rgba.width(),
                rgba.height(),
                rgba.into_raw(),
            )?))
        } else {
            None
        };
        let first = if matches!(
            format,
            ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Tiff
        ) {
            " · первый кадр"
        } else {
            ""
        };
        Ok(Decoded {
            details: format!("{format:?} · {} × {}{first}", dimensions.0, dimensions.1),
            image,
        })
    })())
}

fn vector(bytes: &[u8], graphics: bool) -> Result<Decoded> {
    use resvg::{tiny_skia, usvg};
    // Bound parser work as well as the raster, and never resolve file/data URLs.
    ensure!(
        bytes.iter().filter(|&&b| b == b'<').count() <= 20_000,
        "Слишком сложный SVG для предпросмотра"
    );
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new(|_, _| None),
            resolve_data: Box::new(|_, _, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).context("Не удалось прочитать SVG")?;
    let size = tree.size();
    let scale = (1024.0 / size.width().max(size.height())).min(1.0);
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let image = if graphics {
        let mut pixmap =
            tiny_skia::Pixmap::new(width, height).context("Не удалось выделить память SVG")?;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // tiny-skia stores premultiplied colors; the shared compositor expects straight RGBA.
        let mut pixels = pixmap.take();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            if pixel[3] != 0 {
                for channel in 0..3 {
                    pixel[channel] =
                        ((u16::from(pixel[channel]) * 255) / u16::from(pixel[3])).min(255) as u8;
                }
            }
        }
        Some(Arc::new(ImagePreview::new(width, height, pixels)?))
    } else {
        None
    };
    Ok(Decoded {
        details: format!("SVG · {} × {}", size.width(), size.height()),
        image,
    })
}

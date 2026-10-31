#![cfg(any(feature = "guitui", feature = "tui"))]
use basalgrim::{
    app::App,
    fs,
    icons::IconBackend,
    input::Action,
    preferences::IconMode,
    preview::{PreviewSlot, images::PreviewRasterizer},
    settings::Settings,
    ui,
};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs as stdfs,
    sync::Arc,
    time::{Duration, Instant},
};

fn png(path: &std::path::Path, color: [u8; 4]) {
    RgbaImage::from_pixel(32, 16, Rgba(color))
        .save_with_format(path, ImageFormat::Png)
        .unwrap();
}
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end, "preview worker timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App, width: u16) {
    Terminal::new(TestBackend::new(width, 40))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}

#[test]
fn raster_formats_are_detected_from_contents_with_bounded_shared_pixels() {
    let root = tempfile::tempdir().unwrap();
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Ico,
    ] {
        let path = root.path().join(format!("{format:?}"));
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(32, 16, Rgba([210, 80, 40, 255])));
        let image = if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image.into_rgb8())
        } else {
            image
        };
        image.save_with_format(&path, format).unwrap();
        let preview = fs::preview(&path).unwrap();
        assert!(
            preview.details.iter().any(|d| d.contains("32 × 16")),
            "{format:?}: {:?}",
            preview.lines
        );
        let image = preview.image.as_ref().unwrap();
        assert!(image.width <= 1024 && image.height <= 1024);
        assert_eq!(image.width, 2 * image.height);
        let cloned = preview.clone();
        assert!(Arc::ptr_eq(image, cloned.image.as_ref().unwrap()));
        let metadata = fs::preview_with_images(&path, false).unwrap();
        assert!(metadata.image.is_none());
        assert!(metadata.details.iter().any(|s| s.contains("32 × 16")));
        assert!(
            metadata
                .lines
                .iter()
                .any(|s| s.contains("не поддерживает графику"))
        );
    }
}

#[test]
fn gif_uses_first_frame_and_jpeg_honors_exif_orientation() {
    use image::ImageEncoder;
    let root = tempfile::tempdir().unwrap();
    let gif = root.path().join("animation.gif");
    let mut encoder = image::codecs::gif::GifEncoder::new(stdfs::File::create(&gif).unwrap());
    encoder
        .encode_frames(
            [[240, 20, 10, 255], [10, 220, 30, 255]]
                .map(|color| image::Frame::new(RgbaImage::from_pixel(32, 16, Rgba(color)))),
        )
        .unwrap();
    drop(encoder);
    let preview = fs::preview(&gif).unwrap();
    assert!(preview.details.iter().any(|d| d.contains("первый кадр")));
    assert!(
        preview
            .image
            .unwrap()
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [240, 20, 10, 255])
    );
    let jpeg = root.path().join("rotated.jpg");
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(stdfs::File::create(&jpeg).unwrap());
    encoder
        .set_exif_metadata(
            b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec(),
        )
        .unwrap();
    encoder
        .write_image(&[220; 32 * 16 * 3], 32, 16, image::ExtendedColorType::Rgb8)
        .unwrap();
    let preview = fs::preview(&jpeg).unwrap();
    assert!(preview.details.iter().any(|d| d.contains("16 × 32")));
    let image = preview.image.unwrap();
    assert_eq!(image.height, 2 * image.width);
    assert!(
        fs::preview_with_images(&jpeg, false)
            .unwrap()
            .details
            .iter()
            .any(|s| s.contains("16 × 32"))
    );
}

#[test]
fn svg_is_bounded_and_does_not_read_referenced_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("vector.svg");
    let external = root.path().join("external.png");
    png(&external, [0, 255, 0, 255]);
    stdfs::write(&path, format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="2500"><rect width="5000" height="2500" fill="red"/><image href="{}" width="5000" height="2500"/></svg>"#, external.display())).unwrap();
    let preview = fs::preview(&path).unwrap();
    assert!(preview.details.iter().any(|s| s.contains("5000 × 2500")));
    let image = preview.image.unwrap();
    assert_eq!((image.width, image.height), (1024, 512));
    assert!(
        image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255])
    );
    stdfs::write(&path, format!("<svg>{}</svg>", "<g/>".repeat(20_001))).unwrap();
    assert!(fs::preview(&path).unwrap().image.is_none());
}

#[test]
fn corrupt_unsupported_oversized_and_special_files_leave_useful_metadata() {
    let root = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("bad.png", b"\x89PNG\r\n\x1a\n".as_slice()),
        ("unsupported.avif", b"invalid"),
    ] {
        let path = root.path().join(name);
        stdfs::write(&path, bytes).unwrap();
        let preview = fs::preview(&path).unwrap();
        assert!(preview.image.is_none());
        assert!(!preview.details.is_empty());
        assert!(preview.lines[0].contains("недоступен"));
    }
    let huge = root.path().join("huge.png");
    png(&huge, [0; 4]);
    stdfs::OpenOptions::new()
        .write(true)
        .open(&huge)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let preview = fs::preview(&huge).unwrap();
    assert!(preview.image.is_none());
    assert!(preview.lines.iter().any(|s| s.contains("32 MiB")));
    let bomb = root.path().join("bomb.gif");
    stdfs::write(&bomb, b"GIF89a\xff\xff\xff\xff\0\0\0;").unwrap();
    assert!(fs::preview(&bomb).unwrap().image.is_none());
    let text = root.path().join("notes.md");
    stdfs::write(&text, "Example: <svg> in documentation\n").unwrap();
    assert!(fs::preview(&text).unwrap().lines[0].contains("Example"));
    #[cfg(target_os = "linux")]
    {
        let pipe = root.path().join("pipe.png");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &pipe,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR,
            0,
        )
        .unwrap();
        assert!(fs::preview(&pipe).unwrap().image.is_none());
        let link = root.path().join("link.png");
        std::os::unix::fs::symlink(&huge, &link).unwrap();
        assert_eq!(fs::preview(&link).unwrap().lines[0], "Символическая ссылка");
    }
}

#[test]
fn composition_preserves_aspect_transparency_and_theme_background() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("alpha.png");
    png(&path, [240, 0, 0, 128]);
    let image = fs::preview(&path).unwrap().image.unwrap();
    let mut rasterizer = PreviewRasterizer::default();
    let mut slot = PreviewSlot {
        image,
        area: Rect::new(2, 3, 10, 10),
        background: [20, 40, 60],
    };
    let canvas = rasterizer.image(&slot, 8, 16).unwrap();
    assert_eq!(canvas.dimensions(), (80, 160));
    assert_eq!(canvas.get_pixel(0, 0).0, [20, 40, 60, 255]);
    let center = canvas.get_pixel(40, 80).0;
    assert!((129..=131).contains(&center[0]));
    assert!((19..=20).contains(&center[1]));
    assert_eq!(canvas.get_pixel(40, 59).0, [20, 40, 60, 255]);
    assert_ne!(canvas.get_pixel(40, 60).0, [20, 40, 60, 255]);
    slot.background = [200, 210, 220];
    assert_eq!(
        rasterizer.image(&slot, 8, 16).unwrap().get_pixel(0, 0).0,
        [200, 210, 220, 255]
    );
    assert_eq!(
        rasterizer.image(&slot, 16, 32).unwrap().dimensions(),
        (160, 320)
    );
}

#[test]
fn backend_changes_and_fast_selection_accept_only_current_preview() {
    let root = tempfile::tempdir().unwrap();
    png(&root.path().join("a.png"), [255, 0, 0, 255]);
    png(&root.path().join("b.png"), [0, 255, 0, 255]);
    stdfs::write(root.path().join("c.txt"), "current text").unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    assert!(app.preview.as_ref().unwrap().image.is_none());
    app.icon_backend = IconBackend::Gui;
    app.settings.appearance.icon_mode = IconMode::Symbols;
    app.update_preview();
    settle(&mut app);
    render(&mut app, 150);
    assert!(app.preview_slot.is_some());
    assert!(app.icon_slots.is_empty());
    for i in 0..30 {
        app.tab_mut().selected = i % 3;
        app.update_preview();
    }
    settle(&mut app);
    render(&mut app, 150);
    assert!(
        app.preview
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .any(|s| s.contains("current text"))
    );
    assert!(app.preview_slot.is_none());
    app.tab_mut().selected = 1;
    app.update_preview();
    settle(&mut app);
    render(&mut app, 150);
    assert!(
        app.preview_slot
            .as_ref()
            .unwrap()
            .image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 255, 0, 255])
    );
    app.icon_backend = IconBackend::Symbols;
    app.update_preview();
    settle(&mut app);
    render(&mut app, 150);
    assert!(app.preview_slot.is_none());
    assert!(app.preview.as_ref().unwrap().image.is_none());
}

#[test]
fn preview_hides_for_modals_small_windows_and_disabled_preview() {
    let root = tempfile::tempdir().unwrap();
    png(&root.path().join("a.png"), [255, 0, 0, 255]);
    let mut app = App::new(root.path().into(), Settings::default());
    app.icon_backend = IconBackend::Gui;
    settle(&mut app);
    render(&mut app, 150);
    let slot = app.preview_slot.clone().unwrap();
    app.handle(basalgrim::input::Input::Pointer(Some(
        (slot.area.x, slot.area.y).into(),
    )));
    app.action(Action::Commands);
    render(&mut app, 150);
    assert!(app.preview_slot.is_none());
    app.cancel_dialog();
    for action in [Action::Help, Action::Settings, Action::Operations] {
        app.action(action);
        render(&mut app, 150);
        // Help only suppresses the image if its actual rectangle intersects it.
        if action != Action::Help {
            assert!(app.preview_slot.is_none());
        }
        app.cancel_dialog();
    }
    render(&mut app, 34);
    assert!(app.preview_slot.is_none());
    render(&mut app, 150);
    assert!(app.preview_slot.is_some());
    app.action(Action::Preview);
    settle(&mut app);
    render(&mut app, 150);
    assert!(app.preview_slot.is_none());
}

#[cfg(feature = "guitui")]
#[test]
fn gui_composition_is_clean_after_same_geometry_image_change_and_hiding() {
    use basalgrim::raster;
    let root = tempfile::tempdir().unwrap();
    png(&root.path().join("a.png"), [255, 0, 0, 255]);
    let mut app = App::new(root.path().into(), Settings::default());
    app.icon_backend = IconBackend::Gui;
    app.settings.appearance.icon_mode = IconMode::Symbols;
    settle(&mut app);
    let mut terminal = Terminal::new(raster::backend(16, 150, 40)).unwrap();
    let mut icons = Default::default();
    let mut previews = Default::default();
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let clean = terminal.backend().get_pixmap_data().to_vec();
    let red = raster::compose_frame(
        terminal.backend(),
        &[],
        &mut icons,
        app.preview_slot.as_ref(),
        &mut previews,
        None,
        [0; 3],
    )
    .unwrap();
    assert_ne!(red, clean);
    assert_eq!(clean, terminal.backend().get_pixmap_data());
    png(&root.path().join("a.png"), [0, 255, 0, 255]);
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if app
            .preview
            .as_ref()
            .and_then(|p| p.image.as_ref())
            .is_some_and(|i| i.pixels[0] == 0)
        {
            break;
        }
        assert!(Instant::now() < end, "same-path watched image update");
        std::thread::sleep(Duration::from_millis(10));
    }
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let green = raster::compose_frame(
        terminal.backend(),
        &[],
        &mut icons,
        app.preview_slot.as_ref(),
        &mut previews,
        None,
        [0; 3],
    )
    .unwrap();
    assert_ne!(red, green);
    let restored = raster::compose_frame(
        terminal.backend(),
        &[],
        &mut icons,
        None,
        &mut previews,
        None,
        [0; 3],
    )
    .unwrap();
    assert_eq!(restored, terminal.backend().get_pixmap_data());
}

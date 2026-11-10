//! Render a deterministic PTY screen through the real shared UI (no shell launched).
use anyhow::Result;
use basalgrim::{
    app::App,
    preferences::Theme,
    raster,
    settings::Settings,
    terminal::{Replies, Snapshot},
    ui,
};
use ratatui::Terminal;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let out = PathBuf::from("target/preview");
    fs::create_dir_all(&out)?;
    let dir = tempfile::tempdir()?;
    fs::write(dir.path().join("hello.rs"), "fn main() {}")?;
    fs::create_dir(dir.path().join("src"))?;
    let mut settings = Settings {
        preview: false,
        grid: false,
        ..Default::default()
    };
    settings.appearance.theme = Theme::MoltenZharr;
    let mut app = App::new(dir.path().to_path_buf(), settings);
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        anyhow::ensure!(Instant::now() < end, "startup timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    app.terminal.visible = true;
    app.terminal.focused = true;
    app.terminal.directory = PathBuf::from("/home/adahn/Projects/Basalgrim");
    for (name, w, h) in [
        ("terminal", 150, 38),
        ("terminal-compact", 34, 14),
        ("terminal-native", 170, 53),
    ] {
        if name == "terminal-native" {
            app.settings.appearance.theme = Theme::Light;
        }
        let mut renderer = Terminal::new(raster::font_backend(
            app.settings.appearance.font,
            app.settings.appearance.font_size,
            w,
            h,
        ))?;
        renderer.draw(|f| ui::render(&mut app, f))?;
        let a = app.terminal.area;
        let mut parser = vt100::Parser::new_with_callbacks(
            a.height.try_into()?,
            a.width.try_into()?,
            2000,
            Replies::default(),
        );
        let text = if name == "terminal-native" {
            "\x1b[48;2;17;199;111mCOLOR_OK\x1b[0m\r\nBASALGRIM> "
        } else {
            "\x1b[38;2;90;180;140madahn@basalgrim\x1b[0m ~/Projects/Basalgrim\r\n$ ls\r\n\x1b[34msrc\x1b[0m  hello.rs\r\n$ printf 'Привет, мир!\\n'\r\nПривет, мир!\r\n$ "
        };
        parser.process(text.as_bytes());
        app.terminal.snapshot = Snapshot::capture(&parser, "", true, None);
        renderer.draw(|f| ui::render(&mut app, f))?;
        let b = renderer.backend();
        let pixels = raster::compose_frame(
            b,
            &app.icon_slots,
            &mut Default::default(),
            None,
            &mut Default::default(),
            None,
            [255, 80, 30],
        )?;
        image::save_buffer(
            out.join(format!("{name}.png")),
            &pixels,
            b.get_pixmap_width() as u32,
            b.get_pixmap_height() as u32,
            image::ColorType::Rgb8,
        )?;
        println!("{name}: {}x{} at {},{}", a.width, a.height, a.x, a.y);
    }
    Ok(())
}

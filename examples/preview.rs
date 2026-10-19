//! Render the actual shared interface without a desktop session.
use anyhow::{Result, ensure};
use basalgrim::{
    app::{App, SettingsFocus, SettingsSection, SettingsTab, Target},
    input::{Action, Input},
    preferences::Theme,
    raster,
    settings::Settings,
    ui,
};
use ratatui::Terminal;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

fn settle(app: &mut App) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        ensure!(Instant::now() < deadline, "Background request timed out");
        app.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn render(app: &mut App, out: &Path, name: &str, width: u16, height: u16) -> Result<()> {
    let mut terminal = Terminal::new(raster::font_backend(
        app.settings.appearance.font,
        app.settings.appearance.font_size,
        width,
        height,
    ))?;
    terminal.draw(|frame| ui::render(app, frame))?;
    let backend = terminal.backend();
    let mut file = fs::File::create(out.join(format!("{name}.ppm")))?;
    write!(
        file,
        "P6\n{} {}\n255\n",
        backend.get_pixmap_width(),
        backend.get_pixmap_height()
    )?;
    file.write_all(backend.get_pixmap_data())?;
    Ok(())
}
fn main() -> Result<()> {
    let out = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/preview".into());
    fs::create_dir_all(&out)?;
    let demo = tempfile::Builder::new().prefix("Molten-Zharr-").tempdir()?;
    for name in [
        "Applications",
        "Documents",
        "Downloads",
        "Music",
        "Pictures",
        "Projects",
        "Videos",
    ] {
        fs::create_dir(demo.path().join(name))?;
    }
    fs::write(
        demo.path().join("browser.rs"),
        "use ratatui::Frame;\n\nfn render(frame: &mut Frame) {\n    // One interface. Two backends.\n    let area = frame.area();\n    draw_sidebar(frame, area);\n    draw_files(frame, area);\n    draw_preview(frame, area);\n}\n",
    )?;
    fs::write(
        demo.path().join("Cargo.toml"),
        include_str!("../Cargo.toml"),
    )?;
    fs::write(
        demo.path().join("notes.md"),
        "# Molten-Zharr\n\nRust + Ratatui + egui\n",
    )?;
    fs::write(demo.path().join(".hidden"), "hidden")?;
    let mut app = App::new(demo.path().to_path_buf(), Settings::default());
    settle(&mut app)?;
    let index = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "browser.rs")
        .unwrap();
    app.tab_mut().selected = index;
    app.update_preview();
    settle(&mut app)?;
    render(&mut app, &out, "grid", 150, 44)?;
    let hover = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { pane: 0, index: 0 }).then_some(*area)
        })
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        hover.x, hover.y,
    ))));
    render(&mut app, &out, "grid-hover", 150, 44)?;
    app.handle(Input::Pointer(None));
    app.action(Action::Grid);
    render(&mut app, &out, "list", 150, 44)?;
    let hover = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { pane: 0, index: 0 }).then_some(*area)
        })
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        hover.x, hover.y,
    ))));
    render(&mut app, &out, "list-hover", 150, 44)?;
    app.handle(Input::Pointer(None));
    app.action(Action::Dual);
    render(&mut app, &out, "dual", 190, 44)?;
    render(&mut app, &out, "compact", 70, 26)?;
    render(&mut app, &out, "minimum", 34, 14)?;
    app.action(Action::Help);
    render(&mut app, &out, "help", 100, 34)?;
    app.cancel_dialog();
    app.action(Action::Settings);
    let settings_path = demo.path().join("settings.json");
    app.settings_dirty = true;
    app.persist_preferences(Some(&settings_path));
    render(&mut app, &out, "settings-presets", 120, 36)?;
    app.settings_tab(SettingsTab::Palette);
    app.settings_view.focus = SettingsFocus::Parameters;
    render(&mut app, &out, "settings-palette", 120, 36)?;
    app.settings_tab(SettingsTab::Typography);
    app.settings_view.focus = SettingsFocus::Parameters;
    render(&mut app, &out, "settings-fonts", 120, 36)?;
    let hover = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(
                target,
                Target::Setting {
                    index: 1,
                    activate: true,
                    ..
                }
            )
            .then_some(*area)
        })
        .unwrap();
    app.handle(Input::Pointer(Some(ratatui::layout::Position::new(
        hover.x, hover.y,
    ))));
    render(&mut app, &out, "settings-hover", 120, 36)?;
    app.handle(Input::Pointer(None));
    render(&mut app, &out, "settings-compact", 50, 18)?;
    app.settings_section(SettingsSection::Shortcuts);
    app.settings_view.focus = SettingsFocus::Parameters;
    app.settings_view.selected = 6;
    render(&mut app, &out, "settings-shortcuts", 120, 36)?;
    app.setting(6, true, 1);
    render(&mut app, &out, "settings-capture", 120, 36)?;
    app.cancel_dialog();
    app.cancel_dialog();
    app.settings.dual = false;
    app.settings.appearance.theme = Theme::Light;
    render(&mut app, &out, "light", 150, 44)?;
    println!("Rendered GUI/TUI previews to {}", out.display());
    Ok(())
}

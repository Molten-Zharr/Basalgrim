//! The README's ordinary browsing scene, independent of feature illustrations.
use anyhow::{Result, ensure};
use basalgrim::{app::App, icons::IconBackend, preferences::Theme, raster, settings::Settings, ui};
use ratatui::Terminal;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let out = PathBuf::from("target/preview/basalgrim-overview.png");
    fs::create_dir_all(out.parent().unwrap())?;
    let sandbox = tempfile::Builder::new().prefix("Basalgrim-").tempdir()?;
    let directory = sandbox.path().join("Файлы");
    fs::create_dir(&directory)?;
    for name in [
        "Архивы",
        "Документы",
        "Загрузки",
        "Изображения",
        "Музыка",
        "Проекты",
        "Рабочий стол",
        "Шаблоны",
    ] {
        fs::create_dir(directory.join(name))?;
    }
    for (name, contents) in [
        ("Заметки.md", "# Заметки\n\nИдеи и планы на неделю.\n"),
        ("План.txt", "Документы\nРабочие проекты\nРезервные копии\n"),
        (
            "Задачи.csv",
            "Задача,Статус\nРазобрать документы,В работе\n",
        ),
        ("example.rs", "fn main() {\n    println!(\"Привет!\");\n}\n"),
        (
            "Cargo.toml",
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\n",
        ),
        ("config.json", "{\n  \"language\": \"ru\"\n}\n"),
        (
            "backup.sh",
            "#!/bin/sh\n# Сценарий резервного копирования\n",
        ),
        ("LICENSE", include_str!("../LICENSE")),
    ] {
        fs::write(directory.join(name), contents)?;
    }
    let mut settings = Settings {
        preview: false,
        grid: true,
        dual: false,
        ..Default::default()
    };
    // Never load the user's settings or session for the public overview.
    settings.appearance.theme = Theme::MoltenZharr;
    let mut app = App::new(directory, settings);
    app.icon_backend = IconBackend::Gui;
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        ensure!(Instant::now() < deadline, "Overview directory did not load");
        app.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.tab_mut().selected = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "Документы")
        .unwrap();
    let mut terminal = Terminal::new(raster::font_backend(
        app.settings.appearance.font,
        app.settings.appearance.font_size,
        150,
        38,
    ))?;
    terminal.draw(|frame| ui::render(&mut app, frame))?;
    let backend = terminal.backend();
    let pixels = raster::compose_frame(
        backend,
        &app.icon_slots,
        &mut Default::default(),
        None,
        &mut Default::default(),
        None,
        app.settings.appearance.palette()["selection_accent"],
    )?;
    image::save_buffer(
        &out,
        &pixels,
        backend.get_pixmap_width() as u32,
        backend.get_pixmap_height() as u32,
        image::ColorType::Rgb8,
    )?;
    println!("{}", out.display());
    Ok(())
}

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
    file.write_all(&raster::compose_frame(
        backend,
        &app.icon_slots,
        &mut Default::default(),
        app.selection_area(),
        app.settings.appearance.palette()["selection_accent"],
    )?)?;
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
    app.icon_backend = basalgrim::icons::IconBackend::Gui;
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
    let file = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { index: i, .. } if *i == index).then_some(*area)
        })
        .unwrap();
    app.handle(Input::SecondaryClick {
        x: file.x,
        y: file.y,
    });
    render(&mut app, &out, "context-menu", 150, 44)?;
    app.cancel_dialog();
    render(&mut app, &out, "grid", 150, 44)?;
    let first = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { pane: 0, index: 0 }).then_some(*area)
        })
        .unwrap();
    let second = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { pane: 0, index: 1 }).then_some(*area)
        })
        .unwrap();
    app.handle(Input::PointerDown {
        x: second.right() - 1,
        y: second.bottom() - 1,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: first.x,
        y: first.y,
    });
    render(&mut app, &out, "selection-grid", 150, 44)?;
    app.handle(Input::CancelDrag);
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
    for name in [
        "balena-etcher_2.1.6_amd64.AppImage",
        "batocera-rk3588-sdimage-20261023.img.gz",
        "cloudflared-linux-amd64.deb",
        "detect_record(1).py",
        "detect_record(2).py",
        "discord-1.0.154.deb",
        "drone-detect(1).service",
        "FreeCAD_1.1.1-Linux-x86_64.AppImage",
        "google-chrome-stable_current_amd64.deb",
        "README_WITH_AN_EXTREMELY_LONG_NAME",
        "Очень-длинное-название-документа.txt",
        "資料ファイルの非常に長い名前.txt",
    ] {
        fs::write(demo.path().join(name), "")?;
    }
    app.settings.grid = true;
    app.settings.preview = false;
    app.refresh();
    settle(&mut app)?;
    render(&mut app, &out, "grid-long-names-light", 132, 44)?;
    app.settings.appearance.theme = Theme::MoltenZharr;
    render(&mut app, &out, "grid-long-names", 132, 44)?;
    for name in [
        "album.flac",
        "archive.zip",
        "Font.ttf",
        "manual.pdf",
        "movie.mkv",
        "photo.png",
    ] {
        fs::write(demo.path().join(name), "")?;
    }
    std::os::unix::fs::symlink("browser.rs", demo.path().join("browser-link.rs"))?;
    app.refresh();
    settle(&mut app)?;
    app.settings.appearance.font_size = 20;
    app.settings.appearance.theme = Theme::Light;
    render(&mut app, &out, "icons-light", 132, 52)?;
    app.settings.grid = false;
    render(&mut app, &out, "icons-list", 132, 44)?;
    app.action(Action::Settings);
    app.settings_tab(SettingsTab::Icons);
    app.settings_view.focus = SettingsFocus::Parameters;
    render(&mut app, &out, "settings-icons", 120, 36)?;
    app.cancel_dialog();
    let destination = demo.path().join("Operations");
    fs::create_dir(&destination)?;
    app.start_operation(basalgrim::fs::Operation::Transfer {
        sources: vec![demo.path().join("browser.rs"), demo.path().join("notes.md")],
        destination: destination.clone(),
        cut: false,
    });
    settle(&mut app)?;
    app.start_operation(basalgrim::fs::Operation::Transfer {
        sources: vec![demo.path().join("browser.rs"), demo.path().join("notes.md")],
        destination,
        cut: false,
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(app.dialog, Some(basalgrim::app::Dialog::Conflict(_))) {
        ensure!(Instant::now() < deadline, "Conflict prompt timed out");
        app.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    render(&mut app, &out, "conflict", 120, 36)?;
    render(&mut app, &out, "conflict-compact", 34, 14)?;
    app.toggle_conflict_remaining();
    app.choose_conflict(basalgrim::conflicts::Choice::Skip);
    settle(&mut app)?;
    app.action(Action::Operations);
    app.operations_selected = 1;
    render(&mut app, &out, "operations", 120, 36)?;
    render(&mut app, &out, "operations-compact", 50, 18)?;
    let mut partial = basalgrim::operations::OperationRecord::new(1000, "Перемещение".into(), 1);
    partial.destination = Some("/media/External".into());
    partial.finish(basalgrim::fs::Report {
        total: 1,
        cancelled: true,
        retained_copies: vec![basalgrim::fs::RetainedCopy {
            source: "/home/user/Documents/Research".into(),
            destination: "/media/External/Research".into(),
            recovery: None,
        }],
        progress: basalgrim::operations::Progress {
            bytes_done: 48 * 1024 * 1024,
            bytes_total: Some(48 * 1024 * 1024),
            entries_done: 24,
            entries_total: Some(24),
            ..Default::default()
        },
        ..Default::default()
    });
    partial.elapsed = Some(Duration::from_secs(12));
    app.operations.push_front(partial);
    app.operations_selected = 0;
    app.operations_detail_offset = 0;
    render(&mut app, &out, "operations-partial", 120, 36)?;
    app.cancel_dialog();
    app.settings.dual = true;
    app.settings.grid = false;
    app.settings.preview = false;
    app.active = 1;
    app.navigate(demo.path().join("Operations"), true);
    settle(&mut app)?;
    app.active = 0;
    app.tab_mut().selected = app
        .tab()
        .visible
        .iter()
        .position(|&i| app.tab().entries[i].name == "browser.rs")
        .unwrap();
    app.tab_mut().marked = [demo.path().join("browser.rs"), demo.path().join("notes.md")].into();
    app.tab_mut().explicit_selection = true;
    render(&mut app, &out, "pane-transfer", 150, 44)?;
    let selected = app.tab().selected;
    let entry = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t, Target::Entry { pane: 0, index } if *index == selected).then_some(*r)
        })
        .unwrap();
    app.handle(Input::SecondaryClick {
        x: entry.x,
        y: entry.y,
    });
    for _ in 0..3 {
        app.handle(Input::Key(basalgrim::input::KeyPress::plain(
            basalgrim::input::Key::Down,
        )));
    }
    render(&mut app, &out, "pane-commands", 150, 44)?;
    render(&mut app, &out, "pane-commands-compact", 34, 14)?;
    app.cancel_dialog();
    app.action(Action::Settings);
    app.settings_section(SettingsSection::Session);
    render(&mut app, &out, "settings-session", 120, 36)?;
    render(&mut app, &out, "settings-session-compact", 34, 14)?;
    let storage = tempfile::tempdir()?;
    let journal_path = storage.path().join("operations.sqlite3");
    let mut history = App::new(
        demo.path().join("Operations"),
        Settings {
            preview: false,
            grid: false,
            ..Settings::default()
        },
    );
    history.attach_journal(journal_path.clone());
    let end = Instant::now() + Duration::from_secs(5);
    while history.journal_loading {
        ensure!(Instant::now() < end, "Journal startup timed out");
        history.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    history.start_operation(basalgrim::fs::Operation::Create {
        parent: demo.path().join("Operations"),
        name: "history-note.txt".into(),
        directory: false,
    });
    settle(&mut history)?;
    history.start_operation(basalgrim::fs::Operation::Duplicate {
        sources: vec![demo.path().join("Operations/history-note.txt")],
        destination: demo.path().join("Operations"),
    });
    settle(&mut history)?;
    drop(history);
    let mut history = App::new(
        demo.path().join("Operations"),
        Settings {
            preview: false,
            grid: false,
            ..Settings::default()
        },
    );
    history.settings.appearance.theme = Theme::Light;
    history.attach_journal(journal_path);
    history.action(Action::Operations);
    let end = Instant::now() + Duration::from_secs(5);
    while history.journal_loading {
        ensure!(Instant::now() < end, "Journal reload timed out");
        history.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    render(&mut history, &out, "journal", 120, 36)?;
    render(&mut history, &out, "journal-compact", 34, 14)?;
    println!("Rendered GUI/TUI previews to {}", out.display());
    Ok(())
}

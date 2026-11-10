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
    while app.loading() || app.journal_loading {
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
        app.preview_slot.as_ref(),
        &mut Default::default(),
        app.gesture_area(),
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
    // Exercise the same pointer-selection input as both frontends.
    for (entry, extend) in [(index - 2, false), (index, true)] {
        let area = app
            .hits
            .iter()
            .find_map(|(r, t)| {
                matches!(t, Target::Entry {pane:0,index:i} if *i==entry).then_some(*r)
            })
            .unwrap();
        app.handle(Input::SelectionModifier(extend));
        app.handle(Input::Click {
            x: area.x + 5,
            y: area.y + 2,
            double: false,
            toggle: false,
        });
    }
    settle(&mut app)?;
    render(&mut app, &out, "range-selection", 150, 44)?;
    app.handle(Input::SelectionModifier(false));
    let area = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Entry {pane:0,index:i} if *i==index).then_some(*r))
        .unwrap();
    app.handle(Input::Click {
        x: area.x + 5,
        y: area.y + 2,
        double: false,
        toggle: false,
    });
    let notice = app.notice.clone();
    let saved_grid = app.settings.grid;
    app.action(Action::Rename);
    render(&mut app, &out, "inline-rename-grid", 150, 44)?;
    app.settings.grid = false;
    render(&mut app, &out, "inline-rename-list", 120, 36)?;
    render(&mut app, &out, "inline-rename-compact", 34, 14)?;
    render(&mut app, &out, "inline-rename-list", 120, 36)?;
    let field = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::TextSelect { .. }).then_some(*r))
        .unwrap();
    app.handle(Input::SecondaryClick {
        x: field.x,
        y: field.y,
    });
    render(&mut app, &out, "text-edit-menu", 120, 36)?;
    app.cancel_dialog();
    app.settings.grid = saved_grid;
    app.message(notice.clone(), false);
    render(&mut app, &out, "grid", 150, 44)?;
    let destination = app
        .hits
        .iter()
        .find_map(|(area, target)| {
            matches!(target, Target::Entry { pane: 0, index: 0 })
                .then_some(ratatui::layout::Position::new(area.x, area.y))
        })
        .unwrap();
    app.hover_external_drop(3, Some(destination), 3, basalgrim::dnd::DropAction::Move);
    render(&mut app, &out, "external-drop", 150, 44)?;
    app.capture_external_drop(3);
    render(&mut app, &out, "external-drop-read", 100, 28)?;
    app.cancel_external_drop();
    app.take_drop_cancellations();
    render(&mut app, &out, "grid", 150, 44)?;
    app.accept_clipboard_event(basalgrim::clipboard::service::Event::Ready(Ok(())));
    app.action(Action::Paste);
    render(&mut app, &out, "file-clipboard", 100, 28)?;
    app.cancel_operation();
    app.accept_clipboard_event(basalgrim::clipboard::service::Event::Ready(Err(
        "preview".into()
    )));
    app.message(notice, false);
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
    // Capture a genuine shared file gesture; no filesystem operation is dispatched.
    let source = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t,Target::Entry {pane:0,index:i} if *i==index).then_some(*r))
        .unwrap();
    let destination = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Entry { pane: 0, index: 0 }).then_some(*r))
        .unwrap();
    app.handle(Input::PointerDown {
        x: source.x + 4,
        y: source.y + 2,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: destination.x + 4,
        y: destination.y + 2,
    });
    render(&mut app, &out, "drag-drop", 150, 44)?;
    app.handle(Input::CancelDrag);
    app.handle(Input::Pointer(None));
    render(&mut app, &out, "grid", 150, 44)?;
    let parent = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t,Target::PathGo {path,..} if Some(path.as_path())==demo.path().parent())
                .then_some(*r)
        })
        .unwrap();
    app.handle(Input::PointerDown {
        x: source.x + 4,
        y: source.y + 2,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: parent.x,
        y: parent.y,
    });
    render(&mut app, &out, "breadcrumb-drop", 150, 44)?;
    app.handle(Input::CancelDrag);
    app.handle(Input::Pointer(None));
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
    fs::write(
        demo.path().join("Operations/retained-notes.txt"),
        "Notes retained for redo.\n".repeat(4096),
    )?;
    history.start_operation(basalgrim::fs::Operation::Duplicate {
        sources: vec![demo.path().join("Operations/retained-notes.txt")],
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
    history.action(Action::Undo);
    settle(&mut history)?;
    render(&mut history, &out, "undo", 120, 36)?;
    render(&mut history, &out, "undo-compact", 34, 14)?;
    history.action(Action::HistoryStorage);
    settle(&mut history)?;
    render(&mut history, &out, "history-storage", 100, 28)?;
    render(&mut history, &out, "history-storage-compact", 34, 14)?;
    history.confirm_history_cleanup();
    render(&mut history, &out, "history-storage-confirm", 68, 18)?;
    history.cancel_dialog();
    history.cancel_dialog();
    fs::write(
        demo.path().join("landscape.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="1000" viewBox="0 0 800 1000">
      <defs><linearGradient id="sky" x2="0" y2="1"><stop stop-color="#101b26"/><stop offset="1" stop-color="#bc5b43"/></linearGradient></defs>
      <path fill="url(#sky)" d="M0 0h800v1000H0z"/>
      <circle cx="560" cy="300" r="115" fill="#ffb05c"/>
      <path fill="#854c48" d="m0 650 240-330 160 210 130-140 270 290v320H0z"/>
      <path fill="#ca8c69" d="m130 470 110-150 100 130-95-55-45 60-15-30z"/>
      <path fill="#35444c" d="m0 760 230-200 180 170 160-250 230 250v270H0z"/>
      <path fill="#172e35" d="m0 860 240-140 250 170 310-150v260H0z"/>
      <path fill="#0c2028" d="m80 900 100-330 100 330h-75v100h-50V900zm320 60 90-250 90 250h-65v40h-50v-40z"/>
    </svg>"##,
    )?;
    let mut image_app = App::new(demo.path().into(), Settings::default());
    image_app.icon_backend = basalgrim::icons::IconBackend::Gui;
    settle(&mut image_app)?;
    image_app.tab_mut().selected = image_app
        .tab()
        .visible
        .iter()
        .position(|&i| image_app.tab().entries[i].name == "landscape.svg")
        .unwrap();
    image_app.update_preview();
    settle(&mut image_app)?;
    render(&mut image_app, &out, "image-preview", 150, 44)?;
    let path = demo.path().join("Projects/Basalgrim/src/ui");
    fs::create_dir_all(&path)?;
    image_app.settings.preview = false;
    image_app.navigate(path, true);
    settle(&mut image_app)?;
    render(&mut image_app, &out, "paths", 120, 28)?;
    render(&mut image_app, &out, "paths-minimum", 34, 14)?;
    let more = image_app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::PathParents { .. }).then_some(*r))
        .unwrap();
    image_app.handle(Input::Click {
        x: more.x,
        y: more.y,
        double: false,
        toggle: false,
    });
    render(&mut image_app, &out, "path-parents", 50, 20)?;
    image_app.cancel_dialog();
    image_app.action(Action::Path);
    image_app.handle(Input::Key(basalgrim::input::KeyPress::plain(
        basalgrim::input::Key::End,
    )));
    render(&mut image_app, &out, "path-editor", 100, 28)?;
    image_app.cancel_dialog();
    image_app.navigate(demo.path().into(), true);
    settle(&mut image_app)?;
    let trash = basalgrim::trash::Root::home(demo.path().join("trash-demo"));
    use std::os::unix::fs::PermissionsExt;
    for dir in [
        trash.directory.clone(),
        trash.directory.join("files"),
        trash.directory.join("info"),
    ] {
        fs::create_dir_all(&dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    for (index, name) in [
        "Documents/report.pdf",
        "Projects/Basalgrim/notes.md",
        "Pictures/landscape.svg",
    ]
    .into_iter()
    .enumerate()
    {
        fs::write(
            trash
                .directory
                .join("files")
                .join(format!("stored-{index}")),
            "example content",
        )?;
        fs::write(
            trash
                .directory
                .join("info")
                .join(format!("stored-{index}.trashinfo")),
            format!(
                "[Trash Info]\nPath={}\nDeletionDate=2026-11-01T12:30:45\n",
                demo.path().join(name).display()
            ),
        )?;
    }
    image_app.open_trash(Some(vec![trash.clone()]));
    settle(&mut image_app)?;
    image_app.action(Action::SelectAll);
    render(&mut image_app, &out, "trash", 140, 32)?;
    render(&mut image_app, &out, "trash-compact", 34, 14)?;
    image_app.action(Action::RestoreTrash);
    render(&mut image_app, &out, "trash-confirm", 100, 28)?;
    image_app.cancel_dialog();

    // Show the current shared interface: batch rename, breadcrumbs and trash.
    let folder = demo.path().join("Pictures/Отпуск");
    fs::create_dir(&folder)?;
    for name in ["IMG_1234.png", "IMG_5678.jpg", "IMG_7890.png"] {
        fs::write(folder.join(name), "Screenshot fixture\n")?;
    }
    let mut overview = App::new(folder.clone(), Settings::default());
    overview.icon_backend = basalgrim::icons::IconBackend::Gui;
    overview.settings.dual = true;
    overview.settings.preview = false;
    overview.settings.grid = false;
    settle(&mut overview)?;
    overview.action(Action::SelectAll);
    overview.active = 1;
    overview.open_trash(Some(vec![trash]));
    settle(&mut overview)?;
    overview.action(Action::SelectAll);
    overview.active = 0;
    overview.action(Action::Rename);
    overview.handle(Input::Text("Фото".into()));
    settle(&mut overview)?;
    render(&mut overview, &out, "basalgrim-overview", 150, 38)?;
    overview.settings.appearance.theme = Theme::Light;
    render(&mut overview, &out, "batch-rename", 150, 38)?;
    render(&mut overview, &out, "batch-rename-compact", 34, 14)?;
    overview.handle(Input::Key(basalgrim::input::KeyPress::plain(
        basalgrim::input::Key::F(4),
    )));
    overview.handle(Input::Text("Photo_{n:03}".into()));
    settle(&mut overview)?;
    render(&mut overview, &out, "batch-template-preserve", 150, 38)?;
    render(
        &mut overview,
        &out,
        "batch-template-preserve-compact",
        34,
        14,
    )?;
    overview.handle(Input::Key(basalgrim::input::KeyPress::ctrl('a')));
    overview.handle(Input::Text("Снимок_{n:03}{jpg}".into()));
    settle(&mut overview)?;
    render(&mut overview, &out, "batch-template-error", 150, 38)?;
    render(&mut overview, &out, "batch-template-error-compact", 34, 14)?;
    overview.handle(Input::Key(basalgrim::input::KeyPress::plain(
        basalgrim::input::Key::PageDown,
    )));
    render(&mut overview, &out, "batch-template-hint-compact", 34, 14)?;
    overview.handle(Input::Key(basalgrim::input::KeyPress::ctrl('a')));
    overview.handle(Input::Text("Photo_{n:03}{ext:jpg}".into()));
    settle(&mut overview)?;
    render(&mut overview, &out, "batch-template", 150, 38)?;
    render(&mut overview, &out, "batch-template-compact", 34, 14)?;
    let mut finder = App::new(demo.path().to_path_buf(), Settings::default());
    finder.icon_backend = basalgrim::icons::IconBackend::Gui;
    settle(&mut finder)?;
    finder.begin_recursive_search();
    finder.handle(Input::Text("notes".into()));
    settle(&mut finder)?;
    render(&mut finder, &out, "basalgrim-overview", 150, 38)?;
    finder.settings.appearance.theme = Theme::Light;
    render(&mut finder, &out, "recursive-search", 150, 38)?;
    render(&mut finder, &out, "recursive-search-compact", 34, 14)?;
    let media_dir = demo.path().join("Media");
    fs::create_dir(&media_dir)?;
    fs::copy(
        demo.path().join("landscape.svg"),
        media_dir.join("Горы.svg"),
    )?;
    image::RgbaImage::from_fn(320, 180, |x, y| {
        image::Rgba([(40 + x / 2) as u8, (50 + y) as u8, 170, 255])
    })
    .save(media_dir.join("Палитра.png"))?;
    let movie = media_dir.join("Демо.mp4");
    if let Ok(status) = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=1:duration=2",
            "-c:v",
            "mpeg4",
            "-threads",
            "1",
        ])
        .arg(&movie)
        .status()
    {
        ensure!(status.success(), "Could not create video preview fixture");
    }
    let mut media = App::new(media_dir, Settings::default());
    media.icon_backend = basalgrim::icons::IconBackend::Gui;
    media.settings.appearance.theme = Theme::Light;
    settle(&mut media)?;
    if let Some(index) = media
        .tab()
        .visible
        .iter()
        .position(|&i| media.tab().entries[i].path == movie)
    {
        media.tab_mut().selected = index;
    }
    media.update_preview();
    settle(&mut media)?;
    let ready = Instant::now() + Duration::from_secs(10);
    loop {
        settle(&mut media)?;
        render(&mut media, &out, "media-warmup", 150, 38)?;
        if !media.loading() {
            break;
        }
        ensure!(Instant::now() < ready, "Media scene did not settle");
    }
    ensure!(
        media
            .icon_slots
            .iter()
            .filter(|s| s.source.is_some())
            .all(|s| s.content.is_some()),
        "Media screenshot still has fallback icons"
    );
    // README always uses the project's classic palette, independent of the
    // user's selected theme and feature-specific documentation illustrations.
    media.settings.appearance.theme = Theme::MoltenZharr;
    render(&mut media, &out, "basalgrim-overview", 150, 38)?;
    media.settings.appearance.theme = Theme::Light;
    render(&mut media, &out, "media-grid", 150, 38)?;
    media.settings.grid = false;
    render(&mut media, &out, "media-list", 150, 38)?;
    render(&mut media, &out, "media-compact", 34, 14)?;
    media.action(Action::Settings);
    media.settings_tab(SettingsTab::Icons);
    media.settings_view.selected = 1;
    render(&mut media, &out, "media-settings-compact", 34, 14)?;
    // Feature illustrations belong in usage documentation; keep the README's
    // general overview independent of the latest roadmap feature.
    let code_path = media.tab().cwd.join("Пример.rs");
    fs::write(
        &code_path,
        "// Привет из Basalgrim\nfn main() {\n    let name = \"Мир\";\n    let count = 42;\n\n    for i in 0..count {\n        println!(\"{name}: {i}\");\n    }\n}\n",
    )?;
    media.dialog = None;
    media.settings.grid = true;
    media.action(Action::Refresh);
    settle(&mut media)?;
    media.tab_mut().selected = media
        .tab()
        .visible
        .iter()
        .position(|&i| media.tab().entries[i].path == code_path)
        .unwrap();
    media.update_preview();
    settle(&mut media)?;
    let ready = Instant::now() + Duration::from_secs(10);
    loop {
        render(&mut media, &out, "syntax-warmup", 150, 38)?;
        if !media.loading() {
            break;
        }
        ensure!(Instant::now() < ready, "Code scene did not settle");
        settle(&mut media)?;
    }
    ensure!(
        media
            .icon_slots
            .iter()
            .filter(|s| s.source.is_some())
            .all(|s| s.content.is_some()),
        "Code screenshot still has fallback icons"
    );
    render(&mut media, &out, "syntax-light", 150, 38)?;
    media.settings.grid = false;
    render(&mut media, &out, "syntax-list", 150, 38)?;
    media.settings.appearance.theme = Theme::MoltenZharr;
    render(&mut media, &out, "syntax-dark", 150, 38)?;
    println!("Rendered GUI/TUI previews to {}", out.display());
    Ok(())
}

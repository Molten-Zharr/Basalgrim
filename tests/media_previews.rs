#![cfg(any(feature = "guitui", feature = "tui"))]
use basalgrim::{
    app::{App, SettingsSection, SettingsTab},
    fs,
    icons::{IconBackend, IconKind, IconSlot},
    operations::Cancellation,
    preferences::IconMode,
    settings::Settings,
    thumbnails::{Source, Thumbnails},
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn png(path: &Path, color: [u8; 4]) {
    image::RgbaImage::from_pixel(320, 180, image::Rgba(color))
        .save(path)
        .unwrap();
}
fn video(path: &Path) -> bool {
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return false;
    }
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=0x20d040:s=320x180:d=1",
                "-c:v",
                "mpeg4",
                "-threads",
                "1"
            ])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    true
}
fn settle(app: &mut App, term: &mut Terminal<TestBackend>) {
    let end = Instant::now() + Duration::from_secs(12);
    loop {
        app.tick();
        term.draw(|f| ui::render(app, f)).unwrap();
        if !app.loading() {
            break;
        }
        assert!(Instant::now() < end, "media timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn video_frame_metadata_no_graphics_bad_files_and_text_typescript() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("clip.mp4");
    if !video(&path) {
        eprintln!("FFmpeg unavailable: real video fixture skipped");
        return;
    }
    let preview = fs::preview(&path).unwrap();
    assert!(
        preview
            .details
            .iter()
            .any(|d| d.contains("Видео · 320 × 180")),
        "{:?}",
        preview.lines
    );
    let frame = preview
        .image
        .unwrap_or_else(|| panic!("{:?}", preview.lines));
    assert!(frame.width <= 768 && frame.height <= 768);
    assert!((frame.width as f64 / frame.height as f64 - 16.0 / 9.0).abs() < 0.02);
    assert!(
        frame
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[1] > 170 && p[0] < 90 && p[2] < 120)
    );
    let preview = fs::preview_with_images(&path, false).unwrap();
    assert!(preview.image.is_none());
    assert!(preview.lines.join(" ").contains("не поддерживает графику"));
    std::fs::copy(&path, root.path().join("without-extension")).unwrap();
    assert!(
        fs::preview(&root.path().join("without-extension"))
            .unwrap()
            .image
            .is_some()
    );
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        fs::preview_until(&path, true, &cancel)
            .unwrap()
            .image
            .is_none()
    );
    std::fs::write(root.path().join("bad.mp4"), b"not a video").unwrap();
    assert!(
        fs::preview(&root.path().join("bad.mp4"))
            .unwrap()
            .lines
            .join(" ")
            .contains("недоступен")
    );
    std::fs::write(root.path().join("code.ts"), b"const example = 1;\n").unwrap();
    assert!(
        fs::preview(&root.path().join("code.ts"))
            .unwrap()
            .lines
            .join(" ")
            .contains("const example")
    );
    // A disguised playlist cannot open other paths or protocols.
    std::fs::write(
        root.path().join("playlist.mp4"),
        b"#EXTM3U\nhttp://127.0.0.1:9/movie.mp4\n",
    )
    .unwrap();
    assert!(
        fs::preview(&root.path().join("playlist.mp4"))
            .unwrap()
            .image
            .is_none()
    );
}

#[test]
fn thumbnails_grid_list_modals_settings_and_refresh_preserve_focus() {
    let root = tempfile::tempdir().unwrap();
    png(&root.path().join("a.png"), [220, 30, 40, 255]);
    png(&root.path().join("b.png"), [20, 200, 60, 255]);
    for grid in [true, false] {
        let mut app = App::new(
            root.path().to_path_buf(),
            Settings {
                grid,
                preview: false,
                ..Default::default()
            },
        );
        app.icon_backend = IconBackend::Gui;
        let mut term = Terminal::new(TestBackend::new(150, 40)).unwrap();
        settle(&mut app, &mut term);
        assert_eq!(
            app.icon_slots
                .iter()
                .filter(|s| s.content.is_some())
                .count(),
            2
        );
        let selected = app.tab().entry().unwrap().path.clone();
        let first = app
            .icon_slots
            .iter()
            .filter_map(|s| s.content.as_ref())
            .next()
            .unwrap()
            .id();
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        assert_eq!(
            app.icon_slots
                .iter()
                .filter_map(|s| s.content.as_ref())
                .next()
                .unwrap()
                .id(),
            first
        );
        assert_eq!(app.tab().entry().unwrap().path, selected);
        app.action(basalgrim::input::Action::Search);
        term.draw(|f| ui::render(&mut app, f)).unwrap();
        for slot in &app.icon_slots {
            assert!(
                app.hits
                    .iter()
                    .filter(|(_, t)| matches!(t, basalgrim::app::Target::MenuBackground))
                    .all(|(area, _)| area.intersection(slot.area).is_empty())
            );
        }
        app.cancel_dialog();
        app.settings.appearance.thumbnails = false;
        settle(&mut app, &mut term);
        assert!(app.icon_slots.iter().all(|s| s.content.is_none()));
        assert_eq!(app.thumbnails.cached_frames(), 0);
        app.settings.appearance.thumbnails = true;
        app.settings.appearance.icon_mode = IconMode::Symbols;
        settle(&mut app, &mut term);
        assert!(app.icon_slots.is_empty());
        assert_eq!(app.thumbnails.cached_frames(), 0);
        app.settings.appearance.icon_mode = IconMode::Images;
        app.icon_backend = IconBackend::Symbols;
        settle(&mut app, &mut term);
        assert!(app.icon_slots.is_empty());
        app.icon_backend = IconBackend::Gui;
        png(&root.path().join("a.png"), [40, 60, 230, 255]);
        app.action(basalgrim::input::Action::Refresh);
        settle(&mut app, &mut term);
        assert!(
            app.icon_slots
                .iter()
                .filter_map(|s| s.content.as_ref())
                .any(|im| im
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[2] > 200 && p[0] < 80))
        );
        app.action(basalgrim::input::Action::Settings);
        app.settings_section(SettingsSection::Themes);
        app.settings_tab(SettingsTab::Icons);
        let row = app
            .settings_rows()
            .iter()
            .position(|r| r.label == "Миниатюры файлов")
            .unwrap();
        app.setting(row, true, 1);
        assert!(!app.settings.appearance.thumbnails);
        let json = serde_json::to_string(&app.settings).unwrap();
        let stored: Settings = serde_json::from_str(&json).unwrap();
        assert!(!stored.appearance.thumbnails);
    }
}

#[test]
fn cache_is_bounded_and_old_directory_requests_cannot_reappear() {
    let root = tempfile::tempdir().unwrap();
    for i in 0..140 {
        png(
            &root.path().join(format!("image-{i:03}.png")),
            [i as u8, 30, 200, 255],
        );
    }
    let (_, entries) = (root.path(), fs::read_directory(root.path()).unwrap());
    let slot = |entry: &fs::Entry, request| IconSlot {
        source: Source::new(entry, request),
        content: None,
        kind: IconKind::Image,
        area: Rect::new(0, 0, 6, 2),
        foreground: [0; 3],
        accent: [0; 3],
        background: [0; 3],
    };
    let mut thumbs = Thumbnails::default();
    let mut slots: Vec<_> = entries.iter().map(|e| slot(e, 1)).collect();
    thumbs.update(&mut slots, true);
    // Replace the entire pending request with another directory generation.
    let mut latest = vec![slot(&entries[139], 2)];
    thumbs.update(&mut latest, true);
    let end = Instant::now() + Duration::from_secs(10);
    while thumbs.loading() {
        thumbs.update(&mut latest, true);
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    thumbs.update(&mut latest, true);
    assert!(latest[0].content.is_some());
    let image = latest[0].content.as_ref().unwrap();
    assert_eq!((image.width, image.height), (160, 90));
    assert_eq!(thumbs.cached_frames(), 1);
    thumbs.update(&mut slots, true);
    while thumbs.loading() {
        thumbs.update(&mut slots, true);
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    thumbs.update(&mut slots, true);
    assert_eq!(
        slots.iter().filter(|s| s.content.is_some()).count(),
        basalgrim::thumbnails::CAPACITY
    );
    assert!(thumbs.cached_frames() <= basalgrim::thumbnails::CAPACITY);
    assert!(
        slots
            .iter()
            .filter_map(|s| s.content.as_ref())
            .all(|im| im.width <= 160 && im.height <= 160)
    );
    thumbs.update(&mut [], false);
    assert!(!thumbs.loading());
    assert_eq!(thumbs.cached_frames(), 0);
}

#[test]
fn thumbnail_setting_is_mouse_accessible_at_34_by_14() {
    use basalgrim::{
        app::Target,
        input::{Action, Input},
    };
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    let mut terminal = Terminal::new(TestBackend::new(34, 14)).unwrap();
    settle(&mut app, &mut terminal);
    app.action(Action::Settings);
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let &(rect, _) = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t, Target::SettingsTab(SettingsTab::Icons)))
        .unwrap();
    app.handle(Input::Click {
        x: rect.x,
        y: rect.y,
        double: false,
        toggle: false,
    });
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let area = app.settings_view.parameters_area;
    app.handle(Input::Scroll {
        x: area.x,
        y: area.y,
        delta: 1,
    });
    terminal.draw(|f| ui::render(&mut app, f)).unwrap();
    let &(rect, _) = app
        .hits
        .iter()
        .find(|(_, t)| matches!(t, Target::Setting { index: 1, .. }))
        .unwrap();
    app.handle(Input::Click {
        x: rect.x,
        y: rect.y,
        double: false,
        toggle: false,
    });
    assert!(!app.settings.appearance.thumbnails);
}

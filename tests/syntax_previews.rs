use basalgrim::{
    app::App,
    fs,
    input::{Action, Input},
    operations::Cancellation,
    preferences::{IconMode, Theme},
    preview::syntax::Kind,
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, style::Color};
use std::{
    fs as stdfs,
    path::Path,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < deadline, "preview worker stalled");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn tagged(preview: &fs::Preview, kind: Kind) -> String {
    preview
        .code
        .as_ref()
        .unwrap()
        .lines
        .iter()
        .enumerate()
        .flat_map(|(i, tokens)| {
            tokens
                .iter()
                .filter(move |t| t.kind == kind)
                .map(move |t| &preview.lines[i][t.range.clone()])
        })
        .collect()
}

#[test]
fn common_languages_names_and_shebangs_are_detected_without_graphics() {
    let root = tempfile::tempdir().unwrap();
    for (name, source, language) in [
        (
            "main.rs",
            "fn main() { let n = 42; println!(\"Привет\"); }",
            "Rust",
        ),
        ("app.py", "def greet():\n    return \"Привет\"", "Python"),
        (
            "app.ts",
            "const n: number = 42;\nconst greeting = 'Hello';",
            "TypeScript",
        ),
        ("app.js", "const greeting = 'Hello';", "JavaScript"),
        ("Cargo.toml", "[package]\nname = \"demo\"", "TOML"),
        ("settings.json", "{\"hello\": 42}", "JSON"),
        ("config.yaml", "hello: 'world'", "YAML"),
        ("Dockerfile", "FROM alpine\nRUN echo hello", "Dockerfile"),
        ("Makefile", "all:\n\techo hello", "Makefile"),
        ("script", "#!/usr/bin/env python3\nprint('hello')", "Python"),
        ("run", "#!/bin/bash\necho \"hello\"", "bash"),
        ("UPPER.RS", "fn main() {}", "Rust"),
    ] {
        let path = root.path().join(name);
        stdfs::write(&path, source).unwrap();
        let preview = fs::preview_with_images(&path, false).unwrap();
        let code = preview
            .code
            .as_ref()
            .unwrap_or_else(|| panic!("Missing {name}"));
        assert!(
            code.language.contains(language),
            "{name}: {}",
            code.language
        );
        assert!(!code.limited, "{name}: unexpected limit");
        assert!(
            code.lines.iter().flatten().any(|t| t.kind != Kind::Plain),
            "{name}: no tokens"
        );
        for (i, tokens) in code.lines.iter().enumerate() {
            let reconstructed = tokens
                .iter()
                .map(|t| &preview.lines[i][t.range.clone()])
                .collect::<String>();
            assert_eq!(reconstructed, &preview.lines[i][5..], "{name}");
        }
    }
}

#[test]
fn multiline_state_unicode_tabs_and_untrusted_controls_keep_display_text_intact() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.rs");
    stdfs::write(&path, "/* multi\nfn not_code() {}\n*/\nfn main() {\n\tlet word = \"Привет 😀\"; // comment\n\tlet raw = r#\"line one\nline two\"#;\n}\n// \x1b[31m").unwrap();
    let preview = fs::preview(&path).unwrap();
    assert!(tagged(&preview, Kind::Comment).contains("fn not_code"));
    assert!(tagged(&preview, Kind::String).contains("Привет 😀"));
    assert!(tagged(&preview, Kind::String).contains("line two"));
    assert!(tagged(&preview, Kind::Keyword).contains("fn"));
    assert!(!preview.lines.join("\n").contains('\x1b'));
    assert!(preview.lines[4].starts_with("  5      let"));
    for (i, tokens) in preview.code.as_ref().unwrap().lines.iter().enumerate() {
        assert_eq!(
            tokens
                .iter()
                .map(|t| &preview.lines[i][t.range.clone()])
                .collect::<String>(),
            preview.lines[i][5..]
        );
    }
}

#[test]
fn plain_binary_and_non_regular_files_do_not_get_code_styles() {
    let root = tempfile::tempdir().unwrap();
    for (name, source) in [
        ("notes.txt", "fn main() {}"),
        ("README", "ordinary text"),
        ("bad.rs", "binary\0file"),
    ] {
        let path = root.path().join(name);
        stdfs::write(&path, source).unwrap();
        assert!(fs::preview(&path).unwrap().code.is_none());
    }
    std::os::unix::fs::symlink(root.path().join("notes.txt"), root.path().join("link.rs")).unwrap();
    assert!(
        fs::preview(&root.path().join("link.rs"))
            .unwrap()
            .code
            .is_none()
    );
    assert!(fs::preview(root.path()).unwrap().code.is_none());
}

#[test]
fn parsing_stops_at_long_lines_and_cancellation_but_preserves_plain_content() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("long.rs");
    let source = format!(
        "fn main() {{}}\n/* {}\nstill in comment */\nfn after() {{}}",
        "x".repeat(3000)
    );
    stdfs::write(&path, source).unwrap();
    let preview = fs::preview(&path).unwrap();
    let code = preview.code.as_ref().unwrap();
    assert!(code.limited);
    assert_eq!(code.lines.len(), 1);
    assert!(preview.lines.last().unwrap().ends_with("fn after() {}"));
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        fs::preview_until(&path, false, &cancel)
            .unwrap()
            .code
            .is_none()
    );
}

#[test]
fn byte_and_line_limits_preserve_utf8_and_highlight_only_the_visible_prefix() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.rs");
    stdfs::write(&path, format!("{}😀", " ".repeat(65_535))).unwrap();
    let preview = fs::preview(&path).unwrap();
    assert!(!preview.lines[0].contains("Бинарный"));
    assert!(preview.lines.last().unwrap().contains("64 KiB"));
    stdfs::write(&path, "// комментарий\n".repeat(400)).unwrap();
    let preview = fs::preview(&path).unwrap();
    assert_eq!(preview.lines.len(), 301);
    assert_eq!(preview.code.unwrap().lines.len(), 300);
}

#[test]
fn rendered_colors_follow_all_palettes_and_overrides_without_reloading_or_changing_focus() {
    let root = tempfile::tempdir().unwrap();
    stdfs::write(
        root.path().join("main.rs"),
        "fn main() {\n    let greeting = \"Hello\";\n    // comment\n}\n".repeat(30),
    )
    .unwrap();
    let mut settings = Settings::default();
    settings.appearance.icon_mode = IconMode::Symbols;
    let mut app = App::new(root.path().into(), settings);
    settle(&mut app);
    let selected = app.tab().selected;
    for theme in Theme::ALL {
        app.settings.appearance.theme = theme;
        let buffer = render(&mut app, 150, 40);
        let palette = app.settings.appearance.palette();
        let area = app.preview_area;
        let [r, g, b] = palette["help_heading"];
        assert!(
            (area.y..area.bottom())
                .any(|y| (area.x..area.right())
                    .any(|x| buffer[(x, y)].symbol() == "H"
                        && buffer[(x, y)].fg == Color::Rgb(r, g, b))),
            "{theme:?}: string color"
        );
        assert!(!app.preview_loading);
    }
    app.settings
        .appearance
        .colors
        .entry(Theme::Light)
        .or_default()
        .insert("help_heading".into(), [1, 110, 120]);
    let buffer = render(&mut app, 150, 40);
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.symbol() == "H" && c.fg == Color::Rgb(1, 110, 120))
    );
    let area = app.preview_area;
    app.handle(Input::Scroll {
        x: area.x + 1,
        y: area.y + 12,
        delta: 3,
    });
    render(&mut app, 150, 40);
    assert_eq!(app.preview_offset, 9);
    assert_eq!(app.tab().selected, selected);
    render(&mut app, 34, 14);
    assert!(app.preview_area.is_empty());
    render(&mut app, 150, 40);
    assert_eq!(app.preview_offset, 9);
    app.action(Action::Preview);
    render(&mut app, 150, 40);
    assert!(app.preview.is_none());
}

#[test]
fn obsolete_previews_cannot_color_a_new_plain_file() {
    let root = tempfile::tempdir().unwrap();
    stdfs::write(root.path().join("a.rs"), "fn main() {}\n".repeat(300)).unwrap();
    stdfs::write(root.path().join("b.txt"), "plain final selection").unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    for index in [0, 1, 0, 1] {
        app.tab_mut().selected = index;
        app.update_preview();
    }
    settle(&mut app);
    let preview = app.preview.as_ref().unwrap();
    assert_eq!(preview.title, "b.txt");
    assert!(preview.code.is_none());
    assert!(preview.lines[0].contains("plain final"));
    assert_eq!(
        app.tab().entry().unwrap().path.file_name(),
        Path::new("b.txt").file_name()
    );
}

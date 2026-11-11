use basalgrim::{
    app::{App, Clipboard, Dialog, EditKind, PaletteControl as C, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{
    fs,
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn setup() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("sample.txt"), "kept").unwrap();
    let mut app = App::new(
        root.path().into(),
        Settings {
            grid: false,
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    (root, app)
}
fn render(app: &mut App) -> String {
    let mut t = Terminal::new(TestBackend::new(34, 14)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    assert!(
        app.hits
            .iter()
            .all(|(r, _)| r.right() <= 34 && r.bottom() <= 14)
    );
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| predicate(t).then_some(*r))
        .unwrap()
}
fn click(app: &mut App, r: Rect, double: bool) {
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
fn query(app: &mut App, text: &str) {
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Text(text.into()));
    render(app);
}
fn open(app: &mut App, text: &str) {
    app.action(Action::Palette);
    query(app, text);
}
fn run(app: &mut App) {
    render(app);
    let r = hit(app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Run,
                ..
            }
        )
    });
    click(app, r, false);
}
#[test]
fn mouse_palette_searches_names_aliases_and_bindings_and_uses_normal_dialogs() {
    let (root, mut app) = setup();
    app.action(Action::Commands);
    render(&mut app);
    let index = app
        .command_items()
        .iter()
        .position(|r| r.action == Action::Palette)
        .unwrap();
    let r = hit(&app, |t| matches!(t,Target::Command(i) if *i==index));
    click(&mut app, r, false);
    assert!(app.palette_dialog());
    query(&mut app, "НОВАЯ папка");
    assert_eq!(
        app.palette_view.as_ref().unwrap().rows[0].action,
        Action::NewFolder
    );
    query(&mut app, "new folder");
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Row(0),
                ..
            }
        )
    });
    click(&mut app, r, false);
    assert!(app.palette_dialog());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    click(&mut app, r, true);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Folder,
            ..
        })
    ));
    app.handle(Input::Text("Новая папка".into()));
    app.handle(Input::Key(KeyPress::plain(Key::Enter)));
    settle(&mut app);
    assert!(root.path().join("Новая папка").is_dir());
    open(&mut app, "F2");
    assert_eq!(
        app.palette_view.as_ref().unwrap().rows[0].action,
        Action::Rename
    );
    run(&mut app);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Edit {
            kind: EditKind::Rename(_),
            ..
        })
    ));
    app.cancel_dialog();
    app.settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Rename)
        .unwrap()
        .key = KeyPress {
        key: Key::F(11),
        ctrl: true,
        alt: true,
        shift: true,
    };
    open(&mut app, "Ctrl+Alt+Shift+F11");
    assert_eq!(
        app.palette_view.as_ref().unwrap().rows[0].action,
        Action::Rename
    );
    query(&mut app, "ничего_такого_нет");
    assert!(render(&mut app).contains("Ничего не найдено"));
    run(&mut app);
    assert!(app.palette_dialog());
    let clear = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Clear,
                ..
            }
        )
    });
    click(&mut app, clear, false);
    assert!(app.palette_view.as_ref().unwrap().rows.len() > 40);
}
#[test]
fn unavailable_commands_explain_why_and_hover_and_text_clipboard_cannot_mutate_files() {
    let (root, mut app) = setup();
    app.journal_enabled = true;
    open(&mut app, "redo");
    assert_eq!(
        app.palette_view.as_ref().unwrap().rows[0].action,
        Action::Redo
    );
    run(&mut app);
    assert!(
        app.palette_dialog(),
        "a disabled exact alias must not execute a different enabled match"
    );
    app.cancel_dialog();
    open(&mut app, "paste");
    assert!(render(&mut app).contains("Буфер файлов пуст"));
    let selected = app.tab().selected;
    let row = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Row(0),
                ..
            }
        )
    });
    app.handle(Input::Pointer(Some((row.x, row.y).into())));
    render(&mut app);
    assert_eq!(app.hovered_area(), Some(row));
    let background = hit(&app, |t| matches!(t, Target::MenuBackground));
    app.handle(Input::Pointer(Some((background.x, background.y).into())));
    render(&mut app);
    assert_eq!(app.hovered_area(), None);
    click(&mut app, row, true);
    run(&mut app);
    assert!(app.palette_dialog());
    assert!(!app.busy);
    assert_eq!(app.tab().selected, selected);
    app.clipboard = Some(Clipboard {
        paths: vec![root.path().join("sample.txt")],
        cut: false,
    });
    query(&mut app, "rename");
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Key(KeyPress::ctrl('c')));
    assert_eq!(app.take_text_copy().unwrap().text, "rename");
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        vec![root.path().join("sample.txt")]
    );
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(app.dialog.is_none());
    assert_eq!(
        fs::read_to_string(root.path().join("sample.txt")).unwrap(),
        "kept"
    );
}
#[test]
fn stale_filter_dialog_selection_request_and_busy_hits_never_execute_a_replacement_command() {
    let (_root, mut app) = setup();
    open(&mut app, "new folder");
    let old = app.hits.clone();
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Row(0),
                ..
            }
        )
    });
    app.handle(Input::Key(KeyPress::ctrl('a')));
    app.handle(Input::Text("quit".into()));
    click(&mut app, r, true);
    assert!(!app.quit);
    assert!(app.palette_dialog());
    app.cancel_dialog();
    open(&mut app, "new folder");
    app.hits = old;
    click(&mut app, r, true);
    assert!(app.palette_dialog());
    render(&mut app);
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Run,
                ..
            }
        )
    });
    app.busy = true;
    click(&mut app, r, false);
    assert!(app.palette_dialog());
    assert!(
        app.palette_view.as_ref().unwrap().rows[0]
            .reason
            .unwrap()
            .contains("операции")
    );
    app.busy = false;
    app.tab_mut().explicit_selection = true;
    app.tab_mut().marked.clear();
    click(&mut app, r, false);
    assert!(!app.palette_dialog());
    assert!(app.dialog.is_none());
    open(&mut app, "new folder");
    app.tab_mut().request += 1;
    app.tick();
    assert!(app.dialog.is_none());
}
#[test]
fn palette_preserves_trash_confirmation_and_readonly_archive_guards() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut app) = setup();
    let trash = basalgrim::trash::Root::home(root.path().join("trash-data"));
    for d in [
        &trash.directory,
        &trash.directory.join("files"),
        &trash.directory.join("info"),
    ] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let data = trash.directory.join("files/one");
    fs::write(&data, "retained").unwrap();
    fs::write(
        trash.directory.join("info/one.trashinfo"),
        format!(
            "[Trash Info]\nPath={}/gone.txt\nDeletionDate=2026-11-11T10:00:00\n",
            root.path().display()
        ),
    )
    .unwrap();
    app.open_trash(Some(vec![trash]));
    settle(&mut app);
    open(&mut app, "new file");
    assert!(app.palette_view.as_ref().unwrap().rows[0].reason.is_some());
    run(&mut app);
    assert!(app.palette_dialog());
    app.cancel_dialog();
    open(&mut app, "delete trash");
    let view = app.palette_view.as_mut().unwrap();
    view.selected = view
        .rows
        .iter()
        .position(|r| r.action == Action::DeleteTrash)
        .unwrap();
    run(&mut app);
    assert!(matches!(app.dialog, Some(Dialog::DeleteTrash { .. })));
    assert_eq!(fs::read_to_string(&data).unwrap(), "retained");
    app.cancel_dialog();
    app.action(Action::Parent);
    settle(&mut app);
    let archive = root.path().join("sample.tar");
    let report = basalgrim::fs::operate(basalgrim::fs::Operation::ArchiveCreate {
        sources: vec![root.path().join("sample.txt")],
        destination: archive.clone(),
        format: basalgrim::archive::Format::Tar,
    });
    assert!(report.error.is_none());
    app.navigate(archive, true);
    settle(&mut app);
    assert!(app.in_archive());
    open(&mut app, "new file");
    assert!(
        app.palette_view.as_ref().unwrap().rows[0]
            .reason
            .unwrap()
            .contains("чтения")
    );
    run(&mut app);
    assert!(app.palette_dialog());
}
#[test]
fn new_palette_binding_migrates_without_overwriting_custom_shortcuts() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.bindings.retain(|b| b.action != Action::Palette);
    let wanted = KeyPress {
        shift: true,
        ..KeyPress::ctrl('p')
    };
    settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Help)
        .unwrap()
        .key = wanted;
    settings.normalize().unwrap();
    assert_eq!(settings.action_for(wanted), Some(Action::Help));
    let path = root.path().join("settings.json");
    settings.save(&path).unwrap();
    let loaded = Settings::load(&path).unwrap();
    assert_eq!(
        loaded.shortcut(Action::Palette),
        settings.shortcut(Action::Palette)
    );
    assert_ne!(loaded.shortcut(Action::Palette), wanted.label());
}

#[test]
fn compact_palettes_keep_close_clear_scroll_and_stale_opposite_pane_controls_safe() {
    use basalgrim::preferences::{BorderWeight, Theme};
    let (_root, mut app) = setup();
    for theme in [Theme::MoltenZharr, Theme::Light] {
        for borders in BorderWeight::ALL {
            app.settings.appearance.theme = theme;
            app.settings.appearance.borders = borders;
            open(&mut app, "");
            for c in [C::Run, C::Clear, C::Move(1)] {
                assert!(app.hits.iter().any(|(_,t)|matches!(t,Target::Palette{control,..} if std::mem::discriminant(control)==std::mem::discriminant(&c))));
            }
            let old = app.hits.clone();
            let row = hit(&app, |t| {
                matches!(
                    t,
                    Target::Palette {
                        control: C::Row(0),
                        ..
                    }
                )
            });
            app.handle(Input::Key(KeyPress::plain(Key::PageDown)));
            render(&mut app);
            app.hits = old;
            click(&mut app, row, true);
            assert!(app.palette_dialog());
            app.cancel_dialog();
        }
    }
    app.settings.dual = true;
    open(&mut app, "move panel");
    let r = hit(&app, |t| {
        matches!(
            t,
            Target::Palette {
                control: C::Run,
                ..
            }
        )
    });
    app.panes[1].tab_mut().request += 1;
    click(&mut app, r, false);
    assert!(app.dialog.is_none());
    assert!(!app.busy);
    assert!(app.notice.contains("Вторая панель изменилась"));
    open(&mut app, "x");
    query(&mut app, &"я".repeat(257));
    assert!(app.palette_view.as_ref().unwrap().rows.is_empty());
    assert!(render(&mut app).contains("256"));
}

#[cfg(feature = "guitui")]
#[test]
fn palette_pixels_survive_returning_from_actions_and_query_changes() {
    let (_root, mut app) = setup();
    let backend = basalgrim::raster::backend(16, 170, 53);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.backend_mut().cursor_config.blink = false;
    let draw =
        |app: &mut App,
         t: &mut Terminal<soft_ratatui::SoftBackend<basalgrim::raster::CellFont>>| {
            t.draw(|f| ui::render(app, f)).unwrap();
            let pixels = t.backend().get_pixmap_data().to_vec();
            t.backend_mut().redraw();
            assert!(
                pixels == t.backend().get_pixmap_data(),
                "incremental raster lost glyphs"
            );
        };
    draw(&mut app, &mut terminal);
    app.action(Action::Palette);
    draw(&mut app, &mut terminal);
    app.handle(Input::Text("new folder".into()));
    draw(&mut app, &mut terminal);
    app.cancel_dialog();
    draw(&mut app, &mut terminal);
    app.action(Action::Commands);
    draw(&mut app, &mut terminal);
    let index = app
        .command_items()
        .iter()
        .position(|r| r.action == Action::Palette)
        .unwrap();
    let r = hit(&app, |t| matches!(t,Target::Command(i) if *i==index));
    click(&mut app, r, false);
    draw(&mut app, &mut terminal);
    for c in "redo".chars() {
        app.handle(Input::Text(c.to_string()));
        draw(&mut app, &mut terminal);
    }
    for _ in 0..12 {
        app.tick();
        draw(&mut app, &mut terminal);
    }
}

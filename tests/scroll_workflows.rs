use basalgrim::{
    app::{App, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use std::{
    fs,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < end);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup(grid: bool) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    for i in 0..100 {
        fs::write(
            root.path().join(format!("file-{i:03}.txt")),
            format!("File {i}"),
        )
        .unwrap();
    }
    let mut app = App::new(
        root.path().into(),
        Settings {
            grid,
            preview: false,
            ..Settings::default()
        },
    );
    settle(&mut app);
    render(&mut app, 100, 32);
    (root, app)
}
fn render(app: &mut App, w: u16, h: u16) {
    Terminal::new(TestBackend::new(w, h))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn wheel(app: &mut App, p: Position, delta: i32) {
    app.handle(Input::Scroll {
        x: p.x,
        y: p.y,
        delta,
    });
}
fn first(app: &App, pane: usize) -> usize {
    app.hits
        .iter()
        .find_map(|(_, t)| {
            if let Target::Entry { pane: p, index } = t {
                (*p == pane).then_some(*index)
            } else {
                None
            }
        })
        .unwrap()
}
fn point(app: &App, pane: usize) -> Position {
    let r = app.file_areas[pane];
    (r.x + 1, r.y + 1).into()
}

#[test]
fn wheel_scrolls_visible_files_without_changing_any_selection_or_snapping_back() {
    for grid in [true, false] {
        let (root, mut app) = setup(grid);
        app.tab_mut().selected = 2;
        app.tab_mut().marked.extend([
            root.path().join("file-002.txt"),
            root.path().join("file-099.txt"),
        ]);
        app.tab_mut().explicit_selection = true;
        render(&mut app, 100, 32);
        let selected = app.tab().selected;
        let marked = app.tab().marked.clone();
        let targets = app.tab().targets();
        let p = point(&app, 0);
        wheel(&mut app, p, 1);
        render(&mut app, 100, 32);
        assert!(app.tab().offset > 0, "Wheel did not move the viewport");
        assert_eq!(first(&app, 0), app.tab().offset);
        assert_eq!(app.tab().selected, selected);
        assert_eq!(app.tab().marked, marked);
        assert_eq!(app.tab().targets(), targets);
        assert!(app.tab().explicit_selection);
        let offset = app.tab().offset;
        render(&mut app, 100, 32);
        assert_eq!(app.tab().offset, offset, "Idle render snapped to focus");
        wheel(&mut app, p, 20);
        render(&mut app, 100, 32);
        assert!(first(&app, 0) > selected);
        app.action(Action::Copy);
        assert_eq!(app.clipboard.as_ref().unwrap().paths, targets);
        render(&mut app, 100, 32);
        assert!(first(&app, 0) > selected);
        wheel(&mut app, p, i32::MIN);
        render(&mut app, 100, 32);
        assert_eq!(app.tab().offset, 0);
        assert_eq!(app.tab().selected, selected);
    }
}

#[test]
fn keyboard_navigation_reveals_focus_after_manual_scrolling() {
    for grid in [true, false] {
        let (_root, mut app) = setup(grid);
        let p = point(&app, 0);
        wheel(&mut app, p, 20);
        render(&mut app, 100, 32);
        assert!(app.tab().offset > app.tab().selected);
        app.handle(Input::Key(KeyPress::plain(Key::Down)));
        render(&mut app, 100, 32);
        assert!(app.tab().selected >= app.tab().offset);
        assert!(app.tab().selected < app.tab().offset + app.tab().page_size);
        app.handle(Input::Key(KeyPress::plain(Key::End)));
        render(&mut app, 100, 32);
        assert_eq!(app.tab().selected, 99);
        assert!(app.tab().selected < app.tab().offset + app.tab().page_size);
        app.handle(Input::Key(KeyPress::plain(Key::Home)));
        render(&mut app, 100, 32);
        assert_eq!(app.tab().offset, 0);
    }
}

#[test]
fn wheel_targets_the_hovered_pane_without_changing_keyboard_focus_or_preview() {
    let (root, mut app) = setup(false);
    fs::write(
        root.path().join("file-000.txt"),
        (0..100).map(|i| format!("Line {i}\n")).collect::<String>(),
    )
    .unwrap();
    app.settings.dual = true;
    app.settings.preview = true;
    render(&mut app, 220, 40);
    app.update_preview();
    settle(&mut app);
    let title = app.preview.as_ref().unwrap().title.clone();
    let p = point(&app, 1);
    wheel(&mut app, p, 2);
    render(&mut app, 220, 40);
    assert_eq!(app.active, 0);
    assert_eq!(app.panes[0].tab().offset, 0);
    assert!(app.panes[1].tab().offset > 0);
    assert_eq!(app.panes[0].tab().selected, 0);
    assert_eq!(app.panes[1].tab().selected, 0);
    assert_eq!(app.preview.as_ref().unwrap().title, title);
    assert!(!app.preview_loading);
    let offset = app.panes[1].tab().offset;
    wheel(&mut app, Position::new(0, 0), 1);
    render(&mut app, 220, 40);
    assert_eq!(app.panes[1].tab().offset, offset);
    assert_eq!(app.tab().selected, 0);
    let preview = app.preview_area;
    wheel(&mut app, (preview.x + 1, preview.y + 1).into(), 1);
    assert!(app.preview_offset > 0);
    assert_eq!(app.panes[0].tab().offset, 0);
    assert_eq!(app.panes[1].tab().offset, offset);
    app.action(Action::Help);
    render(&mut app, 220, 40);
    wheel(&mut app, p, 1);
    assert!(app.help_offset > 0);
    assert_eq!(app.panes[1].tab().offset, offset);
}

#[test]
fn scroll_is_bounded_and_refresh_preserves_the_manually_chosen_viewport() {
    for grid in [true, false] {
        let (_root, mut app) = setup(grid);
        let p = point(&app, 0);
        wheel(&mut app, p, 3);
        render(&mut app, 100, 32);
        let offset = app.tab().offset;
        app.action(Action::Refresh);
        settle(&mut app);
        render(&mut app, 100, 32);
        assert_eq!(app.tab().offset, offset);
        wheel(&mut app, p, i32::MAX);
        render(&mut app, 100, 32);
        assert!(
            app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Entry { pane: 0, index: 99 }))
        );
        let bottom = app.tab().offset;
        wheel(&mut app, p, 1);
        render(&mut app, 100, 32);
        assert_eq!(app.tab().offset, bottom);
        render(&mut app, 190, 160);
        assert_eq!(app.tab().offset, 0);
        assert_eq!(app.tab().selected, 0);
        app.action(Action::Search);
        app.handle(Input::Text("no-such-file".into()));
        app.handle(Input::Key(KeyPress::plain(Key::Enter)));
        render(&mut app, 100, 32);
        wheel(&mut app, p, i32::MAX);
        render(&mut app, 100, 32);
        assert!(app.tab().visible.is_empty());
        assert_eq!(app.tab().offset, 0);
    }
}

#[test]
fn rubber_band_after_scrolling_uses_current_entries_and_wheel_preserves_its_set() {
    let (_root, mut app) = setup(false);
    let p = point(&app, 0);
    wheel(&mut app, p, 2);
    render(&mut app, 100, 32);
    let index = first(&app, 0);
    let area = app.file_areas[0];
    app.handle(Input::PointerDown {
        x: area.right() - 2,
        y: area.y + 1,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: area.x + 1,
        y: area.y,
    });
    assert_eq!(app.tab().marked.len(), 2);
    assert!(app.tab().targets().iter().all(
        |p| p.file_name().unwrap().to_str().unwrap() >= format!("file-{index:03}.txt").as_str()
    ));
    let marked = app.tab().marked.clone();
    wheel(&mut app, p, 1);
    render(&mut app, 100, 32);
    assert!(app.selection_area().is_none());
    assert_eq!(app.tab().marked, marked);
    assert_eq!(app.tab().selection_count(), 2);
}

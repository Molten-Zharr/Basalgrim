use basalgrim::{
    app::{App, Dialog, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    layout::{Position, Rect},
};
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
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        fs::write(root.path().join(name), name).unwrap();
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
    render(&mut app, 100);
    (root, app)
}
fn render(app: &mut App, width: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 32)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn entry(app: &App, pane: usize, index: usize) -> Rect {
    app.hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t, Target::Entry { pane:p, index:i } if *p==pane && *i==index).then_some(*r)
        })
        .unwrap()
}
fn start(app: &mut App, point: Position, additive: bool) {
    app.handle(Input::PointerDown {
        x: point.x,
        y: point.y,
        double: false,
        toggle: additive,
    });
}
fn drag(app: &mut App, point: Position) {
    app.handle(Input::Drag {
        x: point.x,
        y: point.y,
    });
}
fn end(app: &mut App, point: Position) {
    app.handle(Input::PointerUp {
        x: point.x,
        y: point.y,
    });
}
fn bounds(app: &App) -> (Position, Position, Position) {
    let a = entry(app, 0, 0);
    let b = entry(app, 0, 1);
    if app.settings.grid {
        (
            (b.right() - 1, a.bottom()).into(),
            (a.x, a.y).into(),
            (b.x, b.y).into(),
        )
    } else {
        (
            (a.right() - 1, b.y).into(),
            (a.x, a.y).into(),
            (b.x, b.y).into(),
        )
    }
}
fn names(app: &App) -> Vec<String> {
    app.tab()
        .targets()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into())
        .collect()
}

#[test]
fn rectangle_marks_intersected_files_and_shrinks_in_grid_and_list() {
    for grid in [true, false] {
        let (_root, mut app) = setup(grid);
        let (origin, point, shrink) = bounds(&app);
        start(&mut app, origin, false);
        let selected = app.tab().selected;
        app.handle(Input::Pointer(Some(point))); // Hover alone must never select.
        assert!(app.selection_area().is_none());
        drag(&mut app, point);
        assert!(app.selection_area().is_some());
        assert_eq!(names(&app), ["alpha.txt", "beta.txt"]);
        assert_eq!(app.tab().selected, selected);
        drag(&mut app, shrink);
        assert_eq!(names(&app), ["beta.txt"]);
        end(&mut app, shrink);
        assert!(app.selection_area().is_none());
        app.action(Action::Copy);
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths[0]
                .file_name()
                .unwrap(),
            "beta.txt"
        );
        assert_eq!(app.operation_count, 0);
    }
}

#[test]
fn ctrl_adds_to_the_original_set_and_escape_or_focus_loss_restores_it() {
    for grid in [true, false] {
        let (root, mut app) = setup(grid);
        let gamma = root.path().join("gamma.txt");
        app.tab_mut().marked.insert(gamma.clone());
        app.tab_mut().explicit_selection = true;
        let (origin, point, shrink) = bounds(&app);
        start(&mut app, origin, true);
        drag(&mut app, point);
        assert_eq!(names(&app), ["alpha.txt", "beta.txt", "gamma.txt"]);
        drag(&mut app, shrink);
        assert_eq!(names(&app), ["beta.txt", "gamma.txt"]);
        app.handle(Input::Key(KeyPress::plain(Key::Escape)));
        assert_eq!(names(&app), ["gamma.txt"]);
        assert!(app.selection_area().is_none());
        start(&mut app, origin, false);
        drag(&mut app, point);
        app.handle(Input::CancelDrag);
        assert_eq!(names(&app), ["gamma.txt"]);
        start(&mut app, origin, true);
        drag(&mut app, point);
        end(&mut app, point);
        assert_eq!(app.tab().marked.len(), 3);
    }
}

#[test]
fn ctrl_rectangle_preserves_and_restores_the_original_single_file() {
    let (_root, mut app) = setup(false);
    let original = app.tab().targets();
    let beta = entry(&app, 0, 1);
    let gamma = entry(&app, 0, 2);
    start(&mut app, (beta.right() - 1, beta.y).into(), true);
    drag(&mut app, (gamma.x, gamma.y).into());
    assert_eq!(app.tab().marked.len(), 3);
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert_eq!(app.tab().targets(), original);
}

#[test]
fn empty_rectangle_is_an_empty_selection_and_does_not_copy_the_focused_file() {
    let (_root, mut app) = setup(true);
    let area = app.file_areas[0];
    let origin = Position::new(area.x + 2, area.bottom() - 4);
    let point = Position::new(area.x + 10, area.bottom() - 1);
    start(&mut app, origin, false);
    let baseline = render(&mut app, 100);
    drag(&mut app, point);
    assert!(app.tab().targets().is_empty());
    let outline = app.selection_area().unwrap();
    let selected = render(&mut app, 100);
    assert_ne!(baseline, selected);
    for y in 0..32 {
        for x in 0..100 {
            if !outline.contains(Position::new(x, y)) {
                assert_eq!(baseline[(x, y)], selected[(x, y)]);
            }
        }
    }
    end(&mut app, point);
    assert_eq!(render(&mut app, 100), baseline);
    app.action(Action::Copy);
    app.action(Action::Rename);
    assert!(app.clipboard.is_none());
    assert!(app.dialog.is_none());
    app.handle(Input::Key(KeyPress::plain(Key::Down)));
    assert_eq!(app.tab().selection_count(), 1);
}

#[test]
fn drag_is_clamped_to_its_pane_and_never_activates_controls_or_other_panes() {
    let (root, mut app) = setup(true);
    app.settings.dual = true;
    render(&mut app, 180);
    let area = app.file_areas[0];
    start(&mut app, (area.x + 1, area.y + 1).into(), false);
    drag(&mut app, Position::new(5000, 5000));
    assert_eq!(
        app.selection_area().unwrap().intersection(area),
        app.selection_area().unwrap()
    );
    assert_eq!(app.tab().marked.len(), 3);
    assert!(app.panes[1].tab().marked.is_empty());
    assert_eq!(app.active, 0);
    end(&mut app, Position::new(5000, 5000));
    assert_eq!(app.tab().cwd, root.path());
    assert!(!app.busy);
    let settings = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Action(Action::Settings)).then_some(*r))
        .unwrap();
    start(&mut app, (settings.x, settings.y).into(), false);
    drag(&mut app, (area.x, area.y).into());
    assert!(matches!(app.dialog, Some(Dialog::Settings)));
    assert!(app.selection_area().is_none());
}

#[test]
fn resize_refresh_navigation_and_removed_tabs_invalidate_a_drag_safely() {
    let (root, mut app) = setup(true);
    let (origin, point, _) = bounds(&app);
    app.tab_mut().marked.insert(root.path().join("gamma.txt"));
    start(&mut app, origin, false);
    drag(&mut app, point);
    render(&mut app, 120);
    assert!(app.selection_area().is_none());
    assert_eq!(names(&app), ["gamma.txt"]);
    render(&mut app, 100);
    start(&mut app, origin, false);
    drag(&mut app, point);
    render(&mut app, 25);
    assert!(app.selection_area().is_none());
    assert_eq!(names(&app), ["gamma.txt"]);
    render(&mut app, 100);
    start(&mut app, origin, false);
    drag(&mut app, point);
    app.action(Action::Refresh);
    app.tick();
    assert!(app.selection_area().is_none());
    settle(&mut app);
    app.action(Action::NewTab);
    settle(&mut app);
    render(&mut app, 100);
    start(&mut app, origin, false);
    drag(&mut app, point);
    app.action(Action::CloseTab);
    app.tick();
    assert!(app.selection_area().is_none());
    render(&mut app, 100);
    start(&mut app, origin, false);
    drag(&mut app, point);
    let next = root.path().join("empty");
    fs::create_dir(&next).unwrap();
    app.navigate(next, true);
    app.tick();
    settle(&mut app);
    assert!(app.selection_area().is_none());
    assert!(app.tab().marked.is_empty());
}

#[test]
fn ordinary_clicks_checkboxes_and_double_clicks_keep_their_behavior() {
    let (root, mut app) = setup(true);
    fs::create_dir(root.path().join("folder")).unwrap();
    app.action(Action::Refresh);
    settle(&mut app);
    render(&mut app, 100);
    let first = entry(&app, 0, 0);
    start(&mut app, (first.x, first.y).into(), false);
    end(&mut app, (first.x, first.y).into());
    assert!(app.selection_area().is_none());
    assert_eq!(app.tab().cwd, root.path());
    app.handle(Input::PointerDown {
        x: first.x,
        y: first.y,
        double: true,
        toggle: false,
    });
    end(&mut app, (first.x, first.y).into());
    settle(&mut app);
    assert_eq!(app.tab().cwd, root.path().join("folder"));
    app.navigate(root.path().to_path_buf(), true);
    settle(&mut app);
    render(&mut app, 100);
    let mark = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::EntryMark { index: 1, .. }).then_some(*r))
        .unwrap();
    start(&mut app, (mark.x, mark.y).into(), false);
    drag(&mut app, Position::new(1, 1));
    assert!(app.selection_area().is_none());
    assert_eq!(app.tab().marked.len(), 1);
}

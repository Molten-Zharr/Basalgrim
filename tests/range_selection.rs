use basalgrim::{
    app::{App, Target},
    fs::Sort,
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() || app.busy {
        assert!(Instant::now() < end, "worker timed out: {}", app.notice);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup(grid: bool) -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    for i in 0..100 {
        fs::write(
            root.path().join(format!("file-{i:03}.txt")),
            vec![b'x'; i + 1],
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
    render(&mut app, 170, 53);
    (root, app)
}
fn render(app: &mut App, width: u16, height: u16) {
    Terminal::new(TestBackend::new(width, height))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn key(app: &mut App, key: Key, shift: bool, ctrl: bool) {
    app.handle(Input::Key(KeyPress {
        key,
        shift,
        ctrl,
        alt: false,
    }));
}
fn point(app: &App, pane: usize, index: usize, mark: bool) -> Position {
    app.hits
        .iter()
        .find_map(|(r, t)| match t {
            Target::Entry { pane: p, index: i } if !mark && *p == pane && *i == index => {
                Some((r.x + 5.min(r.width - 1), r.y + 2.min(r.height - 1)).into())
            }
            Target::EntryMark { pane: p, index: i } if mark && *p == pane && *i == index => {
                Some((r.x, r.y).into())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing entry {pane}/{index}, mark={mark}"))
}
fn down(app: &mut App, p: Position, shift: bool, ctrl: bool, double: bool) {
    app.handle(Input::SelectionModifier(shift));
    app.handle(Input::PointerDown {
        x: p.x,
        y: p.y,
        double,
        toggle: ctrl,
    });
}
fn up(app: &mut App, p: Position) {
    // Modifiers may be released before the button. The press owns the click.
    app.handle(Input::SelectionModifier(false));
    app.handle(Input::PointerUp { x: p.x, y: p.y });
}
fn click(app: &mut App, index: usize, shift: bool, ctrl: bool) {
    let p = point(app, app.active, index, false);
    down(app, p, shift, ctrl, false);
    up(app, p);
}
fn names(app: &App) -> Vec<String> {
    app.tab()
        .targets()
        .into_iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}
fn expected(range: std::ops::RangeInclusive<usize>) -> Vec<String> {
    range.map(|i| format!("file-{i:03}.txt")).collect()
}

#[test]
fn keyboard_extends_shrinks_reverses_and_reveals_focus_in_grid_list_and_compact() {
    for grid in [false, true] {
        for (width, height) in [(170, 53), (34, 14)] {
            let (_root, mut app) = setup(grid);
            render(&mut app, width, height);
            let columns = app.tab().columns;
            key(&mut app, Key::Down, false, false);
            key(&mut app, Key::Down, true, false);
            assert_eq!(names(&app), expected(columns..=columns * 2));
            key(&mut app, Key::Up, true, false);
            assert_eq!(names(&app), expected(columns..=columns));
            key(&mut app, Key::Up, true, false);
            assert_eq!(names(&app), expected(0..=columns));
            if grid {
                key(&mut app, Key::Right, true, false);
                assert_eq!(names(&app), expected(1..=columns));
                key(&mut app, Key::Left, true, false);
            }
            key(&mut app, Key::Home, false, false);
            let p = Position::new(app.file_areas[0].x, app.file_areas[0].y);
            app.handle(Input::Scroll {
                x: p.x,
                y: p.y,
                delta: 50,
            });
            render(&mut app, width, height);
            assert!(app.tab().offset > 0);
            let page = app.tab().page_size;
            key(&mut app, Key::PageDown, true, false);
            render(&mut app, width, height);
            assert_eq!(names(&app), expected(0..=page.min(99)));
            assert!(app.tab().selected >= app.tab().offset);
            assert!(app.tab().selected < app.tab().offset + app.tab().page_size);
            key(&mut app, Key::PageUp, true, false);
            assert_eq!(names(&app), expected(0..=0));
            key(&mut app, Key::End, true, false);
            assert_eq!(names(&app), expected(0..=99));
            key(&mut app, Key::Home, true, false);
            assert_eq!(names(&app), expected(0..=0));
        }
    }
}

#[test]
fn mouse_and_checkboxes_share_anchor_and_ctrl_shift_preserves_only_the_original_base() {
    for grid in [false, true] {
        let (_root, mut app) = setup(grid);
        click(&mut app, 2, false, false);
        click(&mut app, 6, true, false);
        assert_eq!(names(&app), expected(2..=6));
        click(&mut app, 3, true, false);
        assert_eq!(names(&app), expected(2..=3));
        click(&mut app, 0, true, false);
        assert_eq!(names(&app), expected(0..=2));
        click(&mut app, 7, false, true); // Add a disjoint item and anchor the next range there.
        click(&mut app, 4, true, true);
        assert_eq!(names(&app), [expected(0..=2), expected(4..=7)].concat());
        click(&mut app, 6, true, true);
        assert_eq!(names(&app), [expected(0..=2), expected(6..=7)].concat());
        key(&mut app, Key::Home, true, true);
        assert_eq!(names(&app), expected(0..=7));
        let p = point(&app, 0, 6, true);
        down(&mut app, p, true, true, false);
        up(&mut app, p);
        assert_eq!(names(&app), [expected(0..=2), expected(6..=7)].concat());
        app.action(Action::Copy);
        assert_eq!(app.clipboard.as_ref().unwrap().paths, app.tab().targets());
        click(&mut app, 5, false, false);
        key(&mut app, Key::Home, true, false);
        assert_eq!(names(&app), expected(0..=5));
    }
}

#[test]
fn shift_press_is_captured_and_rapid_click_never_opens_a_folder() {
    let (root, mut app) = setup(false);
    fs::create_dir(root.path().join("folder")).unwrap();
    app.refresh();
    settle(&mut app);
    key(&mut app, Key::Home, false, false);
    render(&mut app, 170, 53);
    click(&mut app, 2, false, false);
    let folder = point(&app, 0, 0, false);
    down(&mut app, folder, true, false, true);
    up(&mut app, folder);
    assert_eq!(app.tab().cwd, root.path());
    assert_eq!(app.tab().selection_count(), 3);
    let other = point(&app, 0, 4, false);
    down(&mut app, other, false, false, false);
    app.handle(Input::SelectionModifier(true));
    app.handle(Input::PointerUp {
        x: other.x,
        y: other.y,
    });
    assert_eq!(names(&app), expected(3..=3));
}

#[test]
fn wheel_hover_and_refresh_keep_a_path_anchor_instead_of_a_row_number() {
    for grid in [false, true] {
        let (root, mut app) = setup(grid);
        click(&mut app, 2, false, false);
        click(&mut app, 5, true, false);
        let p = point(&app, 0, 7, false);
        app.handle(Input::Pointer(Some(p)));
        app.handle(Input::Scroll {
            x: p.x,
            y: p.y,
            delta: 3,
        });
        assert_eq!(names(&app), expected(2..=5));
        fs::write(root.path().join("aaa.txt"), "").unwrap();
        app.refresh();
        settle(&mut app);
        key(&mut app, Key::Home, true, false);
        assert_eq!(
            names(&app),
            [vec!["aaa.txt".into()], expected(0..=2)].concat()
        );
        fs::remove_file(root.path().join("file-002.txt")).unwrap();
        app.refresh();
        settle(&mut app);
        key(&mut app, Key::Down, true, false);
        let columns = app.tab().columns;
        assert_eq!(app.tab().selection_count(), columns + 1);
        assert!(
            !app.tab()
                .targets()
                .contains(&root.path().join("file-002.txt"))
        );
    }
}

#[test]
fn range_uses_current_sort_and_filter_and_never_selects_hidden_rows() {
    let (_root, mut app) = setup(false);
    click(&mut app, 2, false, false);
    app.tab_mut().sort = Sort::Size;
    app.tab_mut().rebuild(false);
    key(&mut app, Key::End, true, false);
    assert_eq!(names(&app), expected(0..=2));
    app.tab_mut().filter = "file-09".into();
    app.tab_mut().rebuild(false);
    key(&mut app, Key::Home, true, false);
    assert_eq!(names(&app), expected(90..=99));
    key(&mut app, Key::Home, false, false);
    key(&mut app, Key::Down, true, false);
    assert_eq!(names(&app), expected(98..=99));
    app.tab_mut().filter = "no match".into();
    app.tab_mut().rebuild(false);
    key(&mut app, Key::End, true, false);
    assert!(app.tab().targets().is_empty());
}

#[test]
fn each_tab_and_pane_keeps_its_own_anchor_and_navigation_starts_a_new_one() {
    let (root, mut app) = setup(false);
    click(&mut app, 2, false, false);
    click(&mut app, 5, true, false);
    app.action(Action::NewTab);
    settle(&mut app);
    render(&mut app, 170, 53);
    click(&mut app, 6, false, false);
    app.action(Action::NextTab);
    key(&mut app, Key::Home, true, false);
    assert_eq!(names(&app), expected(0..=2));
    app.action(Action::NextTab);
    key(&mut app, Key::Home, true, false);
    assert_eq!(names(&app), expected(0..=6));
    app.settings.dual = true;
    app.action(Action::NextPane);
    settle(&mut app);
    render(&mut app, 170, 53);
    click(&mut app, 3, false, false);
    key(&mut app, Key::Home, true, false);
    assert_eq!(names(&app), expected(0..=3));
    app.action(Action::NextPane);
    key(&mut app, Key::Down, true, false);
    assert_eq!(names(&app), expected(1..=6));
    app.navigate(root.path().into(), true);
    settle(&mut app);
    key(&mut app, Key::Down, true, false);
    assert_eq!(names(&app), expected(0..=1));
}

#[test]
fn cancelled_file_drag_or_marquee_restores_the_range_anchor_and_additive_base() {
    for grid in [false, true] {
        let (_root, mut app) = setup(grid);
        click(&mut app, 1, false, false);
        click(&mut app, 2, true, false);
        click(&mut app, 7, false, true);
        click(&mut app, 5, true, true);
        let before = app.tab().targets();
        let p = point(&app, 0, 4, false);
        down(&mut app, p, true, true, false);
        app.handle(Input::Drag { x: p.x + 1, y: p.y });
        app.handle(Input::CancelDrag);
        assert_eq!(app.tab().targets(), before);
        click(&mut app, 6, true, true);
        assert_eq!(names(&app), [expected(1..=2), expected(6..=7)].concat());
        let area = app.file_areas[0];
        let origin = Position::new(area.right() - 1, area.bottom() - 1);
        down(&mut app, origin, false, false, false);
        app.handle(Input::Drag { x: p.x, y: p.y });
        key(&mut app, Key::Escape, false, false);
        click(&mut app, 5, true, true);
        assert_eq!(names(&app), [expected(1..=2), expected(5..=7)].concat());
    }
}

#[test]
fn selected_range_can_be_dragged_and_cancelled_without_changing_the_clipboard() {
    for grid in [false, true] {
        let (root, mut app) = setup(grid);
        fs::create_dir(root.path().join("destination")).unwrap();
        app.refresh();
        settle(&mut app);
        key(&mut app, Key::Home, false, false);
        render(&mut app, 170, 53);
        click(&mut app, 2, false, false);
        click(&mut app, 4, true, false);
        app.action(Action::Copy);
        let original = app.clipboard.as_ref().unwrap().paths.clone();
        let p = point(&app, 0, 3, false);
        let to = point(&app, 0, 0, false);
        down(&mut app, p, false, false, false);
        app.handle(Input::Drag { x: to.x, y: to.y });
        app.handle(Input::DragModifiers { copy: true });
        assert_eq!(app.drag_feedback().unwrap().count, 3);
        up(&mut app, to);
        settle(&mut app);
        assert_eq!(app.clipboard.as_ref().unwrap().paths, original);
        let copied: Vec<PathBuf> = fs::read_dir(root.path().join("destination"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into())
            .collect();
        assert_eq!(copied.len(), 3);
        for i in 1..=3 {
            assert!(
                root.path()
                    .join(format!("destination/file-{i:03}.txt"))
                    .exists()
            );
        }
    }
}

#[test]
fn escape_select_all_and_plain_navigation_reset_continuation() {
    let (_root, mut app) = setup(false);
    click(&mut app, 2, false, false);
    click(&mut app, 6, true, false);
    key(&mut app, Key::Escape, false, false);
    key(&mut app, Key::Up, true, false);
    assert_eq!(names(&app), expected(5..=6));
    key(&mut app, Key::Down, false, false);
    key(&mut app, Key::Up, true, false);
    assert_eq!(names(&app), expected(5..=6));
    app.action(Action::SelectAll);
    key(&mut app, Key::Home, true, false);
    assert_eq!(names(&app), expected(0..=5));
    click(&mut app, 2, false, false);
    click(&mut app, 2, false, true);
    click(&mut app, 2, false, true);
    assert!(app.tab().targets().is_empty());
    key(&mut app, Key::Down, true, false);
    assert_eq!(names(&app), expected(2..=3));
}

#[test]
fn confirmed_rename_rebinds_anchor_to_the_actual_new_path() {
    let (root, mut app) = setup(false);
    click(&mut app, 2, false, false);
    click(&mut app, 5, true, false);
    // Rename the anchor through the opposite pane. The first pane's focused
    // endpoint is elsewhere, so falling back to focus would select too much.
    app.settings.dual = true;
    app.active = 1;
    render(&mut app, 170, 53);
    click(&mut app, 2, false, false);
    app.action(Action::Rename);
    key(&mut app, Key::Char('a'), false, true);
    app.handle(Input::Text("file-002-new.txt".into()));
    key(&mut app, Key::Enter, false, false);
    settle(&mut app);
    assert!(root.path().join("file-002-new.txt").exists());
    app.active = 0;
    key(&mut app, Key::Home, true, false);
    assert_eq!(
        names(&app),
        vec!["file-000.txt", "file-001.txt", "file-002-new.txt"]
    );
}

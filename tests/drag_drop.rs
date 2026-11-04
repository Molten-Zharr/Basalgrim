use basalgrim::{
    app::{App, Clipboard, Dialog, Target},
    conflicts::Choice,
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(8);
    while app.loading() || app.journal_loading {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, width: u16, height: u16) {
    Terminal::new(TestBackend::new(width, height))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn fixture(grid: bool) -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let dest = temp.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&dest).unwrap();
    fs::create_dir(source.join("folder")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(source.join(name), name).unwrap();
    }
    let mut app = App::new(
        source.clone(),
        Settings {
            grid,
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    render(&mut app, 140, 32);
    (temp, app, source, dest)
}
fn entry(app: &App, pane: usize, name: &str) -> Position {
    let index = app
        .file_tab(pane)
        .visible
        .iter()
        .position(|&i| app.file_tab(pane).entries[i].name == name)
        .unwrap();
    let r = app
        .hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t,Target::Entry {pane:p,index:i} if *p==pane && *i==index).then_some(*r)
        })
        .unwrap();
    Position::new(r.x + 5.min(r.width - 1), r.y + 2.min(r.height - 1))
}
fn down(app: &mut App, p: Position, ctrl: bool) {
    app.handle(Input::PointerDown {
        x: p.x,
        y: p.y,
        double: false,
        toggle: ctrl,
    });
}
fn drag(app: &mut App, p: Position, copy: bool) {
    app.handle(Input::DragModifiers { copy });
    app.handle(Input::Drag { x: p.x, y: p.y });
}
fn up(app: &mut App, p: Position) {
    app.handle(Input::PointerUp { x: p.x, y: p.y });
}
fn background(app: &App, pane: usize) -> Position {
    let r = app.file_areas[pane];
    Position::new(r.x + 4, r.bottom() - 1)
}
fn pane(app: &mut App, dest: &Path) {
    app.settings.dual = true;
    app.active = 1;
    app.navigate(dest.into(), true);
    settle(app);
    app.active = 0;
    render(app, 180, 32);
}

#[test]
fn files_drag_into_folders_in_grid_list_and_minimum_layout_with_ctrl_copy() {
    for grid in [false, true] {
        for (width, height) in [(140, 32), (34, 14)] {
            for copy in [false, true] {
                let (_temp, mut app, source, _dest) = fixture(grid);
                render(&mut app, width, height);
                // Scroll the focused file into the very small viewport when needed.
                app.tab_mut().selected = 1;
                app.tab_mut().follow_selection = true;
                render(&mut app, width, height);
                let a = entry(&app, 0, "a.txt");
                down(&mut app, a, false);
                if !app
                    .hits
                    .iter()
                    .any(|(_, t)| matches!(t, Target::Entry { pane: 0, index: 0 }))
                {
                    let area = app.file_areas[0];
                    drag(&mut app, Position::new(area.x + 4, area.y), copy);
                    std::thread::sleep(Duration::from_millis(110));
                    app.tick();
                    render(&mut app, width, height);
                }
                let folder = entry(&app, 0, "folder");
                drag(&mut app, folder, copy);
                render(&mut app, width, height);
                assert!(app.selection_area().is_none());
                assert!(app.dragging_files());
                assert_eq!(
                    app.drag_feedback().unwrap().destination,
                    Some(source.join("folder"))
                );
                up(&mut app, folder);
                settle(&mut app);
                assert_eq!(
                    fs::read_to_string(source.join("folder/a.txt")).unwrap(),
                    "a.txt"
                );
                assert_eq!(source.join("a.txt").exists(), copy);
            }
        }
    }
}
#[test]
fn marked_group_moves_to_the_other_pane_without_overwriting_the_file_clipboard() {
    for grid in [false, true] {
        let (_temp, mut app, source, dest) = fixture(grid);
        pane(&mut app, &dest);
        app.tab_mut().marked = [source.join("a.txt"), source.join("b.txt")].into();
        app.tab_mut().explicit_selection = true;
        app.clipboard = Some(Clipboard {
            paths: vec![source.join("c.txt")],
            cut: false,
        });
        render(&mut app, 180, 32);
        let a = entry(&app, 0, "b.txt");
        let target = background(&app, 1);
        down(&mut app, a, false);
        assert_eq!(
            app.tab().marked.len(),
            2,
            "Press must preserve the marked group until a click/drag is resolved"
        );
        drag(&mut app, target, false);
        assert_eq!(app.drag_feedback().unwrap().count, 2);
        assert_eq!(app.active, 0);
        up(&mut app, target);
        settle(&mut app);
        assert!(dest.join("a.txt").exists() && dest.join("b.txt").exists());
        assert!(!source.join("a.txt").exists() && source.join("c.txt").exists());
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths,
            [source.join("c.txt")]
        );
    }
}
#[test]
fn drop_on_inactive_tab_uses_its_captured_directory_without_switching_tabs() {
    let (_temp, mut app, source, dest) = fixture(false);
    app.action(Action::NewTab);
    settle(&mut app);
    app.navigate(dest.clone(), true);
    settle(&mut app);
    app.panes[0].active = 0;
    render(&mut app, 140, 32);
    let tab = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t, Target::Tab { pane: 0, index: 1 }).then_some(*r))
        .unwrap();
    let from = entry(&app, 0, "a.txt");
    down(&mut app, from, false);
    let target = Position::new(tab.x + 1, tab.y);
    drag(&mut app, target, true);
    up(&mut app, target);
    settle(&mut app);
    assert_eq!(app.panes[0].active, 0);
    assert_eq!(app.tab().cwd, source);
    assert!(dest.join("a.txt").exists());
}
#[test]
fn escape_focus_loss_invalid_targets_and_resize_restore_original_selection_without_io() {
    for how in 0..5 {
        let (_temp, mut app, source, _dest) = fixture(true);
        app.tab_mut().marked = [source.join("b.txt")].into();
        app.tab_mut().explicit_selection = true;
        let before = app.tab().targets();
        let from = entry(&app, 0, "a.txt");
        let to = entry(&app, 0, "folder");
        down(&mut app, from, false);
        drag(&mut app, to, false);
        match how {
            0 => app.handle(Input::Key(KeyPress::plain(Key::Escape))),
            1 => app.handle(Input::CancelDrag),
            2 => render(&mut app, 100, 30),
            3 => {
                let other = entry(&app, 0, "c.txt");
                up(&mut app, other);
            }
            _ => up(&mut app, Position::new(0, 0)),
        }
        up(&mut app, to);
        assert!(!app.dragging_files() && !app.busy);
        assert_eq!(app.operation_count, 0);
        assert_eq!(app.tab().targets(), before);
        assert!(source.join("a.txt").exists() && !source.join("folder/a.txt").exists());
    }
}
#[test]
fn recursive_and_same_directory_moves_are_rejected_but_ctrl_copy_can_duplicate() {
    let (_temp, mut app, source, _dest) = fixture(false);
    let folder = entry(&app, 0, "folder");
    down(&mut app, folder, false);
    drag(&mut app, folder, true);
    up(&mut app, folder);
    assert!(!app.busy);
    let file = entry(&app, 0, "a.txt");
    let bg = background(&app, 0);
    down(&mut app, file, false);
    drag(&mut app, bg, false);
    up(&mut app, bg);
    assert!(!app.busy);
    down(&mut app, file, false);
    drag(&mut app, bg, true);
    up(&mut app, bg);
    let end = Instant::now() + Duration::from_secs(5);
    while !matches!(app.dialog, Some(Dialog::Conflict(_))) {
        app.tick();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    app.choose_conflict(Choice::KeepBoth);
    settle(&mut app);
    assert!(source.join("a (1).txt").exists());
}
#[test]
fn moved_files_are_journalled_and_survive_undo_redo_after_reopening() {
    let (temp, mut app, source, dest) = fixture(false);
    let journal = temp.path().join("journal");
    app.attach_journal(journal.clone());
    settle(&mut app);
    pane(&mut app, &dest);
    let a = entry(&app, 0, "a.txt");
    let bg = background(&app, 1);
    down(&mut app, a, false);
    drag(&mut app, bg, false);
    up(&mut app, bg);
    settle(&mut app);
    app.action(Action::Undo);
    settle(&mut app);
    assert!(source.join("a.txt").exists() && !dest.join("a.txt").exists());
    drop(app);
    let mut app = App::new(source.clone(), Settings::default());
    app.attach_journal(journal);
    settle(&mut app);
    app.action(Action::Redo);
    settle(&mut app);
    assert!(!source.join("a.txt").exists() && dest.join("a.txt").exists());
}
#[test]
fn stale_target_tab_geometry_cannot_redirect_a_drop_after_navigation() {
    let (_temp, mut app, source, dest) = fixture(false);
    pane(&mut app, &dest);
    let a = entry(&app, 0, "a.txt");
    let bg = background(&app, 1);
    down(&mut app, a, false);
    drag(&mut app, bg, false);
    app.panes[1].tab_mut().request += 1;
    up(&mut app, bg);
    assert!(!app.busy && source.join("a.txt").exists());
}

#[test]
fn a_quick_second_press_can_drag_instead_of_opening_the_directory() {
    let (_temp, mut app, source, dest) = fixture(false);
    pane(&mut app, &dest);
    let from = entry(&app, 0, "folder");
    let target = background(&app, 1);
    app.handle(Input::PointerDown {
        x: from.x,
        y: from.y,
        double: true,
        toggle: false,
    });
    assert_eq!(
        app.tab().cwd,
        source,
        "Opening waits for release without motion"
    );
    drag(&mut app, target, false);
    up(&mut app, target);
    settle(&mut app);
    assert_eq!(app.tab().cwd, source);
    assert!(dest.join("folder").is_dir());
}

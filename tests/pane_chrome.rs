use basalgrim::{
    app::{App, Target},
    input::{Action, Input},
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
    path::Path,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn setup(root: &Path, grid: bool) -> App {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("file.txt"), "payload").unwrap();
    let mut app = App::new(
        root.into(),
        Settings {
            preview: false,
            grid,
            ..Default::default()
        },
    );
    settle(&mut app);
    app
}
fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    t.backend().buffer().clone()
}
fn hit(app: &App, p: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| p(t).then_some(*r))
        .expect("Missing mouse control")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
fn down(app: &mut App, r: Rect) {
    app.handle(Input::PointerDown {
        x: r.x + 5.min(r.width - 1),
        y: r.y + 2.min(r.height - 1),
        double: false,
        toggle: false,
    });
}
fn drag(app: &mut App, r: Rect, copy: bool) {
    app.handle(Input::DragModifiers { copy });
    app.handle(Input::Drag { x: r.x, y: r.y });
}

#[test]
fn toolbar_is_short_and_function_buttons_live_below_files_at_normal_and_minimum_sizes() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = setup(temp.path(), false);
    for (w, h) in [(170, 53), (80, 24), (34, 14)] {
        render(&mut app, w, h);
        let pane = app.pane_areas[app.active];
        for action in [Action::Copy, Action::Cut, Action::Paste] {
            assert!(
                !app.hits
                    .iter()
                    .any(|(_, t)| matches!(t,Target::Action(a) if *a==action))
            );
        }
        for action in [Action::Operations, Action::Settings, Action::Help] {
            let r = hit(&app, |t| matches!(t,Target::Action(a) if *a==action));
            assert!(r.y >= pane.bottom(), "{action:?} stayed in the top toolbar");
        }
        let new = hit(&app, |t| matches!(t, Target::NewTab { pane: 0 }));
        assert!(pane.contains(Position::new(new.x, new.y)));
        assert!(
            app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Entry { .. })),
            "Tab controls hid the files at minimum size"
        );
        let toggle = hit(&app, |t| matches!(t, Target::Action(Action::Dual)));
        assert_eq!(toggle.y, pane.y);
    }
}
#[test]
fn plus_creates_tabs_in_its_own_pane_and_remains_available_when_the_row_overflows() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = setup(temp.path(), false);
    app.action(Action::Dual);
    render(&mut app, 170, 40);
    let original = app.panes[0].tab().cwd.clone();
    let plus = hit(&app, |t| matches!(t, Target::NewTab { pane: 1 }));
    click(&mut app, plus);
    settle(&mut app);
    assert_eq!(app.active, 1);
    assert_eq!(app.panes[0].tabs.len(), 1);
    assert_eq!(app.panes[0].tab().cwd, original);
    assert_eq!(app.panes[1].tabs.len(), 2);
    app.action(Action::Dual);
    for _ in 0..12 {
        render(&mut app, 34, 14);
        let plus = hit(&app, |t| matches!(t, Target::NewTab { pane: 0 }));
        click(&mut app, plus);
        settle(&mut app);
        render(&mut app, 34, 14);
        assert!(
            app.hits.iter().any(
                |(_, t)| matches!(t,Target::Tab {pane:0,index} if *index==app.panes[0].active)
            )
        );
    }
    let active = app.panes[0].active;
    let previous = hit(
        &app,
        |t| matches!(t,Target::Tab {pane:0,index} if *index<active),
    );
    click(&mut app, previous);
    assert!(app.panes[0].active < active);
}
#[test]
fn second_pane_button_opens_and_closes_the_same_saved_pane_without_losing_tabs() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = setup(temp.path(), false);
    render(&mut app, 170, 40);
    let toggle = hit(&app, |t| matches!(t, Target::Action(Action::Dual)));
    click(&mut app, toggle);
    render(&mut app, 170, 40);
    assert!(!app.pane_areas[1].is_empty());
    let plus = hit(&app, |t| matches!(t, Target::NewTab { pane: 1 }));
    click(&mut app, plus);
    settle(&mut app);
    render(&mut app, 170, 40);
    let close = hit(&app, |t| matches!(t, Target::Action(Action::Dual)));
    assert!(app.pane_areas[1].contains((close.x, close.y).into()));
    click(&mut app, close);
    assert!(!app.settings.dual);
    assert_eq!(app.active, 0);
    assert_eq!(app.panes[1].tabs.len(), 2);
    render(&mut app, 170, 40);
    let toggle = hit(&app, |t| matches!(t, Target::Action(Action::Dual)));
    click(&mut app, toggle);
    assert_eq!(app.panes[1].tabs.len(), 2);
}
#[test]
fn breadcrumbs_accept_groups_several_levels_up_without_navigating_in_grid_and_list() {
    for grid in [false, true] {
        for copy in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("parent");
            let cwd = parent.join("one/two");
            let mut app = setup(&cwd, grid);
            fs::write(cwd.join("other.txt"), "other").unwrap();
            app.action(Action::Refresh);
            settle(&mut app);
            app.tab_mut().marked = [cwd.join("file.txt"), cwd.join("other.txt")].into();
            app.tab_mut().explicit_selection = true;
            render(&mut app, 200, 32);
            let source = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
            let dest = hit(
                &app,
                |t| matches!(t,Target::PathGo {path,..} if *path==parent),
            );
            down(&mut app, source);
            drag(&mut app, dest, copy);
            assert_eq!(
                app.drag_feedback().unwrap().destination,
                Some(parent.clone())
            );
            assert_eq!(app.gesture_area(), Some(dest));
            app.handle(Input::PointerUp {
                x: dest.x,
                y: dest.y,
            });
            settle(&mut app);
            assert_eq!(app.tab().cwd, cwd);
            assert!(app.dialog.is_none());
            for name in ["file.txt", "other.txt"] {
                assert!(parent.join(name).exists());
                assert_eq!(cwd.join(name).exists(), copy);
            }
        }
    }
}
#[test]
fn breadcrumb_from_the_other_pane_is_guarded_against_replaced_tab_geometry() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let mut app = setup(&source, false);
    let other = temp.path().join("target/child");
    fs::create_dir_all(&other).unwrap();
    app.action(Action::Dual);
    app.active = 1;
    app.navigate(other.clone(), true);
    settle(&mut app);
    app.active = 0;
    render(&mut app, 240, 40);
    let from = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    let to = hit(
        &app,
        |t| matches!(t,Target::PathGo {owner,path} if owner.pane==1 && *path==temp.path().join("target")),
    );
    down(&mut app, from);
    drag(&mut app, to, false);
    app.panes[1].tab_mut().request += 1;
    app.handle(Input::PointerUp { x: to.x, y: to.y });
    assert!(!app.busy);
    assert!(source.join("file.txt").exists());
}

#[test]
fn stale_tab_controls_are_ignored_after_closing_a_tab_or_the_other_pane() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = setup(temp.path(), false);
    app.action(Action::NewTab);
    settle(&mut app);
    render(&mut app, 170, 40);
    let closed = hit(&app, |t| matches!(t, Target::Tab { pane: 0, index: 1 }));
    app.action(Action::CloseTab);
    click(&mut app, closed);
    assert_eq!(app.panes[0].active, 0);
    assert_eq!(app.panes[0].tabs.len(), 1);
    app.action(Action::Dual);
    render(&mut app, 170, 40);
    let plus = hit(&app, |t| matches!(t, Target::NewTab { pane: 1 }));
    app.action(Action::Dual);
    click(&mut app, plus);
    assert!(!app.settings.dual);
    assert_eq!(app.panes[1].tabs.len(), 1);
}

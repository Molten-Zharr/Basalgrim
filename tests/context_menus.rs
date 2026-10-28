use basalgrim::{
    app::{App, Clipboard, Dialog, Target},
    input::{Action, Input},
    preferences::{BorderWeight, Theme},
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

fn setup() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("folder")).unwrap();
    for name in ["alpha.txt", "beta.txt"] {
        fs::write(root.path().join(name), name).unwrap();
    }
    let mut app = App::new(root.path().into(), Settings::default());
    let end = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < end);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
    (root, app)
}
fn render(app: &mut App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(150, 44)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
    terminal.backend().buffer().clone()
}
fn entry(app: &mut App, index: usize) -> Rect {
    render(app);
    app.hits
        .iter()
        .find_map(|(r, t)| {
            matches!(t, Target::Entry { index: i, pane: 0 } if *i == index).then_some(*r)
        })
        .unwrap()
}
fn right(app: &mut App, x: u16, y: u16) {
    app.handle(Input::SecondaryClick { x, y });
}
fn actions(app: &App) -> Vec<Action> {
    app.command_items().into_iter().map(|i| i.action).collect()
}
fn background(app: &mut App) -> Position {
    render(app);
    let pane = app.pane_areas[app.active];
    (pane.right() - 2, pane.bottom() - 2).into()
}

#[test]
fn right_click_offers_only_commands_for_one_entry_or_the_marked_group() {
    let (root, mut app) = setup();
    for grid in [true, false] {
        app.settings.grid = grid;
        for index in [0, 1] {
            // a folder and a file
            let r = entry(&mut app, index);
            right(&mut app, r.x, r.y);
            assert_eq!(
                actions(&app),
                [
                    Action::Open,
                    Action::Copy,
                    Action::Cut,
                    Action::Rename,
                    Action::Trash
                ]
            );
            assert!(app.command_items().iter().all(|i| i.enabled));
            let text: String = render(&mut app)
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                text.contains("Копировать"),
                "Normal-width context menu truncated its label"
            );
            app.cancel_dialog();
        }
        for index in [1, 2] {
            let r = entry(&mut app, index);
            app.handle(Input::Click {
                x: r.x,
                y: r.y,
                double: false,
                toggle: true,
            });
        }
        let r = entry(&mut app, 1);
        right(&mut app, r.x, r.y);
        assert_eq!(actions(&app), [Action::Copy, Action::Cut, Action::Trash]);
        render(&mut app);
        let index = actions(&app)
            .iter()
            .position(|a| *a == Action::Copy)
            .unwrap();
        let r = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::Command(i) if *i == index).then_some(*r))
            .unwrap();
        app.handle(Input::Click {
            x: r.x,
            y: r.y,
            double: false,
            toggle: false,
        });
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths,
            [root.path().join("alpha.txt"), root.path().join("beta.txt")]
        );
        app.tab_mut().marked.clear();
    }
}

#[test]
fn background_menu_uses_the_directory_even_when_files_are_marked() {
    let (root, mut app) = setup();
    app.tab_mut().marked.insert(root.path().join("alpha.txt"));
    app.tab_mut().marked.insert(root.path().join("beta.txt"));
    let p = background(&mut app);
    right(&mut app, p.x, p.y);
    let items = actions(&app);
    for a in [
        Action::NewFile,
        Action::NewFolder,
        Action::Sort,
        Action::Refresh,
        Action::Hidden,
    ] {
        assert!(items.contains(&a));
    }
    for a in [
        Action::Open,
        Action::Copy,
        Action::Cut,
        Action::Rename,
        Action::Trash,
        Action::Paste,
        Action::Quit,
    ] {
        assert!(!items.contains(&a));
    }
    assert!(items.len() <= 9);
    assert_eq!(app.tab().marked.len(), 2);
    app.cancel_dialog();
    app.clipboard = Some(Clipboard {
        paths: vec![root.path().join("alpha.txt")],
        cut: false,
    });
    let p = background(&mut app);
    right(&mut app, p.x, p.y);
    assert!(actions(&app).contains(&Action::Paste));
}

#[test]
fn hovering_disabled_commands_changes_only_that_row_and_background_does_not_flash() {
    let (_root, mut app) = setup();
    for (theme, borders) in [Theme::MoltenZharr, Theme::Light]
        .into_iter()
        .flat_map(|theme| BorderWeight::ALL.map(move |borders| (theme, borders)))
    {
        app.settings.appearance.theme = theme;
        app.settings.appearance.borders = borders;
        app.action(Action::Commands);
        app.handle(Input::Pointer(None));
        let baseline = render(&mut app);
        let index = app.command_items().iter().position(|i| !i.enabled).unwrap();
        let row = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::Command(i) if *i == index).then_some(*r))
            .expect("Disabled command needs its own hover row");
        let selected = app.tab().selected;
        app.handle(Input::Pointer(Some((row.x, row.y).into())));
        let hovered = render(&mut app);
        assert_eq!(app.hovered_area(), Some(row));
        assert_ne!(baseline, hovered);
        for y in 0..44 {
            for x in 0..150 {
                if !row.contains(Position::new(x, y)) {
                    assert_eq!(
                        baseline[(x, y)],
                        hovered[(x, y)],
                        "Changed cell outside disabled row: {x},{y}"
                    );
                }
            }
        }
        app.handle(Input::Click {
            x: row.x,
            y: row.y,
            double: false,
            toggle: false,
        });
        assert!(matches!(app.dialog, Some(Dialog::Commands(_))));
        assert!(!app.busy);
        assert_eq!(app.tab().selected, selected);
        let area = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::MenuBackground).then_some(*r))
            .unwrap();
        app.handle(Input::Pointer(Some((area.x, area.y).into())));
        assert_eq!(render(&mut app), baseline);
        assert_eq!(app.hovered_area(), None);
        app.handle(Input::Pointer(Some((149, 0).into())));
        assert_eq!(render(&mut app), baseline);
        app.cancel_dialog();
    }
}

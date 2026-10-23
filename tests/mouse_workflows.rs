use basalgrim::{
    app::{App, Dialog, Target},
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
    let until = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < until);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn menu_resize_keeps_the_enter_target_visible_and_has_a_mouse_close_button() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    menu(&mut app, 150, 44);
    app.handle(Input::Key(KeyPress::plain(Key::End)));
    render(&mut app, 150, 44);
    render(&mut app, 34, 14);
    let quit = app
        .command_items()
        .iter()
        .position(|i| i.action == Action::Quit)
        .unwrap();
    assert!(
        app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::Command(i) if *i == quit))
    );
    let r = hit(&app, |t| matches!(t, Target::Cancel));
    click(&mut app, r);
    assert!(app.dialog.is_none());
    assert!(!app.quit);
    menu(&mut app, 34, 14);
    app.handle(Input::SecondaryClick { x: 10, y: 5 });
    assert!(app.dialog.is_none());
    menu(&mut app, 25, 11);
    command(&mut app, Action::Settings, 25, 11);
    render(&mut app, 25, 11);
    let r = hit(&app, |t| matches!(t, Target::Cancel));
    click(&mut app, r);
    assert!(app.dialog.is_none());
}
fn render(app: &mut App, width: u16, height: u16) {
    Terminal::new(TestBackend::new(width, height))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| predicate(t).then_some(*r))
        .expect("missing mouse target")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn command(app: &mut App, action: Action, width: u16, height: u16) {
    let index = app
        .command_items()
        .iter()
        .position(|i| i.action == action && i.enabled)
        .unwrap();
    loop {
        render(app, width, height);
        if let Some(r) = app
            .hits
            .iter()
            .find_map(|(r, t)| matches!(t, Target::Command(i) if *i == index).then_some(*r))
        {
            click(app, r);
            return;
        }
        let r = hit(app, |t| matches!(t, Target::CommandScroll(n) if *n > 0));
        click(app, r);
    }
}
fn menu(app: &mut App, width: u16, height: u16) {
    render(app, width, height);
    let r = hit(app, |t| matches!(t, Target::Action(Action::Commands)));
    click(app, r);
}

#[test]
fn compact_mouse_menu_creates_copies_and_moves_without_keyboard_commands() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("destination")).unwrap();
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    for (width, height) in [(34, 14), (70, 26), (150, 44)] {
        menu(&mut app, width, height);
        command(&mut app, Action::NewFile, width, height);
        let name = format!("new-{width}.txt");
        app.handle(Input::Text(name.clone()));
        render(&mut app, width, height);
        let r = hit(&app, |t| matches!(t, Target::Confirm));
        click(&mut app, r);
        settle(&mut app);
        assert!(root.path().join(&name).exists());
        let index = app
            .tab()
            .visible
            .iter()
            .position(|&i| app.tab().entries[i].name == name)
            .unwrap();
        app.tab_mut().selected = index;
        render(&mut app, width, height);
        let r = hit(
            &app,
            |t| matches!(t, Target::Entry { index: i, .. } if *i == index),
        );
        app.handle(Input::SecondaryClick { x: r.x, y: r.y });
        command(&mut app, Action::Copy, width, height);
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths,
            [root.path().join(&name)]
        );
        // Open the destination by double click, then paste from its background menu.
        app.tab_mut().selected = 0;
        render(&mut app, width, height);
        let r = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
        app.handle(Input::Click {
            x: r.x,
            y: r.y,
            double: true,
            toggle: false,
        });
        settle(&mut app);
        menu(&mut app, width, height);
        command(&mut app, Action::Paste, width, height);
        settle(&mut app);
        assert!(root.path().join("destination").join(&name).exists());
        menu(&mut app, width, height);
        command(&mut app, Action::Cut, width, height);
        menu(&mut app, width, height);
        command(&mut app, Action::Parent, width, height);
        settle(&mut app);
        // Rename the original to make the move destination free.
        fs::rename(
            root.path().join(&name),
            root.path().join(format!("original-{width}")),
        )
        .unwrap();
        menu(&mut app, width, height);
        command(&mut app, Action::Paste, width, height);
        settle(&mut app);
        assert!(!root.path().join("destination").join(&name).exists());
        assert!(root.path().join(&name).exists());
    }
}

#[test]
fn right_click_preserves_marked_group_and_dismissal_cannot_activate_underlying_controls() {
    let root = tempfile::tempdir().unwrap();
    for n in ["a", "b", "c"] {
        fs::write(root.path().join(n), n).unwrap();
    }
    let mut app = App::new(root.path().into(), Settings::default());
    settle(&mut app);
    for grid in [true, false] {
        app.settings.grid = grid;
        app.tab_mut().marked.clear();
        for index in [0, 1] {
            render(&mut app, 100, 30);
            let r = hit(
                &app,
                |t| matches!(t, Target::EntryMark { index: i, .. } if *i == index),
            );
            click(&mut app, r);
        }
        assert_eq!(app.tab().marked.len(), 2);
        render(&mut app, 100, 30);
        let r = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
        app.handle(Input::SecondaryClick { x: r.x, y: r.y });
        command(&mut app, Action::Copy, 100, 30);
        assert_eq!(app.clipboard.as_ref().unwrap().paths.len(), 2);
        render(&mut app, 100, 30);
        let r = hit(&app, |t| matches!(t, Target::Entry { index: 2, .. }));
        app.handle(Input::SecondaryClick { x: r.x, y: r.y });
        assert!(app.tab().marked.is_empty());
        render(&mut app, 100, 30);
        let selected = app.tab().selected;
        app.handle(Input::Pointer(Some((0, 0).into())));
        assert_eq!(app.tab().selected, selected);
        click(&mut app, Rect::new(0, 0, 1, 1));
        assert!(app.dialog.is_none());
        assert_eq!(app.tab().selected, selected);
        menu(&mut app, 100, 30);
        command(&mut app, Action::Rename, 100, 30);
        render(&mut app, 100, 30);
        app.handle(Input::SecondaryClick { x: r.x, y: r.y });
        assert!(matches!(app.dialog, Some(Dialog::Edit { .. })));
        app.cancel_dialog();
    }
}

use basalgrim::{
    app::{App, Target},
    fs::Operation,
    input::{Action, Input, KeyPress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(6);
    while app.loading() || app.journal_loading {
        app.tick();
        assert!(Instant::now() < end, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn app(files: &Path, storage: &Path) -> App {
    let mut app = App::new(
        files.into(),
        Settings {
            preview: false,
            ..Default::default()
        },
    );
    app.attach_journal(storage.into());
    settle(&mut app);
    app
}
fn render(app: &mut App, width: u16, height: u16) {
    Terminal::new(TestBackend::new(width, height))
        .unwrap()
        .draw(|f| ui::render(app, f))
        .unwrap();
}
fn click(app: &mut App, action: Action, width: u16, height: u16) {
    render(app, width, height);
    let area = app
        .hits
        .iter()
        .find_map(|(r, t)| matches!(t,Target::Action(a) if *a==action).then_some(*r))
        .unwrap_or_else(|| panic!("missing target: {:?}", app.notice));
    app.handle(Input::Click {
        x: area.x,
        y: area.y,
        double: false,
        toggle: false,
    });
    settle(app);
}
#[test]
fn keyboard_undo_and_redo_after_restart_preserve_clipboard() {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let storage = root.path().join("journal");
    let mut first = app(&files, &storage);
    first.start_operation(Operation::Create {
        parent: files.clone(),
        name: "a".into(),
        directory: false,
    });
    settle(&mut first);
    first.action(Action::Copy);
    let clipboard = first.clipboard.as_ref().unwrap().paths.clone();
    first.handle(Input::Key(KeyPress::ctrl('z')));
    settle(&mut first);
    assert!(!files.join("a").exists());
    assert_eq!(first.clipboard.as_ref().unwrap().paths, clipboard);
    drop(first);
    let mut reopened = app(&files, &storage);
    reopened.handle(Input::Key(KeyPress::ctrl('y')));
    settle(&mut reopened);
    assert!(files.join("a").exists());
    assert!(
        reopened
            .operations
            .front()
            .unwrap()
            .journal
            .as_ref()
            .unwrap()
            .history
            .undo
            > 0
    );
}
#[test]
fn mouse_undo_redo_in_minimum_history_and_global_actions_menu() {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let mut app = app(&files, &root.path().join("journal"));
    app.start_operation(Operation::Create {
        parent: files.clone(),
        name: "folder".into(),
        directory: true,
    });
    settle(&mut app);
    app.action(Action::Operations);
    settle(&mut app);
    click(&mut app, Action::Undo, 34, 14);
    assert!(!files.join("folder").exists());
    click(&mut app, Action::Redo, 34, 14);
    assert!(files.join("folder").exists());
    app.cancel_dialog();
    app.action(Action::Commands);
    assert!(
        app.command_items()
            .iter()
            .any(|item| item.action == Action::Undo && item.enabled)
    );
    let index = app
        .command_items()
        .iter()
        .position(|item| item.action == Action::Undo)
        .unwrap();
    // Keyboard chooses the same shared menu item; full mouse access is exposed
    // through the history footer at the minimum geometry above.
    app.cancel_dialog();
    app.action(Action::Undo);
    settle(&mut app);
    assert!(index > 0);
    assert!(!files.join("folder").exists());
}
#[test]
fn old_custom_bindings_survive_adding_undo_redo() {
    let mut settings = Settings::default();
    settings
        .bindings
        .retain(|b| !matches!(b.action, Action::Undo | Action::Redo));
    let binding = settings
        .bindings
        .iter_mut()
        .find(|b| b.action == Action::Copy)
        .unwrap();
    binding.key = KeyPress::ctrl('z');
    settings.normalize().unwrap();
    assert_eq!(settings.action_for(KeyPress::ctrl('z')), Some(Action::Copy));
    assert_ne!(settings.shortcut(Action::Undo), "Ctrl+Z");
    assert!(!settings.shortcut(Action::Redo).is_empty());
}

#[test]
fn recent_replay_can_address_an_original_outside_the_twenty_visible_rows() {
    let root = tempfile::tempdir().unwrap();
    let files = root.path().join("files");
    fs::create_dir(&files).unwrap();
    let storage = root.path().join("journal");
    let mut first = app(&files, &storage);
    first.start_operation(Operation::Create {
        parent: files.clone(),
        name: "a".into(),
        directory: false,
    });
    settle(&mut first);
    for _ in 0..12 {
        first.action(Action::Undo);
        settle(&mut first);
        first.action(Action::Redo);
        settle(&mut first);
    }
    first.action(Action::Undo);
    settle(&mut first);
    drop(first);
    let mut reopened = app(&files, &storage);
    assert_eq!(reopened.operations.len(), 20);
    reopened.action(Action::Redo);
    settle(&mut reopened);
    assert!(files.join("a").exists());
}

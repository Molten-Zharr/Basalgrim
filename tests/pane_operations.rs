use basalgrim::{
    app::{App, Clipboard, Dialog, MenuContext, Target},
    conflicts::Choice,
    fs::{self, Operation},
    input::{Action, Input, Key, KeyPress},
    operations::{Cancellation, Phase},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs as disk,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn wait(app: &mut App, predicate: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if predicate(app) {
            return;
        }
        assert!(Instant::now() < deadline, "operation timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn settle(app: &mut App) {
    wait(app, |a| !a.loading());
}
fn fixture(reverse: bool) -> (tempfile::TempDir, App, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let dest = root.path().join("destination");
    disk::create_dir(&source).unwrap();
    disk::create_dir(&dest).unwrap();
    for name in ["a.txt", "b.txt", "untouched"] {
        disk::write(source.join(name), format!("incoming {name}")).unwrap();
    }
    let mut app = App::new(
        source.clone(),
        Settings {
            dual: true,
            grid: false,
            preview: false,
            ..Settings::default()
        },
    );
    settle(&mut app);
    app.active = usize::from(!reverse);
    app.navigate(dest.clone(), true);
    settle(&mut app);
    app.active = usize::from(reverse);
    mark_pair(&mut app);
    app.clipboard = Some(Clipboard {
        paths: vec![source.join("untouched")],
        cut: true,
    });
    (root, app, source, dest)
}
fn mark_pair(app: &mut App) {
    app.tab_mut().marked = ["a.txt", "b.txt"]
        .map(|name| app.tab().cwd.join(name))
        .into();
    app.tab_mut().explicit_selection = true;
}
fn shortcut(app: &mut App, action: Action) {
    let key = app
        .settings
        .bindings
        .iter()
        .find(|b| b.action == action)
        .unwrap()
        .key;
    app.handle(Input::Key(key));
}
fn render(app: &mut App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| ui::render(app, f)).unwrap();
}

#[test]
fn shortcuts_transfer_only_the_marked_group_in_both_directions_without_replacing_clipboard() {
    for reverse in [false, true] {
        for action in [Action::CopyToOtherPane, Action::MoveToOtherPane] {
            let (_root, mut app, source, dest) = fixture(reverse);
            shortcut(&mut app, action);
            assert_eq!(app.active, usize::from(reverse));
            settle(&mut app);
            for name in ["a.txt", "b.txt"] {
                assert_eq!(
                    disk::read_to_string(dest.join(name)).unwrap(),
                    format!("incoming {name}")
                );
                assert_eq!(
                    source.join(name).exists(),
                    action == Action::CopyToOtherPane
                );
            }
            assert!(!dest.join("untouched").exists());
            assert_eq!(
                app.clipboard.as_ref().unwrap().paths,
                [source.join("untouched")]
            );
            assert!(app.clipboard.as_ref().unwrap().cut);
            assert_eq!(app.operations[0].completed_paths.len(), 2);
            assert!(app.operations[0].error.is_none());
        }
    }
}

#[test]
fn contextual_mouse_commands_work_for_groups_in_minimum_and_normal_windows() {
    for (width, height) in [(34, 14), (150, 44)] {
        for action in [
            Action::CopyToOtherPane,
            Action::MoveToOtherPane,
            Action::Duplicate,
        ] {
            let (_root, mut app, source, dest) = fixture(false);
            render(&mut app, width, height);
            let entry = app
                .hits
                .iter()
                .find_map(|(r, t)| {
                    if let Target::Entry { pane, index } = t {
                        let tab = app.panes[*pane].tab();
                        (*pane == 0 && tab.marked.contains(&tab.entries[tab.visible[*index]].path))
                            .then_some(*r)
                    } else {
                        None
                    }
                })
                .unwrap();
            app.handle(Input::SecondaryClick {
                x: entry.x,
                y: entry.y,
            });
            assert!(
                matches!(&app.dialog, Some(Dialog::Commands(m)) if m.context == MenuContext::Entries)
            );
            let index = app
                .command_items()
                .iter()
                .position(|item| item.action == action && item.enabled)
                .unwrap();
            render(&mut app, width, height);
            let row = app
                .hits
                .iter()
                .find_map(|(r, t)| matches!(t, Target::Command(i) if *i == index).then_some(*r))
                .unwrap();
            app.handle(Input::PointerDown {
                x: row.x,
                y: row.y,
                double: false,
                toggle: false,
            });
            app.handle(Input::PointerUp { x: row.x, y: row.y });
            settle(&mut app);
            assert_eq!(app.operation_count, 1, "release must not click through");
            for stem in ["a", "b"] {
                let target = if action == Action::Duplicate {
                    source.join(format!("{stem} (1).txt"))
                } else {
                    dest.join(format!("{stem}.txt"))
                };
                assert_eq!(
                    disk::read_to_string(target).unwrap(),
                    format!("incoming {stem}.txt")
                );
            }
            assert_eq!(
                app.clipboard.as_ref().unwrap().paths,
                [source.join("untouched")]
            );
        }
    }
}

#[test]
fn destination_and_sources_are_captured_before_pane_or_tab_navigation() {
    let (root, mut app, source, dest) = fixture(false);
    let hidden = root.path().join("hidden-tab");
    disk::create_dir(&hidden).unwrap();
    app.active = 1;
    app.action(Action::NewTab);
    app.navigate(hidden.clone(), true);
    settle(&mut app);
    app.panes[1].active = 0;
    app.active = 0;
    shortcut(&mut app, Action::CopyToOtherPane);
    app.active = 1;
    app.panes[1].active = 1;
    app.active = 0;
    app.navigate(hidden.clone(), true);
    settle(&mut app);
    assert!(dest.join("a.txt").exists() && dest.join("b.txt").exists());
    assert_eq!(disk::read_dir(&hidden).unwrap().count(), 0);
    assert!(source.join("a.txt").exists());
    assert_eq!(
        app.operations[0].destination.as_deref(),
        Some(dest.as_path())
    );
}

#[test]
fn unavailable_other_pane_and_explicit_empty_selection_never_start_operations() {
    let (_root, mut app, _source, _dest) = fixture(false);
    for state in 0..4 {
        app.settings.dual = state != 0;
        app.panes[1].tab_mut().loading = state == 1;
        app.panes[1].tab_mut().error = (state == 2).then(|| "unavailable".into());
        if state == 3 {
            app.tab_mut().marked.clear();
        }
        assert!(
            app.command_items()
                .iter()
                .filter(|i| matches!(i.action, Action::CopyToOtherPane | Action::MoveToOtherPane))
                .all(|i| !i.enabled)
        );
        for action in [Action::CopyToOtherPane, Action::MoveToOtherPane] {
            shortcut(&mut app, action);
            assert!(!app.busy);
        }
    }
    shortcut(&mut app, Action::Duplicate);
    assert_eq!(app.operation_count, 0);
    assert!(app.operations.is_empty());
}

#[test]
fn direct_moves_share_conflicts_and_prune_only_successful_cut_paths() {
    let (_root, mut app, source, dest) = fixture(false);
    for name in ["a.txt", "b.txt"] {
        disk::write(dest.join(name), "existing").unwrap();
    }
    app.clipboard
        .as_mut()
        .unwrap()
        .paths
        .extend([source.join("a.txt"), source.join("b.txt")]);
    shortcut(&mut app, Action::MoveToOtherPane);
    wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
    app.toggle_conflict_remaining();
    app.choose_conflict(Choice::Skip);
    settle(&mut app);
    assert_eq!(app.operations[0].skipped_paths.len(), 2);
    assert_eq!(app.clipboard.as_ref().unwrap().paths.len(), 3);
    for name in ["a.txt", "b.txt"] {
        assert_eq!(disk::read_to_string(dest.join(name)).unwrap(), "existing");
    }
    mark_pair(&mut app);
    shortcut(&mut app, Action::MoveToOtherPane);
    wait(&mut app, |a| matches!(a.dialog, Some(Dialog::Conflict(_))));
    app.toggle_conflict_remaining();
    app.choose_conflict(Choice::Replace);
    settle(&mut app);
    assert_eq!(app.operations[0].completed_paths.len(), 2);
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        [source.join("untouched")]
    );
    for name in ["a.txt", "b.txt"] {
        assert!(!source.join(name).exists());
    }
}

#[test]
fn old_custom_bindings_survive_all_three_new_default_key_collisions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut old = Settings::default();
    let actions = [
        Action::CopyToOtherPane,
        Action::MoveToOtherPane,
        Action::Duplicate,
    ];
    let keys: Vec<_> = actions
        .iter()
        .map(|a| old.bindings.iter().find(|b| b.action == *a).unwrap().key)
        .collect();
    old.bindings.retain(|b| !actions.contains(&b.action));
    for (action, key) in [Action::Help, Action::Quit, Action::Copy]
        .into_iter()
        .zip(keys)
    {
        old.bindings
            .iter_mut()
            .find(|b| b.action == action)
            .unwrap()
            .key = key;
    }
    let original = serde_json::to_vec(&old).unwrap();
    disk::write(&path, &original).unwrap();
    let migrated = Settings::load(&path).unwrap();
    migrated.validate_bindings().unwrap();
    for binding in &old.bindings {
        assert!(migrated.bindings.contains(binding));
    }
    for action in actions {
        assert!(migrated.bindings.iter().any(|b| b.action == action));
    }
    assert_eq!(disk::read(&path).unwrap(), original);
    migrated.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap().bindings, migrated.bindings);
    assert_eq!(
        Settings::default().action_for(KeyPress::plain(Key::F(5))),
        Some(Action::Refresh)
    );
    assert_eq!(
        Settings::default().action_for(KeyPress::plain(Key::F(6))),
        Some(Action::Operations)
    );
}

fn duplicate(source: &Path) -> Operation {
    Operation::Duplicate {
        sources: vec![source.into()],
        destination: source.parent().unwrap().into(),
    }
}

#[test]
fn duplicate_never_prompts_and_uses_free_names_for_files_folders_and_links() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("data.txt");
    disk::write(&file, "original").unwrap();
    disk::write(root.path().join("data (1).txt"), "occupied").unwrap();
    let folder = root.path().join("folder.v1");
    disk::create_dir(&folder).unwrap();
    disk::write(folder.join("child"), "nested").unwrap();
    for source in [&file, &folder] {
        let report = fs::operate_with_conflicts(
            duplicate(source),
            &Cancellation::default(),
            &mut |_| {},
            &mut |_| panic!("duplicate must choose a free name automatically"),
        );
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(report.completed.as_slice(), std::slice::from_ref(source));
        assert_eq!(report.progress.fraction(), 1.0);
    }
    assert_eq!(
        disk::read_to_string(root.path().join("data (2).txt")).unwrap(),
        "original"
    );
    assert_eq!(
        disk::read_to_string(root.path().join("data (1).txt")).unwrap(),
        "occupied"
    );
    assert_eq!(
        disk::read_to_string(root.path().join("folder.v1 (1)/child")).unwrap(),
        "nested"
    );
    #[cfg(unix)]
    {
        let link = root.path().join("dangling");
        std::os::unix::fs::symlink("missing", &link).unwrap();
        let report = fs::operate(duplicate(&link));
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(
            disk::read_link(root.path().join("dangling (1)")).unwrap(),
            Path::new("missing")
        );
    }
}

#[test]
fn duplicate_retries_a_raced_name_and_cleans_a_cancelled_staging_copy() {
    for cancel_copy in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("data.txt");
        disk::File::create(&file)
            .unwrap()
            .set_len(4 * 1024 * 1024)
            .unwrap();
        let cancellation = Cancellation::default();
        let mut raced = false;
        let report = fs::operate_with_control(duplicate(&file), &cancellation, &mut |p| {
            if !raced && p.phase == Phase::Running && p.bytes_done > 0 {
                raced = true;
                disk::write(root.path().join("data (1).txt"), "new occupant").unwrap();
                if cancel_copy {
                    cancellation.cancel();
                }
            }
        });
        assert!(raced);
        assert!(report.error.is_none(), "{report:?}");
        assert_eq!(report.cancelled, cancel_copy);
        assert_eq!(report.completed.len(), usize::from(!cancel_copy));
        assert_eq!(
            disk::read_to_string(root.path().join("data (1).txt")).unwrap(),
            "new occupant"
        );
        assert_eq!(file.metadata().unwrap().len(), 4 * 1024 * 1024);
        assert_eq!(root.path().join("data (2).txt").exists(), !cancel_copy);
        assert!(disk::read_dir(root.path()).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".basalgrim-")
        }));
    }
}

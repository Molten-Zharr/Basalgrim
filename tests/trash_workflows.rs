use basalgrim::{
    app::{App, Clipboard, Dialog, Target},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    trash::Root,
    ui,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(6);
    while app.loading() {
        assert!(Instant::now() < end, "{}", app.notice);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::render(app, f)).unwrap();
    term.backend().buffer().clone()
}
fn text(b: &Buffer) -> String {
    b.content.iter().map(|c| c.symbol()).collect()
}
fn hit(app: &App, predicate: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .rev()
        .find_map(|(r, t)| predicate(t).then_some(*r))
        .expect("missing trash mouse control")
}
fn click(app: &mut App, r: Rect, toggle: bool) {
    app.handle(Input::PointerDown {
        x: r.x,
        y: r.y,
        double: false,
        toggle,
    });
    app.handle(Input::PointerUp { x: r.x, y: r.y });
}
fn setup(count: usize) -> (tempfile::TempDir, Root, App) {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let trash = Root::home(data);
    for dir in [
        &trash.directory,
        &trash.directory.join("files"),
        &trash.directory.join("info"),
    ] {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for i in 0..count {
        add(&trash, root.path(), i);
    }
    let mut app = App::new(
        root.path().into(),
        Settings {
            preview: false,
            grid: false,
            ..Settings::default()
        },
    );
    settle(&mut app);
    app.open_trash(Some(vec![trash.clone()]));
    settle(&mut app);
    (root, trash, app)
}
fn add(trash: &Root, original: &std::path::Path, i: usize) {
    fs::write(
        trash.directory.join("files").join(format!("stored-{i:03}")),
        format!("content-{i}"),
    )
    .unwrap();
    fs::write(
        trash
            .directory
            .join("info")
            .join(format!("stored-{i:03}.trashinfo")),
        format!(
            "[Trash Info]\nPath={}/file-{i:03}.txt\nDeletionDate=2026-11-01T12:30:45\n",
            original.display()
        ),
    )
    .unwrap();
}

#[test]
fn shift_ranges_use_the_regular_pane_and_capture_exact_restore_and_delete_targets() {
    for grid in [false, true] {
        let (_root, _trash, mut app) = setup(7);
        app.settings.grid = grid;
        render(&mut app, 170, 53);
        for (index, shift) in [(1, false), (4, true), (2, true)] {
            let r = hit(
                &app,
                |t| matches!(t, Target::Entry {index:i,..} if *i==index),
            );
            app.handle(Input::SelectionModifier(shift));
            click(
                &mut app,
                Rect::new(r.x + 5.min(r.width - 1), r.y + 2.min(r.height - 1), 1, 1),
                false,
            );
        }
        app.handle(Input::SelectionModifier(false));
        let view = app.trash.as_ref().unwrap();
        assert_eq!(
            view.targets()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["file-001.txt", "file-002.txt"]
        );
        app.action(Action::RestoreTrash);
        assert!(
            matches!(app.dialog, Some(Dialog::RestoreTrash {ref entries,..}) if entries.len()==2)
        );
        app.cancel_dialog();
        render(&mut app, 34, 14);
        app.handle(Input::Key(KeyPress {
            shift: true,
            ..KeyPress::plain(Key::End)
        }));
        assert_eq!(app.tab().selection_count(), 6);
        assert_eq!(app.trash.as_ref().unwrap().deletion_targets().len(), 6);
        app.action(Action::DeleteTrash);
        assert!(matches!(app.dialog, Some(Dialog::DeleteTrash { .. })));
        app.cancel_dialog(); // Selection only: no permanent deletion or host trash access.
    }
}

#[test]
fn mouse_group_restore_works_at_minimum_size_without_touching_file_clipboard_or_panes() {
    for (w, h) in [(34, 14), (150, 44)] {
        let (root, trash, mut app) = setup(2);
        app.clipboard = Some(Clipboard {
            paths: vec![root.path().join("keep")],
            cut: true,
        });
        render(&mut app, w, h);
        for index in 0..2 {
            let r = hit(
                &app,
                |t| matches!(t,Target::EntryMark {index:i,..} if *i==index),
            );
            click(&mut app, r, false);
        }
        assert_eq!(app.trash.as_ref().unwrap().marked.len(), 2);
        let r = hit(&app, |t| matches!(t, Target::Action(Action::RestoreTrash)));
        click(&mut app, r, false);
        assert!(
            matches!(app.dialog,Some(Dialog::RestoreTrash {ref entries,..}) if entries.len()==2)
        );
        render(&mut app, w, h);
        assert!(
            !app.hits
                .iter()
                .any(|(_, t)| matches!(t, Target::Entry { .. }))
        );
        let r = hit(&app, |t| matches!(t, Target::Confirm));
        click(&mut app, r, false);
        settle(&mut app);
        for i in 0..2 {
            assert_eq!(
                fs::read_to_string(root.path().join(format!("file-{i:03}.txt"))).unwrap(),
                format!("content-{i}")
            );
        }
        assert!(
            fs::read_dir(trash.directory.join("files"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(app.clipboard.as_ref().unwrap().cut);
        assert_eq!(
            app.clipboard.as_ref().unwrap().paths,
            [root.path().join("keep")]
        );
        assert_eq!(app.tab().cwd, root.path());
        assert!(app.trash.as_ref().unwrap().catalog.entries.is_empty());
        render(&mut app, w, h);
        let r = hit(&app, |t| matches!(t, Target::Action(Action::Back)));
        click(&mut app, r, false);
        assert!(app.trash.is_none());
    }
}

#[test]
fn wheel_hover_and_background_refresh_preserve_focus_marks_and_viewport() {
    let (root, trash, mut app) = setup(80);
    render(&mut app, 70, 24);
    let row = hit(&app, |t| matches!(t, Target::Entry { index: 1, .. }));
    click(&mut app, row, true);
    let selected = app.trash.as_ref().unwrap().entry().unwrap().data.clone();
    app.handle(Input::Pointer(Some((row.x, row.y).into())));
    render(&mut app, 70, 24);
    let area = app.file_areas[app.active];
    app.handle(Input::Scroll {
        x: area.x + 4,
        y: area.y + 1,
        delta: 2,
    });
    render(&mut app, 70, 24);
    assert_eq!(app.trash.as_ref().unwrap().entry().unwrap().data, selected);
    assert_eq!(app.trash.as_ref().unwrap().marked.len(), 1);
    let offset = app.trash.as_ref().unwrap().offset;
    assert!(offset > 0);
    add(&trash, root.path(), 100);
    app.refresh_trash();
    settle(&mut app);
    render(&mut app, 70, 24);
    assert_eq!(app.trash.as_ref().unwrap().offset, offset);
    assert_eq!(app.trash.as_ref().unwrap().entry().unwrap().data, selected);
    app.handle(Input::Key(KeyPress::plain(Key::Down)));
    render(&mut app, 70, 24);
    let view = app.trash.as_ref().unwrap();
    assert!(view.selected >= view.offset && view.selected < view.offset + view.page_size);
}

#[test]
fn context_menu_is_compact_preserves_group_and_dismisses_without_click_through() {
    let (_temp, _, mut app) = setup(3);
    render(&mut app, 150, 44);
    for index in 0..2 {
        let r = hit(
            &app,
            |t| matches!(t,Target::Entry {index:i,..} if *i==index),
        );
        click(&mut app, r, true);
    }
    let r = hit(&app, |t| matches!(t, Target::Entry { index: 0, .. }));
    app.handle(Input::SecondaryClick { x: r.x + 5, y: r.y });
    assert_eq!(app.trash.as_ref().unwrap().marked.len(), 2);
    assert_eq!(
        app.command_items()
            .iter()
            .map(|i| i.action)
            .collect::<Vec<_>>(),
        [
            Action::RestoreTrash,
            Action::RestoreTrashHere,
            Action::DeleteTrash
        ]
    );
    render(&mut app, 150, 44);
    let outside = hit(&app, |t| matches!(t, Target::DismissMenu));
    app.handle(Input::Click {
        x: outside.right() - 1,
        y: outside.bottom() - 1,
        double: false,
        toggle: false,
    });
    assert!(app.dialog.is_none());
    assert!(!app.busy);
    assert_eq!(app.trash.as_ref().unwrap().marked.len(), 2);
}

#[test]
fn searching_and_explicit_empty_selection_do_not_restore_the_focused_item() {
    let (_temp, _, mut app) = setup(5);
    app.action(Action::Search);
    app.handle(Input::Text("file-003".into()));
    assert_eq!(app.trash.as_ref().unwrap().visible.len(), 1);
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert_eq!(app.trash.as_ref().unwrap().visible.len(), 5);
    app.action(Action::Mark);
    app.action(Action::Mark);
    assert!(app.trash.as_ref().unwrap().explicit_selection);
    app.action(Action::RestoreTrash);
    assert!(app.dialog.is_none());
    assert!(!app.busy);
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    app.action(Action::RestoreTrash);
    assert!(matches!(app.dialog, Some(Dialog::RestoreTrash { .. })));
}

#[test]
fn broken_metadata_disables_restore_and_shows_the_reason() {
    let (_temp, trash, mut app) = setup(1);
    fs::remove_file(trash.directory.join("info/stored-000.trashinfo")).unwrap();
    app.refresh_trash();
    settle(&mut app);
    let buffer = render(&mut app, 70, 24);
    assert!(text(&buffer).contains("Нет .trashinfo"));
    let r = hit(&app, |t| matches!(t, Target::Action(Action::RestoreTrash)));
    click(&mut app, r, false);
    assert!(app.dialog.is_none());
    assert!(!app.busy);
}

#[test]
fn restore_to_current_directory_captures_the_destination_and_resolves_conflicts() {
    let (root, trash, mut app) = setup(1);
    let dest = root.path().join("destination");
    fs::create_dir(&dest).unwrap();
    app.close_trash();
    app.navigate(dest.clone(), true);
    settle(&mut app);
    app.open_trash(Some(vec![trash]));
    settle(&mut app);
    fs::write(dest.join("file-000.txt"), "existing").unwrap();
    app.action(Action::RestoreTrashHere);
    assert!(
        matches!(app.dialog,Some(Dialog::RestoreTrash {destination:Some(ref p),..}) if *p==dest)
    );
    app.confirm_dialog();
    let end = Instant::now() + Duration::from_secs(5);
    while !matches!(app.dialog, Some(Dialog::Conflict(_))) {
        assert!(Instant::now() < end);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
    app.handle(Input::Key(KeyPress::plain(Key::Enter)));
    settle(&mut app);
    assert_eq!(
        fs::read_to_string(dest.join("file-000.txt")).unwrap(),
        "existing"
    );
    assert_eq!(
        fs::read_to_string(dest.join("file-000 (1).txt")).unwrap(),
        "content-0"
    );
    assert!(!root.path().join("file-000.txt").exists());
}

#[test]
fn fresh_catalog_generations_reject_stale_mouse_geometry() {
    let (_temp, _, mut app) = setup(3);
    render(&mut app, 150, 44);
    let stale = hit(&app, |t| matches!(t, Target::Entry { index: 2, .. }));
    app.refresh_trash();
    settle(&mut app);
    click(&mut app, stale, false);
    assert_eq!(app.trash.as_ref().unwrap().selected, 0);
}

#[test]
fn trash_keeps_chrome_tabs_other_pane_icons_and_shared_selection() {
    let (root, _, mut app) = setup(6);
    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("ordinary.txt"), "other pane").unwrap();
    app.active = 1;
    app.navigate(other.clone(), true);
    settle(&mut app);
    app.active = 0;
    app.settings.dual = true;
    app.icon_backend = basalgrim::icons::IconBackend::Gui;
    let buffer = render(&mut app, 170, 44);
    let contents = text(&buffer);
    for expected in [
        "BASALGRIM",
        "МЕСТА",
        "Корзина",
        "ВТОРАЯ ПАНЕЛЬ",
        "ordinary.txt",
    ] {
        assert!(
            contents.contains(expected),
            "missing shared chrome: {expected}"
        );
    }
    assert!(!contents.contains("На место"));
    assert!(
        app.hits
            .iter()
            .any(|(_, t)| matches!(t, Target::Tab { pane: 0, .. }))
    );
    assert!(!app.icon_slots.is_empty());
    assert!(
        app.tab()
            .entries
            .iter()
            .all(|entry| entry.modified.is_some())
    );
    let other_file = hit(&app, |t| matches!(t, Target::Entry { pane: 1, index: 0 }));
    click(&mut app, other_file, false);
    app.action(Action::Copy);
    assert_eq!(
        app.clipboard.as_ref().unwrap().paths,
        [other.join("ordinary.txt")]
    );
    assert!(!app.in_trash());
    let trash_file = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    click(&mut app, trash_file, false);
    assert!(app.in_trash());
    app.action(Action::Grid);
    render(&mut app, 170, 44);
    assert!(app.tab().columns > 1);
    let first = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    let second = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 1 }));
    app.handle(Input::PointerDown {
        x: first.x,
        y: first.y + 2,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: second.x + 4,
        y: second.y + 2,
    });
    app.handle(Input::PointerUp {
        x: second.x + 4,
        y: second.y + 2,
    });
    assert_eq!(app.tab().marked.len(), 2);
    app.action(Action::Commands);
    assert!(app.command_items().len() <= 10);
    assert!(
        app.command_items()
            .iter()
            .any(|item| item.action == Action::Palette)
    );
    assert!(
        app.command_items()
            .iter()
            .any(|item| item.action == Action::Operations)
    );
    assert!(app.command_items().iter().all(|item| item.enabled));
    app.cancel_dialog();
    app.action(Action::NewTab);
    settle(&mut app);
    assert!(!app.in_trash());
    app.action(Action::NextTab);
    assert!(app.in_trash());
    app.action(Action::Back);
    assert!(!app.in_trash());
    assert_eq!(app.tab().cwd, root.path());
}

#[test]
fn trash_background_menu_contains_only_directory_view_actions() {
    let (_, _, mut app) = setup(1);
    render(&mut app, 150, 44);
    let area = app.file_areas[0];
    app.handle(Input::SecondaryClick {
        x: area.right() - 1,
        y: area.bottom() - 1,
    });
    let items = app.command_items();
    assert!(items.len() <= 5);
    assert!(items.iter().all(|item| !matches!(
        item.action,
        Action::RestoreTrash | Action::RestoreTrashHere | Action::NewFile | Action::NewFolder
    )));
}

#[test]
fn inactive_trash_pane_restore_button_selects_its_own_sources() {
    let (_root, _, mut app) = setup(1);
    app.settings.dual = true;
    app.active = 1;
    render(&mut app, 170, 44);
    let restore = hit(&app, |t| matches!(t, Target::Action(Action::RestoreTrash)));
    assert!(app.pane_areas[0].contains((restore.x, restore.y).into()));
    click(&mut app, restore, false);
    assert_eq!(app.active, 0);
    assert!(matches!(app.dialog,Some(Dialog::RestoreTrash{ref entries,..}) if entries.len()==1));
}

#[test]
fn delete_selected_and_empty_everything_ignore_filter_and_preserve_normal_folder() {
    for (width, height) in [(34, 14), (150, 44)] {
        let (root, trash, mut app) = setup(3);
        fs::write(root.path().join("outside"), "safe").unwrap();
        render(&mut app, width, height);
        app.action(Action::Trash);
        assert!(
            matches!(app.dialog,Some(Dialog::DeleteTrash{ref targets,empty:false}) if targets.len()==1)
        );
        let confirmation = render(&mut app, width, height);
        assert!(
            !text(&confirmation).contains("stored-000"),
            "confirmation must show the displayed file name"
        );
        app.cancel_dialog();
        assert_eq!(
            fs::read_dir(trash.directory.join("files")).unwrap().count(),
            3
        );
        app.action(Action::DeleteTrash);
        app.confirm_dialog();
        settle(&mut app);
        assert_eq!(app.trash.as_ref().unwrap().catalog.entries.len(), 2);
        app.action(Action::Search);
        app.handle(Input::Text("no matches".into()));
        app.confirm_dialog();
        assert!(app.tab().visible.is_empty());
        fs::write(trash.directory.join("info/orphan.trashinfo"), "broken").unwrap();
        app.action(Action::EmptyTrash);
        settle(&mut app);
        assert!(
            matches!(app.dialog,Some(Dialog::DeleteTrash{ref targets,empty:true}) if targets.len()==3)
        );
        let buffer = render(&mut app, width, height);
        assert!(text(&buffer).contains("Очистить"));
        let confirm = hit(&app, |target| matches!(target, Target::Confirm));
        click(&mut app, confirm, false);
        settle(&mut app);
        assert_eq!(
            fs::read_dir(trash.directory.join("files")).unwrap().count(),
            0
        );
        assert_eq!(
            fs::read_dir(trash.directory.join("info")).unwrap().count(),
            0
        );
        assert_eq!(
            fs::read_to_string(root.path().join("outside")).unwrap(),
            "safe"
        );
        app.close_trash();
        app.action(Action::DeleteTrash);
        app.action(Action::EmptyTrash);
        assert!(app.dialog.is_none() && !app.busy);
    }
}

#[test]
fn delete_group_captures_only_the_marked_files_and_empty_keeps_later_arrivals() {
    let (root, trash, mut app) = setup(3);
    render(&mut app, 150, 44);
    for index in 0..2 {
        let target = hit(
            &app,
            |t| matches!(t,Target::Entry{pane:0,index:i} if *i==index),
        );
        click(&mut app, target, true);
    }
    app.action(Action::DeleteTrash);
    assert!(
        matches!(app.dialog,Some(Dialog::DeleteTrash{ref targets,empty:false}) if targets.len()==2)
    );
    app.confirm_dialog();
    settle(&mut app);
    assert_eq!(app.trash.as_ref().unwrap().catalog.entries.len(), 1);
    app.action(Action::EmptyTrash);
    settle(&mut app);
    assert!(
        matches!(app.dialog,Some(Dialog::DeleteTrash{ref targets,empty:true}) if targets.len()==1)
    );
    add(&trash, root.path(), 99); // Not part of the confirmed snapshot.
    app.confirm_dialog();
    settle(&mut app);
    assert_eq!(app.trash.as_ref().unwrap().catalog.entries.len(), 1);
    assert!(trash.directory.join("files/stored-099").exists());
}

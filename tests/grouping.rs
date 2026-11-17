use basalgrim::{
    app::{App, Dialog, Target},
    columns::Column,
    fs::{Entry, Sort},
    grouping::{Grouping, project},
    input::{Action, Input, Key, KeyPress},
    settings::Settings,
    ui,
    view::ViewMode,
};
use chrono::{TimeZone, Utc};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};
fn date(year: i32, month: u32) -> SystemTime {
    Utc.with_ymd_and_hms(year, month, 15, 12, 0, 0)
        .unwrap()
        .into()
}
fn entry(name: &str, month: Option<SystemTime>) -> Entry {
    Entry {
        path: PathBuf::from("/test").join(name),
        name: name.into(),
        directory: false,
        symlink: false,
        hidden: false,
        size: 0,
        modified: month,
        details: basalgrim::columns::Details {
            created: month,
            ..Default::default()
        },
    }
}
fn names(entries: &[Entry], visible: &[usize]) -> Vec<String> {
    visible.iter().map(|&i| entries[i].name.clone()).collect()
}
#[test]
fn months_are_chronological_across_years_missing_dates_last_in_both_orders() {
    let entries = vec![
        entry("old", Some(date(2025, 11))),
        entry("oct", Some(date(2026, 10))),
        entry("new-b", Some(date(2026, 11))),
        entry("unknown", None),
        entry("new-a", Some(date(2026, 11))),
    ];
    let mut indices = vec![4, 2, 1, 0, 3];
    let mut p = Grouping {
        enabled: true,
        field: Column::Created,
        descending: true,
    };
    let groups = project(&entries, &mut indices, p, None);
    assert_eq!(
        groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
        [
            "Ноябрь 2026",
            "Октябрь 2026",
            "Ноябрь 2025",
            "Дата неизвестна"
        ]
    );
    assert_eq!(
        names(&entries, &indices),
        ["new-a", "new-b", "oct", "old", "unknown"]
    );
    p.descending = false;
    project(&entries, &mut indices, p, None);
    assert_eq!(
        names(&entries, &indices),
        ["old", "oct", "new-a", "new-b", "unknown"]
    );
}
#[test]
fn practical_buckets_use_real_metadata_and_do_not_change_the_files() {
    let mut entries = vec![
        entry("apple.JPG", None),
        entry("alpha.jpg", None),
        entry("9.txt", None),
        entry("NoSuffix", None),
        entry("Folder", None),
    ];
    entries[4].directory = true;
    entries[0].size = 1;
    entries[1].size = 1 << 20;
    entries[2].size = 2 << 30;
    let original = entries.clone();
    let mut visible = (0..entries.len()).collect::<Vec<_>>();
    let p = Grouping {
        enabled: true,
        field: Column::Extension,
        descending: false,
    };
    let groups = project(&entries, &mut visible, p, None);
    assert_eq!(groups.iter().find(|g| g.label == ".jpg").unwrap().count, 2);
    assert!(groups.iter().any(|g| g.label == "Без расширения"));
    assert!(groups.iter().any(|g| g.label == "Папки"));
    let groups = project(
        &entries,
        &mut visible,
        Grouping {
            field: Column::Size,
            ..p
        },
        None,
    );
    assert_eq!(
        groups.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
        [
            "Папки",
            "Пустые файлы",
            "Меньше 1 MiB",
            "1–16 MiB",
            "1 GiB и больше"
        ]
    );
    for field in Column::ALL {
        let groups = project(&entries, &mut visible, Grouping { field, ..p }, None);
        assert_eq!(
            groups.iter().map(|g| g.range.len()).sum::<usize>(),
            entries.len()
        );
        assert_eq!(
            visible.iter().copied().collect::<BTreeSet<_>>(),
            (0..entries.len()).collect()
        );
    }
    assert_eq!(entries, original);
}
fn settle(app: &mut App) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        app.tick();
        if !app.tab().loading && !app.busy {
            return;
        }
        assert!(Instant::now() < until, "{}", app.notice);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(app, f)).unwrap();
    t.backend().buffer().clone()
}
fn hit(app: &App, p: impl Fn(&Target) -> bool) -> Rect {
    app.hits
        .iter()
        .find_map(|(r, t)| p(t).then_some(*r))
        .expect("missing target")
}
fn click(app: &mut App, r: Rect) {
    app.handle(Input::Click {
        x: r.x,
        y: r.y,
        double: false,
        toggle: false,
    });
}
fn key(app: &mut App, k: Key) {
    app.handle(Input::Key(KeyPress::plain(k)));
}
fn setup(mode: ViewMode) -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..39 {
        let f = fs::File::create(tmp.path().join(format!("file-{i:02}.txt"))).unwrap();
        f.set_times(fs::FileTimes::new().set_modified(date(2026 - i / 13, 11)))
            .unwrap();
    }
    let mut app = App::new(
        tmp.path().into(),
        Settings {
            view: Some(mode),
            preview: false,
            ..Default::default()
        },
    );
    settle(&mut app);
    app.set_grouping(Grouping {
        enabled: true,
        field: Column::Modified,
        descending: true,
    });
    (tmp, app)
}
#[test]
fn every_view_reaches_all_files_and_wheel_never_changes_focus_or_marks() {
    for mode in ViewMode::ALL {
        for (w, h) in [(34, 14), (150, 44)] {
            let (_tmp, mut app) = setup(mode);
            render(&mut app, w, h);
            let focus = app.tab().entry().unwrap().path.clone();
            app.tab_mut().marked.insert(focus.clone());
            let marks = app.tab().marked.clone();
            let mut seen = BTreeSet::new();
            for _ in 0..100 {
                render(&mut app, w, h);
                for (_, target) in &app.hits {
                    if let Target::Entry { pane: 0, index } = target {
                        seen.insert(*index);
                    }
                }
                let r = app.file_areas[0];
                app.handle(Input::Scroll {
                    x: r.x,
                    y: r.y,
                    delta: 1,
                });
            }
            assert_eq!(seen.len(), 39, "{mode:?} {w}x{h}: {seen:?}");
            assert_eq!(app.tab().entry().unwrap().path, focus);
            assert_eq!(app.tab().marked, marks);
            key(&mut app, Key::End);
            render(&mut app, w, h);
            assert!(
                app.hits
                    .iter()
                    .any(|(_, t)| matches!(t, Target::Entry { pane: 0, index: 38 })),
                "end {mode:?}"
            );
            key(&mut app, Key::Home);
            render(&mut app, w, h);
            assert!(
                app.hits
                    .iter()
                    .any(|(_, t)| matches!(t, Target::Entry { pane: 0, index: 0 })),
                "home {mode:?}"
            );
        }
    }
}
#[test]
fn grouping_is_independent_of_sort_columns_and_survives_settings_reload() {
    let (tmp, mut app) = setup(ViewMode::List);
    app.tab_mut().selected = 20;
    let path = app.tab().entry().unwrap().path.clone();
    app.tab_mut().marked.insert(path.clone());
    app.settings.list_columns.toggle(Column::Owner);
    let columns = app.settings.list_columns.clone();
    let sort = Sort::Field(Column::Name, true);
    app.tab_mut().sort = sort;
    app.tab_mut().rebuild(true);
    assert_eq!(app.tab().groups[0].label, "Ноябрь 2026");
    assert_eq!(app.tab().entries[app.tab().visible[0]].name, "file-12.txt");
    assert_eq!(app.tab().entry().unwrap().path, path);
    assert!(app.tab().marked.contains(&path));
    let mut policy = app.settings.grouping;
    policy.descending = false;
    app.set_grouping(policy);
    assert_eq!(app.tab().groups[0].label, "Ноябрь 2024");
    assert_eq!(app.settings.list_columns, columns);
    assert_eq!(app.tab().sort, sort);
    let config = tmp.path().join("settings.json");
    app.settings.save(&config).unwrap();
    assert_eq!(Settings::load(&config).unwrap().grouping, policy);
    policy.enabled = false;
    app.set_grouping(policy);
    assert!(app.tab().groups.is_empty());
    assert!(app.tab().group_view.is_none());
    assert_eq!(app.tab().entry().unwrap().path, path);
}
#[test]
fn contextual_checkbox_dropdown_is_compact_and_rejects_old_controls() {
    let (_tmp, mut app) = setup(ViewMode::List);
    let mut p = app.settings.grouping;
    p.enabled = false;
    app.set_grouping(p);
    render(&mut app, 34, 14);
    let area = app.file_areas[0];
    app.handle(Input::SecondaryClick {
        x: area.right() - 1,
        y: area.y,
    });
    let index = app
        .command_items()
        .iter()
        .position(|i| i.action == Action::Grouping)
        .unwrap();
    if let Some(Dialog::Commands(m)) = &mut app.dialog {
        m.selected = index;
        m.offset = index;
    }
    render(&mut app, 34, 14);
    let r = hit(&app, |t| {
        matches!(t, Target::GroupCheck { enabled: false, .. })
    });
    click(&mut app, r);
    assert!(app.settings.grouping.enabled);
    render(&mut app, 34, 14);
    let old = match &app.dialog {
        Some(Dialog::Grouping(m)) => m.token,
        _ => panic!("group menu"),
    };
    for row in 0..12 {
        assert!(
            app.hits
                .iter()
                .any(|(_, t)| matches!(t,Target::GroupControl{row:r,..}if *r==row))
        );
    }
    let r = hit(&app, |t| matches!(t, Target::GroupControl { row: 5, .. }));
    click(&mut app, r);
    assert_eq!(app.settings.grouping.field, Column::Extension);
    app.group_control(old, 0);
    assert!(app.settings.grouping.enabled, "stale checkbox ignored");
    render(&mut app, 34, 14);
    let r = hit(&app, |t| matches!(t, Target::GroupControl { row: 0, .. }));
    click(&mut app, r);
    assert!(!app.settings.grouping.enabled);
}
#[test]
fn tree_roots_keep_descendants_and_headers_never_become_entries() {
    let mut entries = vec![
        entry("folder", Some(date(2025, 1))),
        entry("folder/child", Some(date(2026, 1))),
        entry("file", Some(date(2026, 1))),
    ];
    entries[0].directory = true;
    let mut visible = vec![0, 1, 2];
    let g = project(
        &entries,
        &mut visible,
        Grouping {
            enabled: true,
            field: Column::Modified,
            descending: true,
        },
        Some(std::path::Path::new("/test")),
    );
    assert_eq!(visible, [2, 0, 1]);
    assert_eq!(g[1].count, 1);
    assert_eq!(g[1].range, 1..3);
    let (_tmp, mut app) = setup(ViewMode::Icons);
    let b = render(&mut app, 150, 44);
    let row = (0..44)
        .find(|&y| {
            (0..150)
                .map(|x| b[(x, y)].symbol())
                .collect::<String>()
                .contains("Ноябрь 2026")
        })
        .unwrap();
    assert!(
        !app.hits.iter().any(
            |(r, t)| r.y == row && matches!(t, Target::Entry { .. } | Target::EntryMark { .. })
        )
    );
    assert_eq!(app.tab().visible.len(), app.tab().entries.len());
}

#[test]
fn manual_group_view_survives_background_refresh_without_rounding_to_old_grid() {
    for mode in ViewMode::ALL {
        let (tmp, mut app) = setup(mode);
        // A one-file first group makes the next group's entry index deliberately
        // unaligned to both the icon grid and the horizontal column stride.
        for i in 1..39 {
            fs::File::options()
                .write(true)
                .open(tmp.path().join(format!("file-{i:02}.txt")))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(date(2025, 11)))
                .unwrap();
        }
        app.action(Action::Refresh);
        settle(&mut app);
        render(&mut app, 100, 24);
        loop {
            let v = app.tab().group_view.as_ref().unwrap();
            let before = v.offset;
            if app.tab().offset >= 1 {
                break;
            }
            app.tab_mut().scroll_rows(1);
            render(&mut app, 100, 24);
            assert_ne!(app.tab().group_view.as_ref().unwrap().offset, before);
        }
        let before = app.tab().group_view.as_ref().unwrap().offset;
        let first = app.tab().entries[app.tab().visible[app.tab().offset]]
            .path
            .clone();
        app.action(Action::Refresh);
        settle(&mut app);
        render(&mut app, 100, 24);
        assert_eq!(
            app.tab().group_view.as_ref().unwrap().offset,
            before,
            "{mode:?}"
        );
        assert_eq!(
            app.tab().entries[app.tab().visible[app.tab().offset]].path,
            first,
            "{mode:?}"
        );
    }
}

#[test]
fn headers_are_background_for_marquee_and_shift_selection_crosses_groups() {
    let (_tmp, mut app) = setup(ViewMode::List);
    render(&mut app, 150, 60);
    let groups = app.tab().groups.clone();
    let first = groups[0].range.end - 1;
    let last = groups[1].range.start + 1;
    app.tab_mut().selected = first;
    app.handle(Input::Key(KeyPress {
        shift: true,
        ..KeyPress::plain(Key::Down)
    }));
    app.handle(Input::Key(KeyPress {
        shift: true,
        ..KeyPress::plain(Key::Down)
    }));
    assert_eq!(app.tab().selected, last);
    assert_eq!(app.tab().marked.len(), 3);
    assert_eq!(app.tab().targets().len(), 3);
    render(&mut app, 150, 60);
    let r = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 0 }));
    let end = hit(&app, |t| matches!(t, Target::Entry { pane: 0, index: 14 }));
    let header_y = r.y - 1;
    app.handle(Input::PointerDown {
        x: r.x + 3,
        y: header_y,
        double: false,
        toggle: false,
    });
    app.handle(Input::Drag {
        x: r.x + 10,
        y: end.y,
    });
    app.handle(Input::PointerUp {
        x: r.x + 10,
        y: end.y,
    });
    assert_eq!(app.tab().marked.len(), 15);
    assert!(app.tab().targets().iter().all(|p| p.is_file()));
}

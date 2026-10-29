use basalgrim::{
    app::{App, Dialog, Target},
    fs::{self, Operation},
    input::{Action, Input, Key, KeyPress},
    operations::{OperationRecord, Phase, Progress},
    settings::Settings,
    ui,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    fs as disk,
    time::{Duration, Instant},
};

fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.loading() {
        assert!(Instant::now() < deadline);
        app.tick();
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::render(app, frame)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn a_finished_progress_bar_keeps_its_label_visible_on_light_and_dark_palettes() {
    use basalgrim::{fs::Report, preferences::Theme, ui::theme::Palette};
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_path_buf(), Settings::default());
    let mut record = OperationRecord::new(1, "Копирование".into(), 1);
    record.finish(Report {
        total: 1,
        completed: vec![root.path().join("completed")],
        progress: Progress {
            phase: Phase::Finished,
            bytes_done: 4096,
            bytes_total: Some(4096),
            ..Default::default()
        },
        ..Default::default()
    });
    app.operations.push_front(record);
    app.action(Action::Operations);
    for theme in [Theme::MoltenZharr, Theme::Light] {
        app.settings.appearance.theme = theme;
        let palette = Palette::new(&app.settings);
        let filled = palette.color(if palette.light() {
            "active"
        } else {
            "selection_accent"
        });
        let buffer = render(&mut app, 120, 36);
        let text_cells: Vec<_> = buffer
            .content
            .iter()
            .filter(|cell| cell.bg == filled && !cell.symbol().trim().is_empty())
            .collect();
        assert!(
            text_cells.len() > 8,
            "The filled progress bar lost its label"
        );
        assert!(text_cells.iter().all(|cell| cell.fg != cell.bg));
    }
}

#[test]
fn shared_operation_screen_shows_byte_progress_and_scrolls_completed_paths() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_path_buf(), Settings::default());
    app.icon_backend = basalgrim::icons::IconBackend::Gui;
    settle(&mut app);
    let mut record = OperationRecord::new(1, "Копирование".into(), 80);
    record.progress = Progress {
        items_skipped: 0,
        phase: Phase::Running,
        items_done: 37,
        items_total: 80,
        entries_done: 125,
        entries_total: Some(300),
        bytes_done: 4 * 1024 * 1024,
        bytes_total: Some(16 * 1024 * 1024),
        current: Some(root.path().join("current-file.bin")),
    };
    record.completed_paths = (0..37)
        .map(|index| root.path().join(format!("completed-{index}.bin")))
        .collect();
    app.operations.push_front(record);
    app.action(Action::Operations);
    let selected = app.tab().selected;
    let buffer = render(&mut app, 120, 36);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("Готово 37/80"));
    assert!(text.contains("4.0 MiB") && text.contains("16.0 MiB"));
    assert!(text.contains("125 / 300"));
    assert!(app.operations_detail_max > 0);
    app.handle(Input::Key(KeyPress::plain(Key::PageDown)));
    assert!(app.operations_detail_offset > 0);
    for _ in 0..30 {
        app.handle(Input::Key(KeyPress::plain(Key::PageDown)));
    }
    let buffer = render(&mut app, 120, 36);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("completed-36.bin"));
    assert_eq!(app.tab().selected, selected);
    for (width, height) in [(1, 1), (24, 8), (34, 14), (50, 18), (80, 26), (150, 44)] {
        let buffer = render(&mut app, width, height);
        assert!(
            app.hits
                .iter()
                .all(|(area, _)| area.right() <= buffer.area.right()
                    && area.bottom() <= buffer.area.bottom())
        );
        assert!(app.hits.iter().all(|(_, target)| matches!(
            target,
            Target::Operation(_) | Target::Cancel | Target::Action(Action::CancelOperation)
        )));
        assert!(app.icon_slots.is_empty());
    }
    app.handle(Input::Key(KeyPress::plain(Key::Escape)));
    assert!(app.dialog.is_none());
}

#[test]
fn worker_records_real_results_and_bounds_the_session_history() {
    let root = tempfile::tempdir().unwrap();
    let mut app = App::new(root.path().to_path_buf(), Settings::default());
    settle(&mut app);
    for index in 0..22 {
        app.start_operation(Operation::Create {
            parent: root.path().to_path_buf(),
            name: format!("file-{index}"),
            directory: false,
        });
        settle(&mut app);
        assert_eq!(app.operations[0].status(), "Завершено");
        assert_eq!(app.operations[0].progress.items_done, 1);
    }
    assert_eq!(app.operation_count, 22);
    assert_eq!(app.operations.len(), 20);
    assert_eq!(
        app.operations[0].completed_paths,
        [root.path().join("file-21")]
    );
    app.action(Action::Operations);
    app.handle(Input::Key(KeyPress::plain(Key::End)));
    render(&mut app, 80, 26);
    assert!(app.operations_offset > 0);
    assert_eq!(app.operations_selected, 19);
    let browser_selection = app.tab().selected;
    let target = app
        .hits
        .iter()
        .find_map(|(area, target)| matches!(target, Target::Operation(18)).then_some(*area))
        .unwrap();
    app.handle(Input::Pointer(Some((target.x, target.y).into())));
    render(&mut app, 80, 26);
    assert_eq!(app.operations_selected, 19);
    app.handle(Input::Click {
        x: target.x,
        y: target.y,
        double: false,
        toggle: false,
    });
    assert_eq!(app.operations_selected, 18);
    assert_eq!(app.tab().selected, browser_selection);
}

#[test]
fn partial_failure_lists_the_successful_items_instead_of_claiming_full_success() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    disk::create_dir(&destination).unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    disk::write(&first, "one").unwrap();
    disk::write(&second, "two").unwrap();
    disk::write(destination.join("second"), "precious").unwrap();
    let report = fs::operate(Operation::Transfer {
        sources: vec![first.clone(), second],
        destination,
        cut: false,
    });
    let mut record = OperationRecord::new(1, "Копирование".into(), 2);
    record.finish(report);
    assert_eq!(record.status(), "Ошибка");
    assert_eq!(record.completed_paths, [first]);
    let mut app = App::new(root.path().to_path_buf(), Settings::default());
    app.operations.push_front(record);
    app.dialog = Some(Dialog::Operations);
    let buffer = render(&mut app, 120, 36);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("Готово 1/2"));
    assert!(text.contains("Завершённые элементы:"));
    assert!(text.contains("first"));
    assert!(text.contains("Цель уже существует"));
}

use anyhow::Result;
use basalgrim::{
    app::App,
    input::{Input, Key, KeyPress},
    options::Options,
    raster,
    settings::Settings,
    ui,
};
use eframe::egui;
use egui_ratatui::RataguiBackend;
use ratatui::{
    Terminal,
    backend::Backend,
    buffer::Buffer,
    layout::{Rect, Size},
};
use soft_ratatui::RgbPixmap;
use std::time::{Duration, Instant};

fn main() {
    if let Err(e) = run() {
        eprintln!("basalgrim: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let Some(options) = Options::parse()? else {
        return Ok(());
    };
    let (settings, settings_path, warning) = Settings::startup();
    let mut app = App::startup(options, settings, warning);
    app.icon_backend = basalgrim::icons::IconBackend::Gui;
    app.settings_save_available = settings_path.is_some();
    let mut font_settings = (
        app.settings.appearance.font,
        app.settings.appearance.font_size,
    );
    let mut terminal = Terminal::new(RataguiBackend::new(
        "basalgrim",
        raster::font_backend(font_settings.0, font_settings.1, 140, 45),
    ))?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([400.0, 360.0]),
        ..Default::default()
    };
    let mut uploaded: Option<Buffer> = None;
    let mut uploaded_icons = Vec::new();
    let mut uploaded_selection = None;
    let mut icons = basalgrim::icons::images::IconRasterizer::default();
    let mut previous_click: Option<(egui::Pos2, Instant)> = None;
    eframe::run_ui_native("Basalgrim · Molten-Zharr", options, move |root, _| {
        let ctx = root.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) && app.busy {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            app.message("Перед выходом дождитесь завершения файловой операции", true);
        }
        app.tick();
        ctx.request_repaint_after(Duration::from_millis(if app.loading() { 30 } else { 250 }));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(root, |gui| {
                let scale = ctx.pixels_per_point();
                let wanted_size = (f32::from(app.settings.appearance.font_size) * scale)
                    .round()
                    .clamp(1.0, f32::from(u16::MAX)) as u16;
                let wanted = (app.settings.appearance.font, wanted_size);
                if wanted != font_settings {
                    terminal.backend_mut().soft_backend =
                        raster::font_backend(wanted.0, wanted.1, 140, 45);
                    let _ = terminal.clear();
                    uploaded = None;
                    font_settings = wanted;
                }
                let available = gui.available_size();
                let soft = &mut terminal.backend_mut().soft_backend;
                let columns =
                    (available.x * scale / soft.char_width as f32).clamp(1.0, 512.0) as u16;
                let rows = (available.y * scale / soft.char_height as f32).clamp(1.0, 256.0) as u16;
                if soft.size().ok() != Some(Size::new(columns, rows)) {
                    soft.buffer.resize(Rect::new(0, 0, columns, rows));
                    soft.rgb_pixmap = RgbPixmap::new(
                        usize::from(columns) * soft.char_width,
                        usize::from(rows) * soft.char_height,
                    );
                    soft.always_redraw_list.clear();
                    soft.rendered_cursor = None;
                    uploaded = None;
                }
                // Hover uses the current grid before painting; font/DPI and window
                // size changes must use the same mapping as clicks.
                let hover_image = egui::Rect::from_min_size(
                    gui.next_widget_position(),
                    egui::vec2(
                        soft.get_pixmap_width() as f32,
                        soft.get_pixmap_height() as f32,
                    ) / scale,
                );
                let pointer = ctx
                    .input(|input| input.pointer.hover_pos())
                    .and_then(|point| pointer_cell(hover_image, Size::new(columns, rows), point));
                app.handle(Input::Pointer(pointer));
                let _ = terminal.draw(|frame| ui::render(&mut app, frame));
                let backend = terminal.backend_mut();
                if uploaded.as_ref() != Some(&backend.soft_backend.buffer)
                    || uploaded_icons != app.icon_slots
                    || uploaded_selection != app.selection_area()
                {
                    let rgb = raster::compose_frame(
                        &backend.soft_backend,
                        &app.icon_slots,
                        &mut icons,
                        app.selection_area(),
                        app.settings.appearance.palette()["selection_accent"],
                    )
                    .unwrap_or_else(|error| {
                        app.icon_backend = basalgrim::icons::IconBackend::Symbols;
                        app.message(format!("Не удалось отрисовать иконки: {error:#}"), true);
                        ctx.request_repaint();
                        backend.soft_backend.get_pixmap_data().to_vec()
                    });
                    let image = egui::ColorImage::from_rgb(
                        [
                            backend.soft_backend.get_pixmap_width(),
                            backend.soft_backend.get_pixmap_height(),
                        ],
                        &rgb,
                    );
                    if let Some(texture) = &mut backend.text_handle {
                        texture.set(image, egui::TextureOptions::NEAREST);
                    } else {
                        backend.text_handle = Some(ctx.load_texture(
                            "basalgrim",
                            image,
                            egui::TextureOptions::NEAREST,
                        ));
                    }
                    uploaded = Some(backend.soft_backend.buffer.clone());
                    uploaded_icons = app.icon_slots.clone();
                    uploaded_selection = app.selection_area();
                }
                let image_size = egui::vec2(
                    backend.soft_backend.get_pixmap_width() as f32,
                    backend.soft_backend.get_pixmap_height() as f32,
                ) / scale;
                let (canvas, _) = gui.allocate_exact_size(available, egui::Sense::click_and_drag());
                let image_rect = egui::Rect::from_min_size(canvas.min, image_size);
                let [r, g, b] = app.settings.appearance.palette()["background"];
                gui.painter()
                    .rect_filled(canvas, 0, egui::Color32::from_rgb(r, g, b));
                if let Some(texture) = &backend.text_handle {
                    gui.painter().image(
                        texture.id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                let to_cell = |pos: egui::Pos2| -> Option<(u16, u16)> {
                    pointer_cell(image_rect, Size::new(columns, rows), pos)
                        .map(|point| (point.x, point.y))
                };
                let events = ctx.input(|i| i.events.clone());
                if !events.is_empty() {
                    ctx.request_repaint();
                }
                let mut suppress_text = false;
                let mut paste_into_text = app.text_active();
                for event in events {
                    match event {
                        egui::Event::Key {
                            key,
                            physical_key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            paste_into_text = app.text_active();
                            if let Some(key) = event_key(key, physical_key, modifiers.shift) {
                                app.handle(Input::Key(KeyPress {
                                    key,
                                    ctrl: modifiers.ctrl || modifiers.command,
                                    alt: modifiers.alt,
                                    shift: modifiers.shift,
                                }));
                                // A plain shortcut such as / opens an editor;
                                // its accompanying Text must not type / into it.
                                suppress_text = !paste_into_text && app.text_active();
                            }
                        }
                        egui::Event::Text(text) if app.text_active() && !suppress_text => {
                            app.handle(Input::Text(text))
                        }
                        egui::Event::Paste(text) if app.text_active() && paste_into_text => {
                            app.handle(Input::Text(text))
                        }
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Secondary,
                            pressed: true,
                            ..
                        } => {
                            previous_click = None;
                            if let Some((x, y)) = to_cell(pos) {
                                app.handle(Input::SecondaryClick { x, y });
                            }
                        }
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers,
                        } => {
                            if let Some((x, y)) = to_cell(pos) {
                                let double = previous_click.is_some_and(|(p, t)| {
                                    p.distance(pos) < 5.0
                                        && t.elapsed() < Duration::from_millis(400)
                                });
                                previous_click = if double {
                                    None
                                } else {
                                    Some((pos, Instant::now()))
                                };
                                app.handle(Input::PointerDown {
                                    x,
                                    y,
                                    double,
                                    toggle: modifiers.ctrl,
                                });
                            }
                        }
                        egui::Event::PointerMoved(pos)
                            if ctx.input(|i| i.pointer.primary_down()) =>
                        {
                            if let Some((x, y)) = to_cell(pos) {
                                app.handle(Input::Drag { x, y });
                                if app.selection_area().is_some() {
                                    previous_click = None;
                                }
                            }
                        }
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            ..
                        } => {
                            if let Some((x, y)) = to_cell(pos) {
                                app.handle(Input::Drag { x, y });
                                if app.selection_area().is_some() {
                                    previous_click = None;
                                }
                                app.handle(Input::PointerUp { x, y });
                            } else {
                                app.handle(Input::CancelDrag);
                            }
                        }
                        egui::Event::MouseWheel { delta, .. } => {
                            if let Some((x, y)) =
                                ctx.input(|i| i.pointer.hover_pos()).and_then(to_cell)
                                && delta.y != 0.0
                            {
                                app.handle(Input::Scroll {
                                    x,
                                    y,
                                    delta: if delta.y > 0.0 { -1 } else { 1 },
                                });
                            }
                        }
                        egui::Event::PointerGone | egui::Event::WindowFocused(false) => {
                            previous_click = None;
                            app.handle(Input::CancelDrag);
                            app.handle(Input::Pointer(None));
                        }
                        _ => {}
                    }
                }
            });
        app.persist_preferences(settings_path.as_deref());
        if app.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    })
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
fn pointer_cell(
    image: egui::Rect,
    size: Size,
    point: egui::Pos2,
) -> Option<ratatui::layout::Position> {
    if !image.contains(point) || image.width() <= 0.0 || image.height() <= 0.0 {
        return None;
    }
    let x = ((point.x - image.left()) / image.width() * f32::from(size.width)).floor() as u16;
    let y = ((point.y - image.top()) / image.height() * f32::from(size.height)).floor() as u16;
    (x < size.width && y < size.height).then(|| ratatui::layout::Position::new(x, y))
}
fn event_key(key: egui::Key, physical: Option<egui::Key>, shift: bool) -> Option<Key> {
    // egui-winit already falls back to the physical key for non-Latin layouts.
    // Overriding a recognized logical key breaks remapped layouts and virtual
    // keyboards (their physical codes can mean entirely different commands).
    convert_key(
        if key == egui::Key::Plus
            || (key == egui::Key::Equals && physical == Some(egui::Key::Equals) && shift)
        {
            egui::Key::Plus
        } else {
            key
        },
    )
}
fn convert_key(key: egui::Key) -> Option<Key> {
    Some(match key {
        egui::Key::Enter => Key::Enter,
        egui::Key::Escape => Key::Escape,
        egui::Key::Backspace => Key::Backspace,
        egui::Key::Delete => Key::Delete,
        egui::Key::Tab => Key::Tab,
        egui::Key::ArrowUp => Key::Up,
        egui::Key::ArrowDown => Key::Down,
        egui::Key::ArrowLeft => Key::Left,
        egui::Key::ArrowRight => Key::Right,
        egui::Key::Home => Key::Home,
        egui::Key::End => Key::End,
        egui::Key::PageUp => Key::PageUp,
        egui::Key::PageDown => Key::PageDown,
        egui::Key::Space => Key::Char(' '),
        egui::Key::Slash => Key::Char('/'),
        egui::Key::F1 => Key::F(1),
        egui::Key::F2 => Key::F(2),
        egui::Key::F3 => Key::F(3),
        egui::Key::F5 => Key::F(5),
        egui::Key::F7 => Key::F(7),
        egui::Key::F8 => Key::F(8),
        egui::Key::F9 => Key::F(9),
        egui::Key::Insert => Key::Insert,
        egui::Key::Minus => Key::Char('-'),
        egui::Key::Quote => Key::Char('\''),
        other => {
            let name = other.symbol_or_name();
            if let Some(number) = name
                .strip_prefix('F')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| *n <= 12)
            {
                Key::F(number)
            } else if name.len() == 1 {
                Key::Char(name.chars().next().unwrap().to_ascii_lowercase())
            } else {
                return None;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logical_keys_survive_remapped_and_virtual_keyboard_codes() {
        assert_eq!(
            event_key(egui::Key::C, Some(egui::Key::Equals), true),
            Some(Key::Char('c'))
        );
        assert_eq!(
            event_key(egui::Key::A, Some(egui::Key::Q), false),
            Some(Key::Char('a'))
        );
        assert_eq!(
            event_key(egui::Key::V, Some(egui::Key::Escape), false),
            Some(Key::Char('v'))
        );
        assert_eq!(
            event_key(egui::Key::Enter, Some(egui::Key::Escape), false),
            Some(Key::Enter)
        );
        // Non-Latin layouts have already fallen back to physical in egui-winit.
        assert_eq!(
            event_key(egui::Key::C, Some(egui::Key::C), false),
            Some(Key::Char('c'))
        );
        assert_eq!(
            event_key(egui::Key::Equals, Some(egui::Key::Equals), true),
            Some(Key::Char('+'))
        );
    }
    #[test]
    fn gui_delivers_every_key_offered_by_the_binding_picker() {
        for expected in basalgrim::input::shortcut_choices() {
            assert!(
                egui::Key::ALL
                    .iter()
                    .copied()
                    .any(|key| convert_key(key) == Some(expected)),
                "Missing GUI key: {expected:?}"
            );
        }
    }
}

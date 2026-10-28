use super::*;

pub fn render(app: &App, frame: &mut Frame, palette: Palette) {
    let Some(area) = app.selection_area() else {
        return;
    };
    let area = area.intersection(frame.area());
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut frame.buffer_mut()[(x, y)];
            cell.bg = palette.hover_background(cell.bg);
        }
    }
    if app.icon_backend != crate::icons::IconBackend::Gui {
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_style(palette.accent()),
            area,
        );
    }
}

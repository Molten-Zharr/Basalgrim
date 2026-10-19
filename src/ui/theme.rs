use crate::{
    preferences::{self, BorderWeight, Corners},
    settings::Settings,
};
use ratatui::style::{Color, Style};

#[derive(Clone, Copy)]
pub struct Palette {
    colors: [Color; 16],
    pub borders: BorderWeight,
    pub corners: Corners,
}
impl Palette {
    pub fn new(settings: &Settings) -> Self {
        let colors = settings.appearance.palette();
        Self {
            colors: std::array::from_fn(|i| {
                let [r, g, b] = colors[&preferences::roles()[i].token];
                Color::Rgb(r, g, b)
            }),
            borders: settings.appearance.borders,
            corners: settings.appearance.corners,
        }
    }
    pub fn color(self, token: &str) -> Color {
        self.colors[preferences::roles()
            .iter()
            .position(|role| role.token == token)
            .expect("Known palette role")]
    }
    pub fn text(self) -> Style {
        Style::default()
            .fg(self.color("text"))
            .bg(self.color("background"))
    }
    pub fn muted(self) -> Style {
        self.text().fg(self.color("folder_path"))
    }
    pub fn accent(self) -> Style {
        self.text().fg(self.color(if self.light() {
            "active"
        } else {
            "selection_accent"
        }))
    }
    pub fn selection(self) -> Style {
        self.accent()
            .fg(self.color(if self.light() {
                "text"
            } else {
                "selection_accent"
            }))
            .bg(self.color("selected_file_background"))
    }
    pub fn light(self) -> bool {
        matches!(self.color("background"),Color::Rgb(r,g,b) if u16::from(r)+u16::from(g)+u16::from(b)>500)
    }
    /// Derive a quiet hover tint from the current palette, preserving selected backgrounds.
    pub fn hover_background(self, background: Color) -> Color {
        if background == self.color("selected_file_background")
            || background == self.color("selection_accent")
        {
            return background;
        }
        let Color::Rgb(r, g, b) = background else {
            return self.hover_background(self.color("background"));
        };
        let (target, amount) = if background == self.color("background") {
            (self.color("selected_file_background"), 2)
        } else {
            (self.color("text"), 8)
        };
        let Color::Rgb(tr, tg, tb) = target else {
            unreachable!("Palette colors are RGB");
        };
        let blend = |base: u8, tint: u8| {
            ((u16::from(base) * (amount - 1) + u16::from(tint)) / amount) as u8
        };
        Color::Rgb(blend(r, tr), blend(g, tg), blend(b, tb))
    }
}

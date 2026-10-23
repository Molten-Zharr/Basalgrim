//! Appearance settings adapted from Almavorn's palette roles and font/geometry controls.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::OnceLock};

pub const MIN_FONT_SIZE: u16 = 10;
pub const MAX_FONT_SIZE: u16 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IconMode {
    #[default]
    Images,
    Symbols,
}
impl IconMode {
    pub const ALL: [Self; 2] = [Self::Images, Self::Symbols];
    pub fn name(self) -> &'static str {
        match self {
            Self::Images => "Изображения",
            Self::Symbols => "Символы",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontFace {
    #[default]
    FiraCode,
    FiraCodeBold,
    DejaVuMono,
}
impl FontFace {
    pub const ALL: [Self; 3] = [Self::FiraCode, Self::FiraCodeBold, Self::DejaVuMono];
    pub fn name(self) -> &'static str {
        match self {
            Self::FiraCode => "Fira Code",
            Self::FiraCodeBold => "Fira Code Bold",
            Self::DejaVuMono => "DejaVu Sans Mono",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BorderWeight {
    None,
    #[default]
    Single,
    Double,
    Thick,
}
impl BorderWeight {
    pub const ALL: [Self; 4] = [Self::None, Self::Single, Self::Double, Self::Thick];
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "Без рамки",
            Self::Single => "Одинарная",
            Self::Double => "Двойная",
            Self::Thick => "Толстая",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Corners {
    Square,
    #[default]
    Rounded,
}
impl Corners {
    pub const ALL: [Self; 2] = [Self::Square, Self::Rounded];
    pub fn name(self) -> &'static str {
        match self {
            Self::Square => "Прямые",
            Self::Rounded => "Скруглённые",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    MoltenZharr,
    ClassicAmber,
    ClassicViolet,
    Light,
}
impl Theme {
    pub const ALL: [Self; 4] = [
        Self::MoltenZharr,
        Self::ClassicAmber,
        Self::ClassicViolet,
        Self::Light,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::MoltenZharr => "Molten-Zharr",
            Self::ClassicAmber => "Classic Amber",
            Self::ClassicViolet => "Classic Violet",
            Self::Light => "Light",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    pub font: FontFace,
    pub font_size: u16,
    pub borders: BorderWeight,
    pub corners: Corners,
    pub icon_mode: IconMode,
    pub colors: BTreeMap<Theme, BTreeMap<String, [u8; 3]>>,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            font: FontFace::default(),
            font_size: 16,
            borders: BorderWeight::default(),
            corners: Corners::default(),
            icon_mode: IconMode::default(),
            colors: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
pub struct ColorRole {
    pub token: String,
    pub hex: String,
    pub role_ru: String,
}
pub fn roles() -> &'static [ColorRole] {
    static ROLES: OnceLock<Vec<ColorRole>> = OnceLock::new();
    ROLES.get_or_init(|| {
        #[derive(Deserialize)]
        struct Brand {
            colors: Vec<ColorRole>,
        }
        serde_json::from_str::<Brand>(include_str!("../assets/brand/molten-zharr.json"))
            .expect("Bundled palette is valid")
            .colors
    })
}
pub fn parse_color(value: &str) -> Result<[u8; 3]> {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    ensure!(
        value.len() == 6 && value.is_ascii(),
        "Укажите цвет в формате #RRGGBB"
    );
    let value = u32::from_str_radix(value, 16)
        .map_err(|_| anyhow::anyhow!("Укажите цвет в формате #RRGGBB"))?;
    Ok([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}
pub fn color_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}
pub fn role_name(token: &str) -> &'static str {
    match token {
        "background" => "Фон",
        "text" => "Текст",
        "button_text" => "Текст кнопок",
        "active" => "Активная рамка",
        "sidebar_title" => "Заголовки",
        "error" => "Ошибка",
        "inactive_file_border" => "Рамка файлов",
        "inactive_panel_border" => "Рамка панелей",
        "gradient_dark" => "Тёмный градиент",
        "folder_icon" => "Значок папки",
        "selection_accent" => "Акцент выделения",
        "confirm_background" => "Подтверждение",
        "help_heading" => "Заголовки справки",
        "folder_path" => "Путь папки",
        "separator" => "Разделители",
        "selected_file_background" => "Фон выделения",
        _ => "Цвет",
    }
}
pub fn base_colors(theme: Theme) -> BTreeMap<String, [u8; 3]> {
    let mut colors: BTreeMap<_, _> = roles()
        .iter()
        .map(|r| {
            (
                r.token.clone(),
                parse_color(&r.hex).expect("Bundled color is valid"),
            )
        })
        .collect();
    if theme != Theme::MoltenZharr {
        for (role, rgb) in [
            ("background", [18, 22, 29]),
            ("text", [223, 228, 238]),
            ("active", [232, 158, 74]),
            ("inactive_panel_border", [138, 153, 175]),
            ("inactive_file_border", [138, 153, 175]),
            ("folder_path", [138, 153, 175]),
            ("selected_file_background", [49, 61, 78]),
            ("error", [255, 119, 135]),
        ] {
            colors.insert(role.into(), rgb);
        }
    }
    if theme == Theme::ClassicViolet {
        colors.insert("active".into(), [188, 125, 228]);
    }
    if theme == Theme::Light {
        for (role, rgb) in [
            ("background", [239, 242, 246]),
            ("text", [31, 39, 49]),
            ("active", [152, 78, 0]),
            ("folder_path", [85, 99, 116]),
            ("selected_file_background", [212, 225, 239]),
            ("error", [174, 37, 57]),
            ("button_text", [31, 39, 49]),
        ] {
            colors.insert(role.into(), rgb);
        }
    }
    colors
}
impl Appearance {
    pub fn palette(&self) -> BTreeMap<String, [u8; 3]> {
        let mut palette = base_colors(self.theme);
        if let Some(overrides) = self.colors.get(&self.theme) {
            palette.extend(overrides.clone());
        }
        palette
    }
    pub fn normalize(&mut self) -> Result<()> {
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        for palette in self.colors.values() {
            for token in palette.keys() {
                ensure!(
                    roles().iter().any(|r| &r.token == token),
                    "Неизвестная роль цвета: {token}"
                );
            }
        }
        Ok(())
    }
}

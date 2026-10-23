use super::*;
use crate::{
    input::{self, Key, KeyPress},
    preferences::{
        self, Appearance, BorderWeight, Corners, FontFace, IconMode, MAX_FONT_SIZE, MIN_FONT_SIZE,
        Theme,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Themes,
    Shortcuts,
}
impl SettingsSection {
    pub const ALL: [Self; 2] = [Self::Themes, Self::Shortcuts];
    pub fn name(self) -> &'static str {
        match self {
            Self::Themes => "Темы",
            Self::Shortcuts => "Клавиши",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Presets,
    Palette,
    Typography,
    Icons,
}
impl SettingsTab {
    pub const ALL: [Self; 4] = [Self::Presets, Self::Palette, Self::Typography, Self::Icons];
    pub fn name(self) -> &'static str {
        match self {
            Self::Presets => "Пресеты",
            Self::Palette => "Палитра",
            Self::Typography => "Шрифт и геометрия",
            Self::Icons => "Иконки",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsFocus {
    #[default]
    Menu,
    Tabs,
    Parameters,
}
#[derive(Default)]
pub struct SettingsView {
    pub section: SettingsSection,
    pub tab: SettingsTab,
    pub focus: SettingsFocus,
    pub selected: usize,
    pub offset: usize,
    pub capacity: usize,
    pub parameters_area: Rect,
    pub tabs_area: Rect,
    pub modifiers: [bool; 3],
    pub binding_offset: usize,
    pub binding_capacity: usize,
}
#[derive(Clone)]
pub enum SettingsEdit {
    FontSize,
    Color { theme: Theme, token: String },
}
impl SettingsEdit {
    pub fn title(&self) -> &'static str {
        match self {
            Self::FontSize => "Размер шрифта · 10–32",
            Self::Color { .. } => "Цвет · #RRGGBB",
        }
    }
}
#[derive(Clone)]
pub enum SettingControl {
    IconMode,
    Theme(Theme),
    ResetAppearance,
    Color(String),
    ResetPalette,
    Font,
    FontSize,
    Borders,
    Corners,
    Binding(usize),
    ResetBindings,
}
pub struct SettingRow {
    pub label: String,
    pub value: String,
    pub description: String,
    pub control: SettingControl,
    pub adjustable: bool,
    pub color: Option<[u8; 3]>,
}
fn cycle<T: Copy + PartialEq>(current: T, all: &[T], direction: isize) -> T {
    let index = all.iter().position(|v| *v == current).unwrap_or(0) as isize;
    all[(index + direction).rem_euclid(all.len() as isize) as usize]
}
impl App {
    pub fn settings_rows(&self) -> Vec<SettingRow> {
        let mut rows = Vec::new();
        let mut push =
            |label: String, value: String, description: String, control, adjustable, color| {
                rows.push(SettingRow {
                    label,
                    value,
                    description,
                    control,
                    adjustable,
                    color,
                })
            };
        if self.settings_view.section == SettingsSection::Shortcuts {
            push(
                "Сбросить все сочетания".into(),
                "Восстановить".into(),
                "Вернуть стандартные сочетания для всех команд, включая дополнительные клавиши."
                    .into(),
                SettingControl::ResetBindings,
                false,
                None,
            );
            for (index, binding) in self.settings.bindings.iter().enumerate() {
                push(
                    binding.action.name().into(),
                    binding.key.label(),
                    format!(
                        "{}: Enter — назначить, Delete — вернуть исходную клавишу. Совпадающие и зарезервированные сочетания отклоняются.",
                        binding.action.name()
                    ),
                    SettingControl::Binding(index),
                    false,
                    None,
                );
            }
        } else {
            match self.settings_view.tab {
                SettingsTab::Icons => {
                    let availability = match self.icon_backend {
                        crate::icons::IconBackend::Gui => "В окне изображения доступны.",
                        crate::icons::IconBackend::Symbols => {
                            "Этот терминал не поддерживает изображения: используются символы. Выбор изображений сохранится для совместимого терминала и GUI."
                        }
                        _ => "Терминал поддерживает изображения.",
                    };
                    push(
                        "Иконки файлов".into(),
                        self.settings.appearance.icon_mode.name().into(),
                        format!(
                            "Изображения или текстовые символы. {availability} Изменение применяется сразу."
                        ),
                        SettingControl::IconMode,
                        true,
                        None,
                    );
                }
                SettingsTab::Presets => {
                    for theme in Theme::ALL {
                        push(theme.name().into(),if theme==self.settings.appearance.theme {"Текущая"} else {"Применить"}.into(),"Палитра из Almavorn. Применяется сразу; выбранные шрифт и геометрия сохраняются.".into(),SettingControl::Theme(theme),false,None);
                    }
                    push("Сбросить оформление".into(),"Molten-Zharr".into(),"Вернуть исходные тему, цвета, шрифт, размер и рамки. Файловые настройки и клавиши сохраняются.".into(),SettingControl::ResetAppearance,false,None);
                }
                SettingsTab::Palette => {
                    let palette = self.settings.appearance.palette();
                    for role in preferences::roles() {
                        let color = palette[&role.token];
                        push(
                            preferences::role_name(&role.token).into(),
                            preferences::color_hex(color),
                            role.role_ru.clone(),
                            SettingControl::Color(role.token.clone()),
                            false,
                            Some(color),
                        );
                    }
                    push("Сбросить цвета темы".into(),"Восстановить".into(),"Удалить изменения цветов только текущей темы. Цвета остальных тем сохраняются.".into(),SettingControl::ResetPalette,false,None);
                }
                SettingsTab::Typography => {
                    let appearance = &self.settings.appearance;
                    let font_note = "В окне шрифт меняется сразу. В TUI шрифт и размер задаются терминалом; выбор сохраняется для оконного запуска.";
                    push(
                        "Шрифт".into(),
                        appearance.font.name().into(),
                        font_note.into(),
                        SettingControl::Font,
                        true,
                        None,
                    );
                    push(
                        "Размер шрифта".into(),
                        appearance.font_size.to_string(),
                        format!("10–32 пикселя. ←/→ — один шаг, Enter — ввести число. {font_note}"),
                        SettingControl::FontSize,
                        true,
                        None,
                    );
                    push(
                        "Рамки".into(),
                        appearance.borders.name().into(),
                        "Без рамки, одинарные, двойные или толстые. Применяется в GUI и TUI."
                            .into(),
                        SettingControl::Borders,
                        true,
                        None,
                    );
                    push("Углы".into(),appearance.corners.name().into(),"Прямые или скруглённые углы одинарной рамки. Для двойной и толстой рамки используются её собственные углы.".into(),SettingControl::Corners,true,None);
                }
            }
        }
        rows
    }
    pub fn preferences_changed(&mut self) {
        self.settings_dirty = true;
        self.settings_save_error = None;
        self.message("Настройки изменены", false);
    }
    pub fn settings_section(&mut self, section: SettingsSection) {
        self.settings_view.section = section;
        self.settings_view.focus = SettingsFocus::Menu;
        self.settings_view.selected = 0;
        self.settings_view.offset = 0;
    }
    pub fn settings_tab(&mut self, tab: SettingsTab) {
        self.settings_view.tab = tab;
        self.settings_view.section = SettingsSection::Themes;
        self.settings_view.focus = SettingsFocus::Tabs;
        self.settings_view.selected = 0;
        self.settings_view.offset = 0;
    }
    pub fn setting(&mut self, index: usize, activate: bool, direction: isize) {
        let Some(row) = self.settings_rows().into_iter().nth(index) else {
            return;
        };
        self.settings_view.selected = index;
        self.settings_view.focus = SettingsFocus::Parameters;
        if !activate && !row.adjustable {
            return;
        }
        let appearance = &mut self.settings.appearance;
        match row.control {
            SettingControl::IconMode => {
                appearance.icon_mode = cycle(appearance.icon_mode, &IconMode::ALL, direction)
            }
            SettingControl::Theme(theme) => appearance.theme = theme,
            SettingControl::ResetAppearance => *appearance = Appearance::default(),
            SettingControl::ResetPalette => {
                appearance.colors.remove(&appearance.theme);
            }
            SettingControl::Font => {
                appearance.font = cycle(appearance.font, &FontFace::ALL, direction)
            }
            SettingControl::FontSize if !activate => {
                appearance.font_size = appearance
                    .font_size
                    .saturating_add_signed(direction as i16)
                    .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
            }
            SettingControl::Borders => {
                appearance.borders = cycle(appearance.borders, &BorderWeight::ALL, direction)
            }
            SettingControl::Corners => {
                appearance.corners = cycle(appearance.corners, &Corners::ALL, direction)
            }
            SettingControl::ResetBindings => self.settings.bindings = input::default_bindings(),
            SettingControl::FontSize => {
                self.dialog = Some(Dialog::Edit {
                    kind: EditKind::Setting(SettingsEdit::FontSize),
                    text: appearance.font_size.to_string(),
                    error: None,
                });
                return;
            }
            SettingControl::Color(token) => {
                let text = preferences::color_hex(appearance.palette()[&token]);
                self.dialog = Some(Dialog::Edit {
                    kind: EditKind::Setting(SettingsEdit::Color {
                        theme: appearance.theme,
                        token,
                    }),
                    text,
                    error: None,
                });
                return;
            }
            SettingControl::Binding(index) => {
                self.settings_view.modifiers = [false; 3];
                self.settings_view.binding_offset = 0;
                self.dialog = Some(Dialog::CaptureBinding { index, error: None });
                return;
            }
        }
        self.preferences_changed();
    }
    pub(super) fn submit_setting(&mut self, edit: SettingsEdit, text: &str) -> anyhow::Result<()> {
        match edit {
            SettingsEdit::FontSize => {
                let size = text
                    .trim()
                    .parse::<u16>()
                    .map_err(|_| anyhow::anyhow!("Введите число от 10 до 32"))?;
                anyhow::ensure!(
                    (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size),
                    "Введите число от 10 до 32"
                );
                self.settings.appearance.font_size = size;
            }
            SettingsEdit::Color { theme, token } => {
                let rgb = preferences::parse_color(text)?;
                self.settings
                    .appearance
                    .colors
                    .entry(theme)
                    .or_default()
                    .insert(token, rgb);
            }
        }
        self.preferences_changed();
        Ok(())
    }
    pub fn capture_binding(&mut self, index: usize, key: KeyPress) {
        let key = key.normalized();
        let result = (|| {
            let binding = self
                .settings
                .bindings
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("Команда не найдена"))?;
            input::validate_binding(binding.action, key)?;
            if let Some(other) = self
                .settings
                .bindings
                .iter()
                .enumerate()
                .find(|(i, b)| *i != index && b.key.normalized() == key)
            {
                anyhow::bail!("{} уже назначено: {}", key.label(), other.1.action.name());
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.settings.bindings[index].key = key;
                self.preferences_changed();
                self.dialog = Some(Dialog::Settings);
            }
            Err(error) => {
                self.dialog = Some(Dialog::CaptureBinding {
                    index,
                    error: Some(format!("{error:#}")),
                })
            }
        }
    }
    pub fn reset_binding(&mut self, index: usize) {
        let Some(binding) = self.settings.bindings.get(index) else {
            return;
        };
        let ordinal = self.settings.bindings[..index]
            .iter()
            .filter(|b| b.action == binding.action)
            .count();
        if let Some(default) = input::default_bindings()
            .iter()
            .filter(|b| b.action == binding.action)
            .nth(ordinal)
        {
            self.capture_binding(index, default.key);
        }
    }
    pub(super) fn settings_key(&mut self, press: KeyPress) {
        let KeyPress { key, shift, .. } = press;
        if key == Key::Escape {
            self.dialog = None;
            return;
        }
        if press.ctrl && key == Key::Char('s') {
            self.settings_save_error = None;
            self.settings_dirty = true;
            return;
        }
        if self.settings.action_for(press) == Some(Action::Quit) && (press.ctrl || press.alt) {
            self.action(Action::Quit);
            return;
        }
        let focus = self.settings_view.focus;
        match key {
            Key::Tab => {
                let focuses = if self.settings_view.section == SettingsSection::Themes {
                    &[
                        SettingsFocus::Menu,
                        SettingsFocus::Tabs,
                        SettingsFocus::Parameters,
                    ][..]
                } else {
                    &[SettingsFocus::Menu, SettingsFocus::Parameters][..]
                };
                self.settings_view.focus = cycle(focus, focuses, if shift { -1 } else { 1 });
            }
            Key::Right | Key::Enter if focus == SettingsFocus::Menu => {
                self.settings_view.focus = if self.settings_view.section == SettingsSection::Themes
                {
                    SettingsFocus::Tabs
                } else {
                    SettingsFocus::Parameters
                }
            }
            Key::Enter if focus == SettingsFocus::Tabs => {
                self.settings_view.focus = SettingsFocus::Parameters
            }
            Key::Left | Key::Right if focus == SettingsFocus::Tabs => self.settings_tab(cycle(
                self.settings_view.tab,
                &SettingsTab::ALL,
                if key == Key::Left { -1 } else { 1 },
            )),
            Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown => self
                .settings_move(match key {
                    Key::Up => -1,
                    Key::Down => 1,
                    Key::Home => isize::MIN,
                    Key::End => isize::MAX,
                    Key::PageUp => -(self.settings_view.capacity.max(1) as isize),
                    _ => self.settings_view.capacity.max(1) as isize,
                }),
            Key::Enter | Key::Char(' ') if focus == SettingsFocus::Parameters => {
                self.setting(self.settings_view.selected, true, 1)
            }
            Key::Left | Key::Right if focus == SettingsFocus::Parameters => self.setting(
                self.settings_view.selected,
                false,
                if key == Key::Left { -1 } else { 1 },
            ),
            Key::Delete if focus == SettingsFocus::Parameters => {
                if let Some(row) = self.settings_rows().get(self.settings_view.selected)
                    && let SettingControl::Binding(index) = row.control
                {
                    self.reset_binding(index);
                }
            }
            _ => {}
        }
    }
    pub(super) fn settings_move(&mut self, direction: isize) {
        match self.settings_view.focus {
            SettingsFocus::Menu => {
                let index = usize::from(self.settings_view.section == SettingsSection::Shortcuts)
                    .saturating_add_signed(direction)
                    .min(1);
                self.settings_section(SettingsSection::ALL[index]);
            }
            SettingsFocus::Tabs => self.settings_tab(cycle(
                self.settings_view.tab,
                &SettingsTab::ALL,
                direction.clamp(-1, 1),
            )),
            SettingsFocus::Parameters => {
                self.settings_view.selected = self
                    .settings_view
                    .selected
                    .saturating_add_signed(direction)
                    .min(self.settings_rows().len().saturating_sub(1))
            }
        }
    }
}

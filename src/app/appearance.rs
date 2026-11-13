use super::*;
use crate::{
    appearance::{self, Outcome, Preset, Request},
    input::{Input, Key, KeyPress},
    operations::Cancellation,
    preferences::{Appearance, Theme},
};
use std::time::Instant;

#[derive(Clone)]
pub enum ProfileControl {
    Save,
    Apply(u64),
    Rename(u64),
    Remove(u64),
    Import,
    Export,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Places,
    Preview,
}
struct Pending {
    id: u64,
    revision: u64,
    theme: Theme,
    cancel: Cancellation,
}
pub struct DesignState {
    pub revision: u64,
    pub status: String,
    pending: Option<Pending>,
    pub panel_token: u64,
    pub panel_offset: usize,
    pub panel_selected: usize,
    pub panel_max: usize,
    pub sidebar_visible: bool,
    pub preview_visible: bool,
    pub epoch: Instant,
}
impl Default for DesignState {
    fn default() -> Self {
        Self {
            revision: 0,
            status: String::new(),
            pending: None,
            panel_token: 0,
            panel_offset: 0,
            panel_selected: 0,
            panel_max: 0,
            sidebar_visible: false,
            preview_visible: false,
            epoch: Instant::now(),
        }
    }
}
impl Drop for DesignState {
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            p.cancel.cancel();
        }
    }
}
impl DesignState {
    pub fn working(&self) -> bool {
        self.pending.is_some()
    }
}
impl App {
    pub fn profile_control(&mut self, revision: u64, control: ProfileControl) {
        if !matches!(self.dialog, Some(Dialog::Settings)) || self.design.revision != revision {
            return;
        }
        if matches!(control, ProfileControl::Stop) {
            if let Some(p) = &self.design.pending {
                p.cancel.cancel();
            }
            return;
        }
        if self.design.working() {
            return;
        }
        self.design.status.clear();
        self.settings_view.focus = SettingsFocus::Parameters;
        let selected = match control {
            ProfileControl::Save => Some(0),
            ProfileControl::Import => Some(1),
            ProfileControl::Export => Some(2),
            ProfileControl::Apply(id) | ProfileControl::Rename(id) | ProfileControl::Remove(id) => {
                self.settings
                    .appearance_presets
                    .iter()
                    .position(|p| p.id == id)
                    .map(|i| i + 3)
            }
            ProfileControl::Stop => None,
        };
        if let Some(selected) = selected {
            self.settings_view.selected = selected;
        }
        let edit = match control {
            ProfileControl::Save => {
                if self.settings.appearance_presets.len() >= appearance::MAX_PRESETS {
                    self.design.status = "Сохранено 32 пресета; удалите ненужный".into();
                    return;
                }
                let saved = appearance::snapshot(&self.settings.appearance);
                (SettingsEdit::PresetSave(saved), "Мой пресет".into())
            }
            ProfileControl::Apply(id) => {
                let Some(p) = self.settings.appearance_presets.iter().find(|p| p.id == id) else {
                    return;
                };
                let title = p.name.clone();
                let saved = p.appearance.clone();
                self.cancel_file_drag();
                self.finish_marquee(true);
                appearance::apply(&mut self.settings.appearance, &saved);
                self.preferences_changed();
                self.design.status = format!("Применён пресет: {title}");
                return;
            }
            ProfileControl::Rename(id) => {
                let Some(p) = self.settings.appearance_presets.iter().find(|p| p.id == id) else {
                    return;
                };
                (SettingsEdit::PresetRename(id), p.name.clone())
            }
            ProfileControl::Remove(id) => {
                self.settings.appearance_presets.retain(|p| p.id != id);
                self.preferences_changed();
                self.design.status = "Пресет удалён; текущее оформление сохранено".into();
                return;
            }
            ProfileControl::Import => (
                SettingsEdit::PaletteImport(self.settings.appearance.theme),
                self.tab()
                    .entry()
                    .filter(|e| !e.directory)
                    .map(|e| e.path.clone())
                    .unwrap_or_else(|| self.tab().cwd.join("palette.json"))
                    .to_string_lossy()
                    .into_owned(),
            ),
            ProfileControl::Export => (
                SettingsEdit::PaletteExport(crate::appearance::PaletteFile::capture(
                    &self.settings.appearance,
                )),
                self.tab()
                    .cwd
                    .join("palette.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ProfileControl::Stop => return,
        };
        let n = edit.1.len();
        self.begin_edit(EditKind::Setting(edit.0), edit.1, 0..n);
    }
    pub(super) fn save_preset(
        &mut self,
        rename: Option<u64>,
        saved: Option<Appearance>,
        text: &str,
    ) -> anyhow::Result<()> {
        let name = appearance::name(text)?;
        anyhow::ensure!(
            !self
                .settings
                .appearance_presets
                .iter()
                .any(|p| Some(p.id) != rename && p.name.to_lowercase() == name.to_lowercase()),
            "Такое название уже есть"
        );
        if let Some(id) = rename {
            self.settings
                .appearance_presets
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| anyhow::anyhow!("Пресет уже удалён"))?
                .name = name;
        } else {
            anyhow::ensure!(
                self.settings.appearance_presets.len() < appearance::MAX_PRESETS,
                "Допускается до 32 пресетов"
            );
            let id = self
                .settings
                .appearance_presets
                .iter()
                .map(|p| p.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Нет свободного id"))?;
            self.settings.appearance_presets.push(Preset {
                id,
                name,
                appearance: saved.unwrap(),
            });
        }
        Ok(())
    }
    pub(super) fn palette_file(&mut self, request: Request, theme: Theme) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.design.working(),
            "Дождитесь завершения обмена палитрой"
        );
        let id = self.token();
        let cancel = Cancellation::default();
        anyhow::ensure!(
            self.worker.appearance(id, request, cancel.clone()),
            "Исполнитель палитры занят"
        );
        self.design.pending = Some(Pending {
            id,
            revision: self.design.revision,
            theme,
            cancel,
        });
        self.design.status = "Обмен палитрой…".into();
        Ok(())
    }
    pub(super) fn finish_palette_file(&mut self, id: u64, result: Result<Outcome, String>) {
        if !self.design.pending.as_ref().is_some_and(|p| p.id == id) {
            return;
        }
        let p = self.design.pending.take().unwrap();
        self.design.status = match result {
            Ok(Outcome::Exported(path)) => {
                format!("Палитра сохранена: {}", fs::clean(&path.to_string_lossy()))
            }
            Ok(Outcome::Imported(palette))
                if !p.cancel.is_cancelled()
                    && p.revision == self.design.revision
                    && matches!(self.dialog, Some(Dialog::Settings)) =>
            {
                match palette.apply(&mut self.settings.appearance, p.theme) {
                    Ok(()) => {
                        self.preferences_changed();
                        format!("Палитра применена: {}", palette.name)
                    }
                    Err(e) => format!("Ошибка: {e:#}"),
                }
            }
            Ok(Outcome::Imported(_)) => "Импорт отменён: окно или оформление изменилось".into(),
            Err(_) if p.cancel.is_cancelled() => "Обмен палитрой остановлен".into(),
            Err(e) => format!("Ошибка: {}", fs::clean(&e)),
        };
        if !matches!(self.dialog, Some(Dialog::Settings)) {
            self.message(self.design.status.clone(), false);
        }
    }
    pub(super) fn tick_design(&mut self) {
        if let Some(p) = &self.design.pending
            && (!matches!(self.dialog, Some(Dialog::Settings))
                || p.revision != self.design.revision)
        {
            p.cancel.cancel();
        }
    }
    pub fn open_panel(&mut self, kind: PanelKind) {
        if self.dialog.is_some() {
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        if kind == PanelKind::Preview && !self.settings.preview {
            self.settings.preview = true;
            self.preferences_changed();
        }
        self.design.panel_token = self.token();
        self.design.panel_offset = 0;
        self.design.panel_selected = 0;
        self.dialog = Some(Dialog::Panel(kind));
    }
    pub fn panel_scroll(&mut self, delta: isize) {
        if matches!(self.dialog, Some(Dialog::Panel(PanelKind::Preview))) {
            self.preview_offset = self.preview_offset.saturating_add_signed(delta);
        } else {
            self.design.panel_selected = self
                .design
                .panel_selected
                .saturating_add_signed(delta)
                .min(self.design.panel_max);
        }
    }
    pub(super) fn panel_key(&mut self, key: KeyPress) -> bool {
        if !matches!(self.dialog, Some(Dialog::Panel(_))) {
            return false;
        }
        match key.key {
            Key::Enter if matches!(self.dialog, Some(Dialog::Panel(PanelKind::Places))) => {
                let i = self
                    .design
                    .panel_selected
                    .saturating_sub(self.design.panel_offset);
                if let Some((r, _)) = self
                    .hits
                    .iter()
                    .filter(|(_, t)| matches!(t, Target::PanelLink { .. }))
                    .nth(i)
                    .cloned()
                {
                    self.handle(Input::Click {
                        x: r.x,
                        y: r.y,
                        double: false,
                        toggle: false,
                    });
                }
            }
            Key::Escape => self.cancel_dialog(),
            Key::Up => self.panel_scroll(-1),
            Key::Down => self.panel_scroll(1),
            Key::PageUp => self.panel_scroll(-8),
            Key::PageDown => self.panel_scroll(8),
            _ => {}
        }
        true
    }
    pub fn activity(&self) -> &'static str {
        appearance::activity(
            self.settings.appearance.animations,
            self.design.epoch.elapsed().as_millis(),
        )
    }
}

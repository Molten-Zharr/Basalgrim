use crate::{
    input::{self, Action, Binding, KeyPress},
    preferences::Appearance,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub grid: bool,
    pub preview: bool,
    pub dual: bool,
    pub hidden: bool,
    #[serde(flatten)]
    pub appearance: Appearance,
    pub bindings: Vec<Binding>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            grid: true,
            preview: true,
            dual: false,
            hidden: false,
            appearance: Appearance::default(),
            bindings: input::default_bindings(),
        }
    }
}
impl Settings {
    pub fn startup() -> (Self, Option<PathBuf>, Option<String>) {
        let Some(path) = Self::path() else {
            return (Self::default(), None, None);
        };
        match Self::load(&path) {
            Ok(settings) => (settings, Some(path), None),
            Err(error) => (
                Self::default(),
                None,
                Some(format!(
                    "Настройки не загружены; исходный файл сохранён: {error:#}"
                )),
            ),
        }
    }
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("molten-zharr/basalgrim/settings.json"))
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut settings: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid settings JSON")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        settings.normalize()?;
        Ok(settings)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate_bindings()?;
        let parent = path.parent().context("Settings path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn normalize(&mut self) -> Result<()> {
        self.appearance.normalize()?;
        for binding in &mut self.bindings {
            binding.key = binding.key.normalized();
        }
        // Older settings do not have these commands. Keep custom bindings intact,
        // including keys that were free before the commands were introduced.
        for action in [
            Action::Operations,
            Action::CancelOperation,
            Action::Commands,
        ] {
            if self.bindings.iter().any(|binding| binding.action == action) {
                continue;
            }
            let default = input::default_bindings()
                .into_iter()
                .find(|binding| binding.action == action)
                .unwrap();
            let mut candidates = vec![default.key];
            for n in 1..=12 {
                for (ctrl, alt, shift) in [
                    (true, false, false),
                    (false, true, false),
                    (true, true, false),
                    (true, false, true),
                ] {
                    candidates.push(KeyPress {
                        key: input::Key::F(n),
                        ctrl,
                        alt,
                        shift,
                    });
                }
            }
            let key = candidates
                .into_iter()
                .find(|key| !self.bindings.iter().any(|binding| binding.key == *key))
                .context("Нет свободного сочетания для новой команды")?;
            self.bindings.push(Binding { action, key });
        }
        self.validate_bindings()
    }
    pub fn validate_bindings(&self) -> Result<()> {
        for (index, binding) in self.bindings.iter().enumerate() {
            input::validate_binding(binding.action, binding.key)?;
            anyhow::ensure!(
                !self.bindings[..index]
                    .iter()
                    .any(|b| b.key.normalized() == binding.key.normalized()),
                "Конфликт горячих клавиш: {}",
                binding.key.label()
            );
        }
        for &action in Action::ALL {
            anyhow::ensure!(
                self.bindings.iter().any(|b| b.action == action),
                "Нет сочетания для команды: {}",
                action.name()
            );
        }
        Ok(())
    }
    pub fn action_for(&self, key: KeyPress) -> Option<Action> {
        let key = key.normalized();
        self.bindings
            .iter()
            .find(|binding| binding.key.normalized() == key)
            .map(|binding| binding.action)
    }
    pub fn shortcut(&self, action: Action) -> String {
        self.bindings
            .iter()
            .find(|b| b.action == action)
            .map_or_else(|| "—".into(), |b| b.key.label())
    }
    pub fn shortcuts(&self, action: Action) -> String {
        self.bindings
            .iter()
            .filter(|b| b.action == action)
            .map(|b| b.key.label())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

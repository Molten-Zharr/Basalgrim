//! Named appearance snapshots and bounded, explicit palette interchange.
use crate::{
    operations::Cancellation,
    preferences::{self, Appearance, Theme},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub const MAX_PRESETS: usize = 32;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub id: u64,
    pub name: String,
    pub appearance: Appearance,
}
pub fn name(value: &str) -> Result<String> {
    let value = value.trim();
    ensure!(
        !value.is_empty() && value.chars().count() <= 48 && !value.chars().any(char::is_control),
        "Название: от 1 до 48 символов без управляющих знаков"
    );
    Ok(value.into())
}
pub fn validate(presets: &mut [Preset]) -> Result<()> {
    ensure!(
        presets.len() <= MAX_PRESETS,
        "Допускается до 32 своих пресетов"
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for p in presets {
        ensure!(p.id != 0 && ids.insert(p.id), "Повторный id пресета");
        p.name = name(&p.name)?;
        ensure!(
            names.insert(p.name.to_lowercase()),
            "Повторное название пресета: {}",
            p.name
        );
        p.appearance.normalize()?;
    }
    Ok(())
}
pub fn snapshot(current: &Appearance) -> Appearance {
    let mut saved = current.clone();
    saved.colors = BTreeMap::from([(current.theme, current.palette())]);
    saved
}
pub fn apply(current: &mut Appearance, saved: &Appearance) {
    let mut colors = current.colors.clone();
    colors.insert(saved.theme, saved.palette());
    *current = saved.clone();
    current.colors = colors;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteFile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub colors: BTreeMap<String, String>,
}
impl PaletteFile {
    pub fn capture(appearance: &Appearance) -> Self {
        Self {
            format: "basalgrim-palette".into(),
            version: 1,
            name: appearance.theme.name().into(),
            colors: appearance
                .palette()
                .into_iter()
                .map(|(k, v)| (k, preferences::color_hex(v)))
                .collect(),
        }
    }
    pub fn validate(&self) -> Result<BTreeMap<String, [u8; 3]>> {
        ensure!(
            self.format == "basalgrim-palette" && self.version == 1,
            "Неизвестный формат палитры; нужен basalgrim-palette версии 1"
        );
        name(&self.name)?;
        ensure!(
            self.colors.len() == preferences::roles().len(),
            "Палитра должна содержать все {} цветов",
            preferences::roles().len()
        );
        preferences::roles()
            .iter()
            .map(|role| {
                let color = self
                    .colors
                    .get(&role.token)
                    .with_context(|| format!("Отсутствует цвет {}", role.token))?;
                Ok((
                    role.token.clone(),
                    preferences::parse_color(color)
                        .with_context(|| format!("Цвет {}", role.token))?,
                ))
            })
            .collect()
    }
    pub fn apply(&self, appearance: &mut Appearance, theme: Theme) -> Result<()> {
        let colors = self.validate()?;
        appearance.colors.insert(theme, colors);
        Ok(())
    }
}
pub enum Request {
    Import(PathBuf),
    Export { path: PathBuf, palette: PaletteFile },
}
pub enum Outcome {
    Imported(PaletteFile),
    Exported(PathBuf),
}
pub fn execute(request: Request, cancel: &Cancellation) -> Result<Outcome> {
    crate::fs::operation_checkpoint(cancel)?;
    match request {
        Request::Import(path) => {
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(
                    (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
                )
                .open(&path)?;
            ensure!(
                file.metadata()?.is_file(),
                "Выберите обычный JSON-файл палитры"
            );
            let mut data = Vec::new();
            Read::by_ref(&mut file).take(16385).read_to_end(&mut data)?;
            ensure!(data.len() <= 16384, "Файл палитры больше 16 KiB");
            crate::fs::operation_checkpoint(cancel)?;
            let palette: PaletteFile =
                serde_json::from_slice(&data).context("Ошибка JSON палитры")?;
            palette.validate()?;
            Ok(Outcome::Imported(palette))
        }
        Request::Export { path, palette } => {
            palette.validate()?;
            let parent = path.parent().context("Укажите полный путь файла")?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            temp.write_all(&serde_json::to_vec_pretty(&palette)?)?;
            temp.as_file().sync_all()?;
            crate::fs::operation_checkpoint(cancel)?;
            if let Err(e) = temp.persist_noclobber(&path) {
                if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::bail!("Файл уже существует. Укажите другое имя.");
                }
                return Err(e.error).context("Не удалось сохранить палитру");
            }
            Ok(Outcome::Exported(path))
        }
    }
}
pub fn palette_path(text: &str) -> Result<PathBuf> {
    let p = Path::new(text);
    ensure!(
        p.is_absolute() && p.file_name().is_some(),
        "Укажите полный путь JSON-файла"
    );
    Ok(p.into())
}
pub fn activity(animations: bool, millis: u128) -> &'static str {
    if animations {
        [" | ", " / ", " — ", " \\ "][(millis / 160 % 4) as usize]
    } else {
        " ◌ "
    }
}

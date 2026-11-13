//! Versioned, opt-in executable extensions shared by both frontends.
mod package;
mod process;
mod service;
use crate::{
    fs::Entry,
    journal::{bytes, path},
    operations::Cancellation,
};
use anyhow::{Result, ensure};
pub use package::Store;
use serde::{Deserialize, Serialize};
pub use service::{Event, Job, Service};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_PLUGINS: usize = 32;
pub const MAX_OUTPUT: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub api: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub executable: String,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub preview: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub name: String,
    pub contexts: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
}
pub(crate) fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.api == 1, "Поддерживается API расширений 1");
        ensure!(
            identifier(&self.id) && label(&self.name, 120) && label(&self.version, 40),
            "Некорректные id, название или версия"
        );
        ensure!(
            self.description.len() <= 1024 && !self.description.chars().any(char::is_control),
            "Некорректное описание"
        );
        ensure!(
            identifier(&self.executable)
                || (self.executable.len() <= 120
                    && Path::new(&self.executable).components().count() == 1
                    && !self.executable.starts_with('.')
                    && !self.executable.chars().any(char::is_control)),
            "Исполнитель должен находиться в корне пакета"
        );
        ensure!(
            self.commands.len() <= 24 && self.events.len() <= 2 && self.preview.len() <= 32,
            "Слишком много возможностей расширения"
        );
        let mut ids = std::collections::BTreeSet::new();
        for command in &self.commands {
            ensure!(
                identifier(&command.id) && ids.insert(&command.id) && label(&command.name, 120),
                "Некорректная или повторная команда"
            );
            ensure!(
                !command.contexts.is_empty()
                    && command.contexts.len() <= 4
                    && command.contexts.iter().all(|s| matches!(
                        s.as_str(),
                        "file" | "directory" | "selection" | "background"
                    )),
                "Неизвестный контекст команды"
            );
            ensure!(
                command.extensions.len() <= 32,
                "Слишком много расширений файлов"
            );
        }
        ensure!(
            self.events
                .iter()
                .all(|e| matches!(e.as_str(), "directory_changed" | "operation_finished")),
            "Неизвестное событие"
        );
        for ext in self
            .preview
            .iter()
            .chain(self.commands.iter().flat_map(|c| &c.extensions))
        {
            ensure!(
                identifier(ext) && ext == &ext.to_lowercase(),
                "Расширения файлов задаются без точки, строчными буквами"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    pub directory: PathBuf,
    pub digest: String,
    pub enabled: bool,
    pub error: Option<String>,
    pub lease: Cancellation,
}
#[derive(Clone, Debug, Serialize)]
pub struct Selection {
    pub path: Vec<u8>,
    pub directory: bool,
    pub symlink: bool,
    pub size: u64,
    #[serde(skip)]
    pub modified: Option<std::time::SystemTime>,
}
impl From<&Entry> for Selection {
    fn from(e: &Entry) -> Self {
        Self {
            path: bytes(&e.path),
            directory: e.directory,
            symlink: e.symlink,
            size: e.size,
            modified: e.modified,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub cwd: Vec<u8>,
    pub selected: Vec<Selection>,
}
impl Context {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.selected.len() <= 256 && serde_json::to_vec(self)?.len() <= MAX_OUTPUT,
            "Слишком большой выбор для расширения (до 256 объектов / 128 KiB)"
        );
        ensure!(
            path(&self.cwd).is_absolute() && std::fs::metadata(path(&self.cwd))?.is_dir(),
            "Папка недоступна"
        );
        for s in &self.selected {
            let p = path(&s.path);
            ensure!(
                p.parent() == Some(path(&self.cwd).as_path()),
                "Объект вне выбранной папки"
            );
            let m = std::fs::symlink_metadata(&p)?;
            ensure!(
                m.len() == s.size
                    && m.modified().ok() == s.modified
                    && m.is_symlink() == s.symlink
                    && (m.is_dir() || (m.is_symlink() && p.is_dir())) == s.directory,
                "Объект изменился: {}",
                p.display()
            );
        }
        Ok(())
    }
    pub fn matches(&self, command: &Command) -> bool {
        let kind = match self.selected.as_slice() {
            [] => "background",
            [s] if s.directory => "directory",
            [_] => "file",
            _ => "selection",
        };
        command.contexts.iter().any(|c| c == kind)
            && (command.extensions.is_empty()
                || (!self.selected.is_empty()
                    && self
                        .selected
                        .iter()
                        .all(|s| extension_matches(&path(&s.path), &command.extensions))))
    }
}
pub fn extension_matches(p: &Path, extensions: &[String]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
}
#[derive(Serialize)]
pub struct Request<'a> {
    pub api: u32,
    pub hook: &'a str,
    pub command: Option<&'a str>,
    pub context: &'a Context,
    pub outcome: Option<&'a str>,
}
#[derive(Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub text: Option<String>,
}
pub type Catalog = Arc<Vec<Plugin>>;

pub fn preview(
    catalog: &Catalog,
    source: &Path,
    cancel: &Cancellation,
) -> Result<Option<crate::fs::Preview>> {
    let meta = std::fs::symlink_metadata(source)?;
    if !meta.is_file() {
        return Ok(None);
    }
    let Some(plugin) = catalog
        .iter()
        .find(|p| p.enabled && p.error.is_none() && extension_matches(source, &p.manifest.preview))
    else {
        return Ok(None);
    };
    let context = Context {
        cwd: bytes(source.parent().unwrap()),
        selected: vec![Selection {
            path: bytes(source),
            directory: false,
            symlink: false,
            size: meta.len(),
            modified: meta.modified().ok(),
        }],
    };
    let response = process::run(
        plugin,
        &Request {
            api: 1,
            hook: "preview",
            command: None,
            context: &context,
            outcome: None,
        },
        cancel,
        std::time::Duration::from_secs(3),
    )?;
    context.validate()?;
    let text = response.text.unwrap_or_default();
    let lines = text
        .lines()
        .take(2000)
        .map(|l| crate::fs::clean(&l.chars().take(2048).collect::<String>()))
        .collect();
    Ok(Some(crate::fs::Preview {
        title: crate::fs::clean(&source.file_name().unwrap_or_default().to_string_lossy()),
        details: vec![format!("Расширение: {}", plugin.manifest.name)],
        lines,
        code: None,
        image: None,
    }))
}

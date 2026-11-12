//! Desktop requests are local URI batches, never shell commands or file mutations.
mod install;
pub(crate) mod service;
use anyhow::{Context, Result, ensure};
pub use install::{Config, InstallAction, integrate};
pub use service::{Event, Runtime};
use std::{collections::BTreeMap, path::PathBuf};

pub const BUS: &str = "org.moltenzharr.Basalgrim";
pub const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
pub const OBJECT: &str = "/org/freedesktop/FileManager1";
pub const DESKTOP_ID: &str = "org.moltenzharr.Basalgrim.desktop";
pub const MAX_ITEMS: usize = 64;
pub const MAX_BYTES: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Open,
    Folders,
    Items,
    Properties,
}
impl Kind {
    pub fn method(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Folders => "ShowFolders",
            Self::Items => "ShowItems",
            Self::Properties => "ShowItemProperties",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Request {
    pub kind: Kind,
    pub paths: Vec<PathBuf>,
    pub startup_id: String,
}
impl Request {
    pub fn from_uris(kind: Kind, uris: &[String], startup_id: String) -> Result<Self> {
        ensure!(
            uris.len() <= MAX_ITEMS && uris.iter().map(String::len).sum::<usize>() <= MAX_BYTES,
            "Слишком большой запрос рабочего стола"
        );
        ensure!(
            startup_id.len() <= 2048 && !startup_id.contains('\0'),
            "Некорректный идентификатор запуска"
        );
        let mut paths = Vec::new();
        for uri in uris {
            let path = crate::clipboard::files::path(uri)?;
            ensure!(
                !path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
                "Путь должен быть без .."
            );
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        Ok(Self {
            kind,
            paths,
            startup_id,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Location {
    pub directory: PathBuf,
    pub selected: Vec<PathBuf>,
    pub properties: bool,
}
#[derive(Clone, Debug)]
pub struct Batch {
    pub locations: Vec<Location>,
}
/// Called only in the desktop worker. Preserve the final symlink/name when revealing it.
pub fn resolve(request: Request) -> Result<Batch> {
    let mut locations = Vec::new();
    let mut groups = BTreeMap::<PathBuf, usize>::new();
    for path in request.paths {
        let info = std::fs::symlink_metadata(&path).with_context(|| {
            format!(
                "Недоступен путь: {}",
                crate::fs::clean(&path.to_string_lossy())
            )
        })?;
        let folder = request.kind == Kind::Folders
            || request.kind == Kind::Open && (info.is_dir() || info.is_symlink() && path.is_dir());
        let directory = if folder {
            path.clone()
        } else {
            path.parent().unwrap_or(&path).to_path_buf()
        }
        .canonicalize()?;
        ensure!(directory.is_dir(), "Ожидалась папка");
        let selected = if folder {
            Vec::new()
        } else if path.parent().is_some() {
            vec![directory.join(path.file_name().context("Нет имени файла")?)]
        } else {
            Vec::new()
        };
        if request.kind == Kind::Items
            && let Some(&index) = groups.get(&directory)
        {
            let location: &mut Location = &mut locations[index];
            location.selected.extend(selected);
        } else {
            groups.insert(directory.clone(), locations.len());
            locations.push(Location {
                directory,
                selected,
                properties: request.kind == Kind::Properties,
            });
        }
    }
    Ok(Batch { locations })
}

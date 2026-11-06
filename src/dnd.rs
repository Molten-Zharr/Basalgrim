//! File drag sessions share path validation and operations; backends only transport offers.
use crate::clipboard::files::{self, FileList, Formats};
use anyhow::{Result, ensure};
use std::{path::PathBuf, sync::Arc};

#[cfg(feature = "guitui")]
pub mod native;
#[cfg(feature = "tui")]
pub mod terminal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    Copy,
    Move,
}
impl DropAction {
    pub fn bit(self) -> u32 {
        match self {
            Self::Copy => 1,
            Self::Move => 2,
        }
    }
    pub fn choose(allowed: u32, preferred: Self) -> Option<Self> {
        [preferred, Self::Copy, Self::Move]
            .into_iter()
            .find(|a| allowed & a.bit() != 0)
    }
}

pub fn encode(paths: Vec<PathBuf>) -> Result<Arc<[u8]>> {
    ensure!(!paths.is_empty(), "Пустое перетаскивание");
    let formats = files::encode(&FileList { paths, cut: false }, "dnd", 0)?;
    Ok(formats[files::URI_LIST].clone())
}
pub fn decode(bytes: Arc<[u8]>) -> Result<Vec<PathBuf>> {
    ensure!(
        bytes.len() <= files::MAX_BYTES,
        "Список перетаскивания превышает 4 MiB"
    );
    let formats = Formats::from([(files::URI_LIST.into(), bytes)]);
    let list = files::decode(&formats)?
        .ok_or_else(|| anyhow::anyhow!("Нет локальных файлов в перетаскивании"))?;
    ensure!(!list.paths.is_empty(), "Пустое перетаскивание");
    Ok(list.paths)
}

#[derive(Debug)]
pub struct Outgoing {
    pub id: u64,
    pub paths: Vec<PathBuf>,
    pub copy: bool,
}
#[derive(Clone, Debug)]
pub enum Request {
    Start {
        id: u64,
        paths: Vec<PathBuf>,
        copy: bool,
    },
    Accept {
        id: u64,
        action: Option<DropAction>,
    },
    Read {
        id: u64,
    },
    Finish {
        id: u64,
        action: Option<DropAction>,
    },
    Cancel {
        id: u64,
    },
}
#[derive(Debug)]
pub enum Event {
    Ready(Result<(), String>),
    Hover {
        id: u64,
        x: f64,
        y: f64,
        physical: bool,
        allowed: u32,
        preferred: DropAction,
    },
    Leave {
        id: u64,
    },
    Drop {
        id: u64,
    },
    Data {
        id: u64,
        result: Result<Vec<PathBuf>, String>,
    },
    Sent {
        id: u64,
        result: Result<(), String>,
    },
}

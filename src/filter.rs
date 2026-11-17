//! Per-pane visual filename filters. Depth counts directories below the current root.
use crate::fs::Entry;
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    All,
    Files,
    Folders,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Все файлы и папки",
            Self::Files => "Только файлы",
            Self::Folders => "Только папки",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// None is unlimited. Zero examines the root's immediate entries only.
    pub depth: Option<u8>,
    pub case_sensitive: bool,
    pub kind: Kind,
    pub extension: String,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            depth: Some(0),
            case_sensitive: false,
            kind: Kind::All,
            extension: String::new(),
        }
    }
}
impl Options {
    pub fn active(&self, query: &str) -> bool {
        !query.is_empty()
            || !self.extension.is_empty()
            || self.kind != Kind::All
            || self.depth != Some(0)
    }
    pub fn deep(&self) -> bool {
        self.depth != Some(0)
    }
    pub fn depth_label(&self) -> String {
        self.depth
            .map_or_else(|| "Максимальная".into(), |n| n.to_string())
    }
    pub fn hint(&self) -> String {
        match self.depth {
            Some(0) => "0 — только текущая папка".into(),
            Some(1) => "1 — на одну папку внутрь".into(),
            Some(n) => format!("{n} — на {n} папок внутрь"),
            None => "Максимальная — все подпапки".into(),
        }
    }
    pub fn matches_name(&self, name: &str, directory: bool, query: &str) -> bool {
        if (self.kind == Kind::Files && directory) || (self.kind == Kind::Folders && !directory) {
            return false;
        }
        let contains = |value: &str, needle: &str| {
            if self.case_sensitive {
                value.contains(needle)
            } else {
                value.to_lowercase().contains(&needle.to_lowercase())
            }
        };
        if !contains(name, query) {
            return false;
        }
        let extension = self.extension.trim().trim_start_matches('.');
        if extension.is_empty() {
            return true;
        }
        !directory
            && Path::new(name)
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| contains(s, extension))
    }
    pub fn matches(&self, e: &Entry, query: &str) -> bool {
        self.matches_name(&e.name, e.directory, query)
    }
}

pub struct Run {
    pub worker: crate::search::Search,
    pub query: String,
    pub options: Options,
    pub hidden: bool,
    pub sort: crate::fs::Sort,
    pub grouping: crate::grouping::Grouping,
    pub page: usize,
    pub accepted: Vec<Entry>,
    pub accepted_page: usize,
    pub marked_directories: std::collections::BTreeSet<std::path::PathBuf>,
}
impl Run {
    pub fn new(
        query: String,
        options: Options,
        hidden: bool,
        sort: crate::fs::Sort,
        grouping: crate::grouping::Grouping,
    ) -> Self {
        Self {
            worker: Default::default(),
            query,
            options,
            hidden,
            sort,
            grouping,
            page: 0,
            accepted: vec![],
            accepted_page: usize::MAX,
            marked_directories: Default::default(),
        }
    }
}

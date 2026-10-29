//! Conflict decisions shared by the filesystem worker, GUI and TUI.
use std::{path::PathBuf, time::SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Replace,
    Skip,
    KeepBoth,
    Cancel,
}
impl Choice {
    pub const ALL: [Self; 4] = [Self::Replace, Self::Skip, Self::KeepBoth, Self::Cancel];
    pub fn label(self) -> &'static str {
        match self {
            Self::Replace => "Заменить",
            Self::Skip => "Пропустить",
            Self::KeepBoth => "Сохранить оба",
            Self::Cancel => "Отменить операцию",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Decision {
    pub choice: Choice,
    pub remaining: bool,
}
impl Decision {
    pub const CANCEL: Self = Self {
        choice: Choice::Cancel,
        remaining: false,
    };
}
#[derive(Debug, Clone)]
pub struct ItemInfo {
    pub kind: &'static str,
    pub bytes: Option<u64>,
    pub modified: Option<SystemTime>,
}
#[derive(Debug, Clone)]
pub struct Conflict {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub incoming: ItemInfo,
    pub existing: ItemInfo,
    pub remaining: usize,
    pub can_replace: bool,
    pub can_keep_both: bool,
}
impl Conflict {
    pub fn allows(&self, choice: Choice) -> bool {
        match choice {
            Choice::Replace => self.can_replace,
            Choice::KeepBoth => self.can_keep_both,
            _ => true,
        }
    }
}
#[derive(Debug, Clone)]
pub struct DestinationRecovery {
    pub destination: PathBuf,
    pub backup: PathBuf,
}

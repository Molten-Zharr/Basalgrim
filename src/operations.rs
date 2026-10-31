//! Runtime operation state shared by GUI, TUI and filesystem workers.
use crate::fs::Report;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    #[default]
    Preparing,
    Running,
    Verifying,
    RemovingSource,
    AwaitingDecision,
    CleaningUp,
    Finished,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    pub phase: Phase,
    pub items_done: usize,
    pub items_skipped: usize,
    pub items_total: usize,
    pub entries_done: u64,
    pub entries_total: Option<u64>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    #[serde(skip)]
    pub current: Option<PathBuf>,
}
impl Progress {
    pub fn fraction(&self) -> f64 {
        let fraction = if let Some(total) = self.bytes_total.filter(|&total| total > 0) {
            self.bytes_done as f64 / total as f64
        } else if let Some(total) = self.entries_total.filter(|&total| total > 0) {
            self.entries_done as f64 / total as f64
        } else {
            (self.items_done + self.items_skipped) as f64 / self.items_total.max(1) as f64
        };
        fraction.clamp(
            0.0,
            if self.phase == Phase::Finished {
                1.0
            } else {
                0.99
            },
        )
    }
}

pub struct OperationRecord {
    pub id: u64,
    pub title: String,
    pub destination: Option<PathBuf>,
    pub progress: Progress,
    pub started: Instant,
    pub elapsed: Option<Duration>,
    pub cancel_requested: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub completed_paths: Vec<PathBuf>,
    pub retained_copies: Vec<crate::fs::RetainedCopy>,
    pub skipped_paths: Vec<PathBuf>,
    pub destination_recoveries: Vec<crate::conflicts::DestinationRecovery>,
    pub journal: Option<crate::journal::Receipt>,
    pub recorded_at: Option<i64>,
    pub interrupted: bool,
    pub external: bool,
    pub from_journal: bool,
}
impl OperationRecord {
    pub fn new(id: u64, title: String, items_total: usize) -> Self {
        Self {
            id,
            title,
            destination: None,
            progress: Progress {
                items_total,
                ..Default::default()
            },
            started: Instant::now(),
            elapsed: None,
            cancel_requested: false,
            cancelled: false,
            error: None,
            completed_paths: Vec::new(),
            retained_copies: Vec::new(),
            skipped_paths: Vec::new(),
            destination_recoveries: Vec::new(),
            journal: None,
            recorded_at: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(i64::MAX as u128) as i64,
            ),
            interrupted: false,
            external: false,
            from_journal: false,
        }
    }
    pub fn finish(&mut self, report: Report) {
        self.progress = report.progress;
        self.progress.phase = Phase::Finished;
        self.progress.items_done = report.completed.len();
        self.progress.items_total = report.total;
        self.elapsed = Some(self.started.elapsed());
        self.cancelled = report.cancelled;
        self.error = report.error;
        self.completed_paths = report.completed;
        self.retained_copies = report.retained_copies;
        self.progress.items_skipped = report.skipped.len();
        self.skipped_paths = report.skipped;
        self.destination_recoveries = report.destination_recoveries;
        self.journal = report.journal;
    }
    pub fn status(&self) -> &'static str {
        if self.interrupted {
            return "Прервано: нужна проверка";
        }
        if self.external {
            return "В другом окне";
        }
        if self.elapsed.is_some() {
            if self.error.is_some() {
                "Ошибка"
            } else if self.cancelled {
                "Отменено"
            } else {
                "Завершено"
            }
        } else if self.progress.phase == Phase::CleaningUp {
            "Очистка временных данных"
        } else if self.cancel_requested {
            "Отмена…"
        } else if self.progress.phase == Phase::Preparing {
            "Подготовка…"
        } else if self.progress.phase == Phase::Verifying {
            "Проверка копии…"
        } else if self.progress.phase == Phase::RemovingSource {
            "Удаление исходника…"
        } else if self.progress.phase == Phase::AwaitingDecision {
            "Ожидает выбора"
        } else {
            "Выполняется"
        }
    }
    pub fn duration(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }
}

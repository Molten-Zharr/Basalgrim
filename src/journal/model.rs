use crate::{
    fs::{Operation, Report},
    operations::{OperationRecord, Phase, Progress},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}
pub fn path(bytes: &[u8]) -> PathBuf {
    std::ffi::OsString::from_vec(bytes.to_vec()).into()
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub size: u64,
    pub modified: [i64; 2],
    pub changed: [i64; 2],
    pub uid: u32,
    pub gid: u32,
    pub link: Option<Vec<u8>>,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Option<Self>> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
            size: meta.len(),
            modified: [meta.mtime(), meta.mtime_nsec()],
            changed: [meta.ctime(), meta.ctime_nsec()],
            uid: meta.uid(),
            gid: meta.gid(),
            link: if meta.is_symlink() {
                Some(bytes(&fs::read_link(path)?))
            } else {
                None
            },
        }))
    }
    pub fn directory(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inverse {
    pub action: String,
    pub from: Option<Vec<u8>>,
    pub to: Option<Vec<u8>>,
    pub expected: Option<Identity>,
    pub original: Option<Identity>,
    /// A future executor must reject blocked plans, never infer missing evidence.
    pub blocked: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub kind: &'static str,
    pub source: PathBuf,
    pub target: Option<PathBuf>,
}
pub fn specs(operation: &Operation) -> Vec<Spec> {
    match operation {
        Operation::Create {
            parent,
            name,
            directory,
        } => {
            let target = parent.join(name);
            vec![Spec {
                kind: if *directory { "mkdir" } else { "create" },
                source: target.clone(),
                target: Some(target),
            }]
        }
        Operation::Rename { source, name } => vec![Spec {
            kind: "rename",
            source: source.clone(),
            target: source.parent().map(|p| p.join(name)),
        }],
        Operation::Transfer {
            sources,
            destination,
            cut,
        } => sources
            .iter()
            .map(|p| Spec {
                kind: if *cut { "move" } else { "copy" },
                source: p.clone(),
                target: p.file_name().map(|n| destination.join(n)),
            })
            .collect(),
        Operation::Duplicate {
            sources,
            destination,
        } => sources
            .iter()
            .map(|p| Spec {
                kind: "duplicate",
                source: p.clone(),
                target: p.file_name().map(|n| destination.join(n)),
            })
            .collect(),
        Operation::Trash { sources } => sources
            .iter()
            .map(|p| Spec {
                kind: "trash",
                source: p.clone(),
                target: None,
            })
            .collect(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub index: usize,
    pub kind: String,
    pub source: Vec<u8>,
    pub target: Option<Vec<u8>>,
    pub before: Option<Identity>,
    pub after: Option<Identity>,
    pub state: String,
    pub inverse: Option<Inverse>,
    pub issue: Option<String>,
    pub recovery_paths: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct Receipt {
    pub id: i64,
    pub items: Vec<Item>,
}

type RetainedPaths = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub progress: Progress,
    pub current: Option<Vec<u8>>,
    pub completed: Vec<Vec<u8>>,
    pub skipped: Vec<Vec<u8>>,
    pub cancelled: bool,
    pub error: Option<String>,
    pub retained: Vec<RetainedPaths>,
    pub recoveries: Vec<(Vec<u8>, Vec<u8>)>,
}
impl Summary {
    pub fn new(report: &Report) -> Self {
        fn reserve(budget: &mut usize, size: usize) -> bool {
            if let Some(left) = budget.checked_sub(size) {
                *budget = left;
                true
            } else {
                false
            }
        }
        fn selected(paths: &[PathBuf], budget: &mut usize) -> Vec<Vec<u8>> {
            paths
                .iter()
                .take(200)
                .map_while(|p| reserve(budget, p.as_os_str().len()).then(|| bytes(p)))
                .collect()
        }
        let mut budget = 256 * 1024;
        // Recovery information takes priority over long lists of successful paths.
        let retained = report
            .retained_copies
            .iter()
            .take(200)
            .map_while(|r| {
                let size = r.source.as_os_str().len()
                    + r.destination.as_os_str().len()
                    + r.recovery.as_ref().map_or(0, |p| p.as_os_str().len());
                reserve(&mut budget, size).then(|| {
                    (
                        bytes(&r.source),
                        bytes(&r.destination),
                        r.recovery.as_deref().map(bytes),
                    )
                })
            })
            .collect();
        let recoveries = report
            .destination_recoveries
            .iter()
            .take(200)
            .map_while(|r| {
                reserve(
                    &mut budget,
                    r.destination.as_os_str().len() + r.backup.as_os_str().len(),
                )
                .then(|| (bytes(&r.destination), bytes(&r.backup)))
            })
            .collect();
        let current = report
            .progress
            .current
            .as_ref()
            .and_then(|p| reserve(&mut budget, p.as_os_str().len()).then(|| bytes(p)));
        Self {
            progress: report.progress.clone(),
            current,
            completed: selected(&report.completed, &mut budget),
            skipped: selected(&report.skipped, &mut budget),
            cancelled: report.cancelled,
            error: report
                .error
                .as_ref()
                .map(|s| s.chars().take(8192).collect()),
            retained,
            recoveries,
        }
    }
}

pub struct Stored {
    pub id: i64,
    pub title: String,
    pub destination: Option<Vec<u8>>,
    pub started_ms: i64,
    pub elapsed_ms: u64,
    pub state: String,
    pub total: usize,
    pub summary: Option<Summary>,
    pub items: Vec<Item>,
    pub completed: usize,
    pub skipped: usize,
}
impl Stored {
    pub fn into_record(self, token: u64) -> OperationRecord {
        let mut record = OperationRecord::new(token, self.title, self.total);
        record.destination = self.destination.as_deref().map(path);
        record.journal = Some(Receipt {
            id: self.id,
            items: self.items,
        });
        record.recorded_at = Some(self.started_ms);
        record.from_journal = true;
        record.interrupted = self.state == "interrupted";
        record.external = self.state == "running";
        if !record.external {
            record.elapsed = Some(Duration::from_millis(self.elapsed_ms));
        }
        if let Some(summary) = self.summary {
            record.progress = summary.progress;
            record.progress.current = summary.current.as_deref().map(path);
            record.completed_paths = summary.completed.iter().map(|p| path(p)).collect();
            record.skipped_paths = summary.skipped.iter().map(|p| path(p)).collect();
            record.cancelled = summary.cancelled;
            record.error = summary.error;
            record.retained_copies = summary
                .retained
                .into_iter()
                .map(|(s, d, r)| crate::fs::RetainedCopy {
                    source: path(&s),
                    destination: path(&d),
                    recovery: r.as_deref().map(path),
                })
                .collect();
            record.destination_recoveries = summary
                .recoveries
                .into_iter()
                .map(|(d, b)| crate::conflicts::DestinationRecovery {
                    destination: path(&d),
                    backup: path(&b),
                })
                .collect();
        } else if !record.external {
            record.progress.phase = Phase::Finished;
        }
        if record.interrupted || record.external {
            record.progress.items_done = self.completed;
            record.progress.items_skipped = self.skipped;
        }
        if record.interrupted {
            record.error=Some("Работа прервана. Результат незавершённых шагов требует проверки; автоматический откат не выполнялся.".into());
        }
        record
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_summaries_prioritize_recovery_paths_and_preserve_true_counts() {
        let long = PathBuf::from(format!("/{}", "a".repeat(3900)));
        let report = Report {
            completed: vec![long; 1000],
            retained_copies: vec![crate::fs::RetainedCopy {
                source: "/original".into(),
                destination: "/copy".into(),
                recovery: Some("/recovery".into()),
            }],
            progress: Progress {
                items_done: 1000,
                items_total: 1001,
                ..Default::default()
            },
            ..Default::default()
        };
        let summary = Summary::new(&report);
        let encoded = serde_json::to_vec(&summary).unwrap();
        assert!(encoded.len() < 4 * 1024 * 1024);
        let decoded: Summary = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.progress.items_done, 1000);
        assert!(decoded.completed.len() < 200);
        assert_eq!(
            decoded.retained[0].2.as_deref(),
            Some(b"/recovery".as_slice())
        );
    }
}

//! Explicit, guarded disposal of retained undo data. Audit history is never erased.
use super::*;
use crate::operations::{Cancellation, Phase, Progress};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    #[default]
    All,
    Days7,
    Days30,
    Days90,
}
impl Retention {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Days7,
            Self::Days7 => Self::Days30,
            Self::Days30 => Self::Days90,
            Self::Days90 => Self::All,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "Все записи",
            Self::Days7 => "Старше 7 дней",
            Self::Days30 => "Старше 30 дней",
            Self::Days90 => "Старше 90 дней",
        }
    }
    fn cutoff(self) -> i64 {
        let days = match self {
            Self::All => return i64::MAX,
            Self::Days7 => 7,
            Self::Days30 => 30,
            Self::Days90 => 90,
        };
        now().saturating_sub(days * 86_400_000)
    }
}

#[derive(Clone, Debug)]
struct Revision {
    sequence: i64,
    state: String,
    digest: [u8; 32],
}
#[derive(Clone, Debug)]
pub struct Candidate {
    pub operation: i64,
    pub title: String,
    pub bytes: u64,
    changed: i64,
    revisions: Vec<Revision>,
    source: PathBuf,
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// Logical bytes of verified retained objects, excluding live results and trash.
    pub bytes: u64,
    pub protected: usize,
    pub issues: Vec<String>,
    pub candidates: Vec<Candidate>,
}
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub entries: Vec<Candidate>,
    pub bytes: u64,
}
impl Catalog {
    pub fn select(&self, retention: Retention) -> Selection {
        let entries: Vec<_> = self
            .candidates
            .iter()
            .filter(|entry| entry.changed < retention.cutoff())
            .cloned()
            .collect();
        let bytes = entries.iter().map(|entry| entry.bytes).sum();
        Selection { entries, bytes }
    }
}

struct Row {
    operation: i64,
    sequence: i64,
    title: String,
    state: String,
    changed: i64,
    data: Vec<u8>,
    available: bool,
    plan: Option<undo::Plan>,
}
impl Row {
    fn revision(&self) -> Revision {
        Revision {
            sequence: self.sequence,
            state: self.state.clone(),
            digest: Sha256::digest(&self.data).into(),
        }
    }
    fn retained(&self) -> Result<Vec<(PathBuf, snapshot::Snapshot)>> {
        let p = self.plan.as_ref().context("Нет проверяемого плана")?;
        ensure!(p.version == 1, "Неизвестный план хранения");
        let mut result = Vec::new();
        for (raw, expected, present, prefixes) in [
            (
                p.parking.as_deref(),
                Some(&p.expected),
                self.state == "undone",
                &[".basalgrim-undo-"][..],
            ),
            (
                p.backup.as_ref().map(|b| b.path.as_slice()),
                p.backup.as_ref().map(|b| &b.snapshot),
                self.state == "applied",
                &[".basalgrim-replaced-"][..],
            ),
            (
                p.info_parking.as_deref(),
                p.trash.as_ref().map(|t| &t.metadata),
                p.kind == "restore" && self.state == "applied"
                    || p.kind == "trash" && self.state == "undone",
                &[".basalgrim-undo-", ".basalgrim-restore-"][..],
            ),
        ] {
            let Some(raw) = raw else { continue };
            let held = path(raw);
            ensure!(
                held.is_absolute()
                    && !raw.contains(&0)
                    && held.components().all(|c| matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )),
                "Недопустимый путь хранения"
            );
            let parent = held.parent().context("Нет папки хранения")?;
            ensure!(
                parent.file_name().is_some_and(|n| prefixes
                    .iter()
                    .any(|prefix| n.as_encoded_bytes().starts_with(prefix.as_bytes()))),
                "Объект находится вне личного хранилища: {}",
                held.display()
            );
            if present {
                result.push((
                    held,
                    expected.context("Нет снимка сохранённых данных")?.clone(),
                ));
            } else {
                ensure!(
                    Identity::read(&held)?.is_none(),
                    "Неожиданный объект хранилища: {}",
                    held.display()
                );
            }
        }
        Ok(result)
    }
    fn references(&self) -> Vec<PathBuf> {
        let Some(p) = &self.plan else {
            return Vec::new();
        };
        [
            &Some(p.source.clone()),
            &Some(p.target.clone()),
            &p.parking,
            &p.backup.as_ref().map(|b| b.path.clone()),
            &p.info_parking,
        ]
        .into_iter()
        .filter_map(|p| p.as_deref().map(path))
        .collect()
    }
}

impl Database {
    fn storage_rows(&self, cancellation: &Cancellation) -> Result<Vec<Row>> {
        crate::fs::operation_checkpoint(cancellation)?;
        // Also inspect protected plans: a held tree may be referenced by another
        // operation. Never infer ownership from a directory-name prefix alone.
        let mut stmt = self.connection.prepare("SELECT u.operation_id,u.sequence,o.title,u.state,u.changed_ms,CASE WHEN length(u.data)<=33554432 THEN u.data END,u.issue IS NULL AND o.state NOT IN ('running','interrupted') AND NOT EXISTS(SELECT 1 FROM replay_runs r JOIN operations x ON x.id=r.operation_id WHERE r.original_id=o.id AND x.state='running') FROM undo_items u JOIN operations o ON o.id=u.operation_id WHERE u.state!='expired' ORDER BY u.operation_id,u.sequence LIMIT 10001")?;
        let mut result = Vec::new();
        let mut budget = 64 * 1024 * 1024usize;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
                r.get::<_, bool>(6)?,
            ))
        })? {
            crate::fs::operation_checkpoint(cancellation)?;
            ensure!(
                result.len() < 10000,
                "Хранилище содержит больше 10000 записей; автоматический выбор очистки недоступен"
            );
            let (operation, sequence, title, state, changed, data, available) = row?;
            let data = data.context("План хранения больше 32 MiB")?;
            budget = budget
                .checked_sub(data.len())
                .context("Планы хранения превышают 64 MiB")?;
            let plan = serde_json::from_slice::<Option<undo::Plan>>(&data)
                .ok()
                .flatten();
            result.push(Row {
                operation,
                sequence,
                title,
                state,
                changed,
                data,
                available,
                plan,
            });
        }
        Ok(result)
    }
    pub fn storage_catalog(&mut self, cancellation: &Cancellation) -> Result<Catalog> {
        crate::fs::operation_checkpoint(cancellation)?;
        recover(&mut self.connection, self._lock.path().parent().unwrap())?;
        let rows = self.storage_rows(cancellation)?;
        let references: Vec<_> = rows
            .iter()
            .flat_map(|r| r.references().into_iter().map(|p| (r.operation, p)))
            .collect();
        let mut groups = BTreeMap::<i64, Vec<&Row>>::new();
        for row in &rows {
            groups.entry(row.operation).or_default().push(row);
        }
        let mut catalog = Catalog::default();
        for (operation, group) in groups {
            crate::fs::operation_checkpoint(cancellation)?;
            let outcome = (|| -> Result<Candidate> {
                ensure!(
                    group
                        .iter()
                        .all(|r| r.available && matches!(r.state.as_str(), "applied" | "undone")),
                    "Незавершённая или непроверяемая операция"
                );
                let mut total = 0u64;
                let mut seen = BTreeSet::new();
                let mut source = None;
                for row in &group {
                    for (held, expected) in row.retained()? {
                        ensure!(
                            !seen
                                .iter()
                                .any(|p: &PathBuf| p.starts_with(&held) || held.starts_with(p)),
                            "Пересекающиеся сохранённые объекты"
                        );
                        seen.insert(held.clone());
                        source.get_or_insert_with(|| held.clone());
                        ensure!(
                            !references.iter().any(|(other, p)| *other != operation
                                && (p.starts_with(&held) || held.starts_with(p))),
                            "Данные используются другой записью истории: {}",
                            held.display()
                        );
                        crate::fs::RetainedRemoval::prepare(&held, &expected, cancellation)?;
                        total = total.saturating_add(
                            expected
                                .nodes
                                .iter()
                                .filter(|n| !n.identity.directory())
                                .map(|n| n.identity.size)
                                .sum::<u64>(),
                        );
                    }
                }
                let first = group[0];
                Ok(Candidate {
                    operation,
                    title: first.title.clone(),
                    bytes: total,
                    changed: group.iter().map(|r| r.changed).max().unwrap_or(0),
                    revisions: group.iter().map(|r| r.revision()).collect(),
                    source: source.unwrap_or_else(|| path(&first.plan.as_ref().unwrap().target)),
                })
            })();
            match outcome {
                Ok(candidate) => {
                    catalog.bytes = catalog.bytes.saturating_add(candidate.bytes);
                    catalog.candidates.push(candidate);
                }
                Err(error) => {
                    crate::fs::operation_checkpoint(cancellation)?;
                    catalog.protected += 1;
                    if catalog.issues.len() < 100 {
                        let retained: BTreeSet<_> = group
                            .iter()
                            .flat_map(|row| row.references())
                            .filter(|p| {
                                p.parent().and_then(Path::file_name).is_some_and(|n| {
                                    n.as_encoded_bytes().starts_with(b".basalgrim-")
                                })
                            })
                            .collect();
                        let paths = retained
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("; ");
                        catalog.issues.push(format!(
                            "#{operation}: {error:#}{}",
                            if paths.is_empty() {
                                String::new()
                            } else {
                                format!(" · {paths}")
                            }
                        ));
                    }
                }
            }
        }
        Ok(catalog)
    }
    pub fn cleanup_history(
        &mut self,
        selection: Selection,
        token: u64,
        cancellation: &Cancellation,
        notify: &mut dyn FnMut(&Progress),
    ) -> crate::fs::Report {
        let mut report = crate::fs::Report {
            total: selection.entries.len(),
            ..Default::default()
        };
        report.progress.items_total = report.total;
        report.progress.bytes_total = Some(selection.bytes);
        let outcome = (|| -> Result<()> {
            crate::fs::operation_checkpoint(cancellation)?;
            ensure!(
                !selection.entries.is_empty() && selection.entries.len() <= 10000,
                "Нет записей для очистки"
            );
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO operations(owner,runtime_id,title,started_ms,state,total) VALUES (?1,?2,'Очистка undo/redo',?3,'running',?4)",params![self.owner,token as i64,now(),report.total as i64])?;
            let id = tx.last_insert_rowid();
            let requested: Vec<Spec> = selection
                .entries
                .iter()
                .map(|c| Spec {
                    kind: "cleanup_history",
                    source: c.source.clone(),
                    target: None,
                })
                .collect();
            for (i, spec) in requested.iter().enumerate() {
                tx.execute("INSERT INTO items(operation_id,sequence,kind,source,state) VALUES (?1,?2,'cleanup_history',?3,'pending')",params![id,i as i64,bytes(&spec.source)])?;
            }
            tx.commit()?;
            let mut recorder = Recorder::new(self, id, requested);
            recorder.replaying();
            let work = (|| -> Result<()> {
                for (index, candidate) in selection.entries.iter().enumerate() {
                    crate::fs::operation_checkpoint(cancellation)?;
                    report.progress.phase = Phase::Verifying;
                    report.progress.current = Some(candidate.source.clone());
                    notify(&report.progress);
                    // Re-read the complete inventory to protect cross-record references
                    // created after the confirmation, including other live instances.
                    let rows = recorder.database.storage_rows(cancellation)?;
                    let group: Vec<_> = rows
                        .iter()
                        .filter(|r| r.operation == candidate.operation)
                        .collect();
                    ensure!(
                        same_revision(&group, candidate),
                        "Запись #{} изменилась после подтверждения",
                        candidate.operation
                    );
                    let mut objects = Vec::new();
                    for row in &group {
                        for (held, expected) in row.retained()? {
                            ensure!(
                                !rows
                                    .iter()
                                    .filter(|r| r.operation != candidate.operation)
                                    .flat_map(|r| r.references())
                                    .any(|p| p.starts_with(&held) || held.starts_with(&p)),
                                "Сохранённые данные теперь используются другой операцией"
                            );
                            crate::fs::RetainedRemoval::prepare(&held, &expected, cancellation)?;
                            objects.push((held, expected));
                        }
                    }
                    recorder.begin_item(index)?;
                    // Invalidate all inverses durably before the first deletion.
                    // The same transaction rechecks revisions and live replays.
                    {
                        let tx = recorder
                            .database
                            .connection
                            .transaction_with_behavior(TransactionBehavior::Immediate)?;
                        for revision in &candidate.revisions {
                            let current:(String,Vec<u8>,bool)=tx.query_row("SELECT u.state,u.data,u.issue IS NULL AND o.state NOT IN ('running','interrupted') AND NOT EXISTS(SELECT 1 FROM replay_runs r JOIN operations x ON x.id=r.operation_id WHERE r.original_id=o.id AND x.state='running') FROM undo_items u JOIN operations o ON o.id=u.operation_id WHERE u.operation_id=?1 AND u.sequence=?2",params![candidate.operation,revision.sequence],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                            ensure!(
                                current.2
                                    && current.0 == revision.state
                                    && <[u8; 32]>::from(Sha256::digest(&current.1))
                                        == revision.digest,
                                "Запись истории изменена в другом окне"
                            );
                        }
                        let count=tx.execute("UPDATE undo_items SET state='discarding',issue='Подтверждена очистка сохранённых данных',last_run=?2,changed_ms=?3 WHERE operation_id=?1",params![candidate.operation,id,now()])?;
                        ensure!(
                            count == candidate.revisions.len(),
                            "Состав записи истории изменился"
                        );
                        tx.commit()?;
                    }
                    let mut started = false;
                    let mut fully_removed = false;
                    let removed = (|| -> Result<()> {
                        crate::fs::operation_checkpoint(cancellation)?;
                        for (object_index, (held, expected)) in objects.iter().enumerate() {
                            let step =
                                recorder.before_step("history_delete", Some(held), None, false)?;
                            if !started {
                                crate::fs::operation_checkpoint(cancellation)?;
                            }
                            // Once disposal starts, finish this original operation before
                            // honouring cancellation, just like permanent trash deletion.
                            let cancel = if started {
                                Cancellation::default()
                            } else {
                                cancellation.clone()
                            };
                            let prepared =
                                crate::fs::RetainedRemoval::prepare(held, expected, &cancel)?;
                            report.progress.phase = Phase::RemovingSource;
                            // Other instances must commit an operation intent before
                            // touching data. Hold the writer gate during removal so a
                            // new operation cannot acquire a reference mid-disposal.
                            let tx = recorder
                                .database
                                .connection
                                .transaction_with_behavior(TransactionBehavior::Immediate)?;
                            ensure!(!tx.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE state='running' AND id!=?1)", [id], |r|r.get::<_,bool>(0))?, "Другое окно выполняет файловую операцию; очистка остановлена");
                            if !started {
                                crate::fs::operation_checkpoint(cancellation)?;
                            }
                            started = true;
                            let (removed, result) = prepared.remove(&mut report.progress, notify);
                            if removed && object_index + 1 == objects.len() {
                                fully_removed = true;
                            }
                            let committed = tx.commit().map_err(Into::into);
                            recorder.destructive_started();
                            let result = result.and(committed);
                            recorder.after_step(step, "history_delete", None, &result);
                            result?;
                            report.progress.bytes_done = report.progress.bytes_done.saturating_add(
                                expected
                                    .nodes
                                    .iter()
                                    .filter(|n| !n.identity.directory())
                                    .map(|n| n.identity.size)
                                    .sum::<u64>(),
                            );
                        }
                        fully_removed = true;
                        Ok(())
                    })();
                    if fully_removed {
                        // Count filesystem completion even if recording its result fails.
                        report.completed.push(candidate.source.clone());
                        report.progress.items_done = report.completed.len();
                        notify(&report.progress);
                        recorder.database.connection.execute("UPDATE undo_items SET state='expired',data=?4,issue='Данные undo/redo очищены пользователем',changed_ms=?2 WHERE operation_id=?1 AND state='discarding' AND last_run=?3",params![candidate.operation,now(),id,b"null".as_slice()])?;
                    } else if !started {
                        for revision in &candidate.revisions {
                            recorder.database.connection.execute("UPDATE undo_items SET state=?3,issue=NULL WHERE operation_id=?1 AND sequence=?2 AND state='discarding' AND last_run=?4",params![candidate.operation,revision.sequence,revision.state,id])?;
                        }
                    } else {
                        recorder.database.connection.execute("UPDATE undo_items SET state='discard_failed',issue='Очистка прервана: проверьте сохранённые пути' WHERE operation_id=?1 AND state='discarding' AND last_run=?2",params![candidate.operation,id])?;
                    }
                    let issue = removed.as_ref().err().map(|e| {
                        format!(
                            "{e:#}; {}",
                            objects
                                .iter()
                                .map(|(p, _)| p.display().to_string())
                                .collect::<Vec<_>>()
                                .join("; ")
                        )
                    });
                    let recorded = recorder.finish_item(fully_removed, false, issue.clone());
                    removed.map_err(|e| {
                        e.context(issue.unwrap_or_else(|| "Проверьте сохранённые пути".into()))
                    })?;
                    recorded?;
                    recorder.check()?;
                }
                Ok(())
            })();
            if let Err(error) = work {
                report.cancelled = crate::fs::operation_cancelled(&error);
                if !report.cancelled {
                    report.error = Some(format!("{error:#}"));
                }
            }
            report.progress.phase = Phase::Finished;
            notify(&report.progress);
            recorder.finish(&mut report);
            Ok(())
        })();
        if let Err(error) = outcome {
            report.cancelled = crate::fs::operation_cancelled(&error);
            if !report.cancelled {
                report.error = Some(format!("{error:#}"));
            }
        }
        report
    }
}
fn same_revision(rows: &[&Row], candidate: &Candidate) -> bool {
    rows.len() == candidate.revisions.len()
        && rows
            .iter()
            .zip(&candidate.revisions)
            .all(|(row, revision)| {
                row.available
                    && row.sequence == revision.sequence
                    && row.state == revision.state
                    && matches!(row.state.as_str(), "applied" | "undone")
                    && row.revision().digest == revision.digest
            })
}

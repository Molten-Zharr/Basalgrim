use super::*;
use crate::{
    fs::Report,
    operations::{Cancellation, Phase, Progress},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default)]
pub struct HistoryState {
    pub undo: usize,
    pub redo: usize,
    pub blocked: Option<String>,
    pub replay_of: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Backup {
    pub path: Vec<u8>,
    pub snapshot: snapshot::Snapshot,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Plan {
    pub version: u32,
    pub kind: String,
    pub source: Vec<u8>,
    pub target: Vec<u8>,
    pub expected: snapshot::Snapshot,
    pub origin: Option<snapshot::Snapshot>,
    pub backup: Option<Backup>,
    pub parking: Option<Vec<u8>>,
    pub trash: Option<trash::Locator>,
    pub info_parking: Option<Vec<u8>>,
}
#[derive(Deserialize)]
struct ReplayPaths {
    kind: String,
    source: Vec<u8>,
    target: Vec<u8>,
    parking: Option<Vec<u8>>,
}
pub(super) fn parking(path: &Path, database: &Database) -> Result<PathBuf> {
    let mut parent = path
        .parent()
        .context("Нельзя отменять операции над корнем")?;
    // Retained children belong outside a tracked parent, so its own undo can
    // still see the original tree instead of our redo storage.
    for ancestor in path.ancestors().skip(1) {
        let tracked:bool=database.connection.query_row("SELECT EXISTS(SELECT 1 FROM items i JOIN undo_items u ON u.operation_id=i.operation_id AND u.sequence=i.sequence WHERE i.destination=?1 AND u.state='applied')",[bytes(ancestor)],|r|r.get(0))?;
        if tracked && let Some(outer) = ancestor.parent() {
            parent = outer;
        }
    }
    ensure!(
        snapshot::mount(parent)? == snapshot::mount(path)?,
        "Нет доступного места для отмены на той же файловой системе"
    );
    let directory = tempfile::Builder::new()
        .prefix(".basalgrim-undo-")
        .tempdir_in(parent)?
        .keep();
    Ok(directory.join("item"))
}
impl Database {
    pub fn history_state(&self, id: i64) -> Result<HistoryState> {
        use rusqlite::OptionalExtension;
        let mut state = HistoryState {
            replay_of: self
                .connection
                .query_row(
                    "SELECT original_id FROM replay_runs WHERE operation_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?,
            ..Default::default()
        };
        let id = state.replay_of.unwrap_or(id);
        let operation_state: String =
            self.connection
                .query_row("SELECT state FROM operations WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        if matches!(operation_state.as_str(), "running" | "interrupted") {
            state.blocked = Some(
                if operation_state == "running" {
                    "Операция ещё выполняется"
                } else {
                    "Прерванная операция требует ручной проверки"
                }
                .into(),
            );
        }
        let mut stmt=self.connection.prepare("SELECT state,issue,COUNT(*) FROM undo_items WHERE operation_id=?1 GROUP BY state,issue")?;
        for row in stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })? {
            let (status, issue, count) = row?;
            if let Some(issue) = issue {
                state.blocked = Some(issue);
            }
            match status.as_str() {
                "applied" => state.undo += count as usize,
                "undone" => state.redo += count as usize,
                _ => {
                    state.blocked = Some("Результат отмены/повтора требует ручной проверки".into())
                }
            }
        }
        let completed: u32 = self.connection.query_row(
            "SELECT COUNT(*) FROM items WHERE operation_id=?1 AND state='done'",
            [id],
            |r| r.get(0),
        )?;
        let tracked: u32 = self.connection.query_row(
            "SELECT COUNT(*) FROM undo_items WHERE operation_id=?1",
            [id],
            |r| r.get(0),
        )?;
        if completed > tracked {
            state.blocked =
                Some("Для этой записи не сохранён проверяемый снимок; отмена недоступна".into());
        }
        if state.blocked.is_some() {
            state.undo = 0;
            state.redo = 0;
        }
        Ok(state)
    }
    pub fn replay(
        &mut self,
        original: i64,
        redo: bool,
        token: u64,
        cancellation: &Cancellation,
        notify: &mut dyn FnMut(&Progress),
    ) -> Report {
        let mut report = Report::default();
        let result = (|| -> Result<()> {
            crate::fs::operation_checkpoint(cancellation)?;
            let state = self.history_state(original)?;
            let original = state.replay_of.unwrap_or(original);
            ensure!(
                state.blocked.is_none(),
                "{}",
                state.blocked.as_deref().unwrap_or_default()
            );
            ensure!(
                if redo { state.redo > 0 } else { state.undo > 0 },
                "Нет доступных элементов для {}",
                if redo {
                    "повтора"
                } else {
                    "отмены"
                }
            );
            let status = if redo { "undone" } else { "applied" };
            let order = if redo { "ASC" } else { "DESC" };
            let rows: Vec<(usize, Vec<u8>)> = {
                let mut stmt=self.connection.prepare(&format!("SELECT sequence,CASE WHEN length(data)<=33554432 THEN data END FROM undo_items WHERE operation_id=?1 AND state=?2 AND issue IS NULL ORDER BY sequence {order} LIMIT 100001"))?;
                let records = stmt.query_map(params![original, status], |r| {
                    Ok((
                        r.get::<_, u32>(0)? as usize,
                        r.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                })?;
                let mut paths = Vec::new();
                for record in records {
                    let (sequence, data) = record?;
                    let plan: ReplayPaths = serde_json::from_slice::<Option<ReplayPaths>>(
                        &data.context("План отмены слишком большой")?,
                    )?
                    .context("Нет плана отмены")?;
                    let source = if !redo {
                        plan.target
                    } else if matches!(
                        plan.kind.as_str(),
                        "copy" | "duplicate" | "create" | "mkdir"
                    ) {
                        plan.parking.context("Нет сохранённых данных для повтора")?
                    } else {
                        plan.source
                    };
                    paths.push((sequence, source));
                }
                paths
            };
            ensure!(
                rows.len() <= 100_000,
                "Слишком много элементов для одного отката"
            );
            report.total = rows.len();
            report.progress.items_total = report.total;
            let tx = self
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO operations(owner,runtime_id,title,started_ms,state,total) VALUES (?1,?2,?3,?4,'running',?5)",params![self.owner,token as i64,if redo{"Повтор операции"}else{"Отмена операции"},now(),report.total as i64])?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO replay_runs(operation_id,original_id,redo) VALUES (?1,?2,?3)",
                params![id, original, redo],
            )?;
            let requested: Vec<Spec> = rows
                .iter()
                .map(|(_, source)| Spec {
                    kind: if redo { "redo" } else { "undo" },
                    source: path(source),
                    target: None,
                })
                .collect();
            for (sequence, spec) in requested.iter().enumerate() {
                tx.execute("INSERT INTO items(operation_id,sequence,kind,source,state) VALUES (?1,?2,?3,?4,'pending')",params![id,sequence as i64,spec.kind,bytes(&spec.source)])?;
            }
            tx.commit()?;
            let mut recorder = Recorder::new(self, id, requested);
            recorder.replaying();
            let work = (|| -> Result<()> {
                for (index, (sequence, source)) in rows.into_iter().enumerate() {
                    crate::fs::operation_checkpoint(cancellation)?;
                    report.progress.phase = Phase::Verifying;
                    report.progress.current = Some(path(&source));
                    notify(&report.progress);
                    let data:Option<Vec<u8>>=recorder.database.connection.query_row("SELECT CASE WHEN length(data)<=33554432 THEN data END FROM undo_items WHERE operation_id=?1 AND sequence=?2 AND state=?3 AND issue IS NULL",params![original,sequence as i64,status],|r|r.get(0))?;
                    let mut plan: Plan = serde_json::from_slice::<Option<Plan>>(
                        &data.context("Снимок отмены слишком большой")?,
                    )?
                    .context("План отмены недоступен")?;
                    plan.validate(redo, cancellation)?;
                    if !redo
                        && matches!(
                            plan.kind.as_str(),
                            "copy" | "duplicate" | "create" | "mkdir"
                        )
                        && plan.parking.is_none()
                    {
                        plan.parking =
                            Some(bytes(&parking(&path(&plan.target), recorder.database)?));
                    }
                    if !redo && plan.kind == "trash" && plan.info_parking.is_none() {
                        plan.info_parking = Some(bytes(&parking(
                            &path(&plan.trash.as_ref().context("Нет адреса корзины")?.info),
                            recorder.database,
                        )?));
                    }
                    recorder.begin_item(index)?;
                    let changed=recorder.database.connection.execute("UPDATE undo_items SET state='busy',data=?4,last_run=?5 WHERE operation_id=?1 AND sequence=?2 AND state=?3 AND issue IS NULL",params![original,sequence as i64,status,serde_json::to_vec(&plan)?,id])?;
                    ensure!(changed == 1, "Состояние операции изменилось в другом окне");
                    crate::fs::operation_checkpoint(cancellation).inspect_err(|_| {
                        let _=recorder.database.connection.execute("UPDATE undo_items SET state=?3 WHERE operation_id=?1 AND sequence=?2 AND state='busy' AND last_run=?4",params![original,sequence as i64,status,id]);
                    })?;
                    let mut execution = Execution {
                        cancellation,
                        notify,
                        recorder: &mut recorder,
                        report: &mut report,
                        changed: false,
                        complete: false,
                        moves: Vec::new(),
                    };
                    let mut outcome = plan.execute(redo, &mut execution);
                    if outcome.is_err()
                        && let Err(recovery) = execution.rollback()
                    {
                        outcome = Err(outcome.unwrap_err().context(format!(
                            "Восстановление незавершённого шага не удалось: {recovery:#}"
                        )));
                    }
                    let changed = execution.changed;
                    let complete = execution.complete;
                    if complete {
                        report.completed.push(path(&source));
                        report.progress.items_done = report.completed.len();
                    }
                    let next = if outcome.is_ok() {
                        if redo { "applied" } else { "undone" }
                    } else if changed {
                        "uncertain"
                    } else {
                        status
                    };
                    let issue = if changed && outcome.is_err() {
                        Some("Откат не завершён; сохранённые пути требуют проверки")
                    } else {
                        None
                    };
                    let recorded = (|| -> Result<usize> {
                        let tx = recorder
                            .database
                            .connection
                            .transaction_with_behavior(TransactionBehavior::Immediate)?;
                        if outcome.is_ok()
                            && !redo
                            && matches!(plan.kind.as_str(), "move" | "rename")
                            && let Some(origin) = &plan.origin
                        {
                            lineage::rebase(
                                &tx,
                                original,
                                &path(&plan.source),
                                origin,
                                &plan.expected,
                            )?;
                        }
                        let data = serde_json::to_vec(&plan)?;
                        ensure!(
                            data.len() <= 32 * 1024 * 1024,
                            "Обновлённый план отмены слишком большой"
                        );
                        let changed=tx.execute("UPDATE undo_items SET state=?3,data=?4,issue=?5,changed_ms=?6 WHERE operation_id=?1 AND sequence=?2 AND state='busy' AND last_run=?7",params![original,sequence as i64,next,data,issue,now(),id])?;
                        tx.commit()?;
                        Ok(changed)
                    })();
                    let item_issue = outcome.as_ref().err().map(|e| format!("{e:#}"));
                    let item_recorded = recorder.finish_item(complete, false, item_issue);
                    if let Err(error) = outcome {
                        if changed {
                            return Err(error.context(format!(
                                "Данные сохранены. Проверьте {}",
                                plan.locations()
                            )));
                        }
                        return Err(error);
                    }
                    ensure!(
                        recorded? == 1,
                        "Не удалось подтвердить состояние отмены/повтора"
                    );
                    item_recorded?;
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
        if let Err(error) = result {
            report.cancelled = crate::fs::operation_cancelled(&error);
            if !report.cancelled {
                report.error = Some(format!("{error:#}"));
            }
        }
        report
    }
}

impl Plan {
    fn locations(&self) -> String {
        [
            &Some(self.source.clone()),
            &Some(self.target.clone()),
            &self.parking,
            &self.backup.as_ref().map(|b| b.path.clone()),
            &self.info_parking,
        ]
        .into_iter()
        .filter_map(|p| p.as_deref())
        .map(|p| path(p).display().to_string())
        .collect::<Vec<_>>()
        .join("; ")
    }
    fn validate(&self, redo: bool, cancellation: &Cancellation) -> Result<()> {
        ensure!(
            self.version == 1 && !self.expected.nodes.is_empty(),
            "Неизвестный план отмены"
        );
        ensure!(
            matches!(
                self.kind.as_str(),
                "copy" | "duplicate" | "create" | "mkdir" | "move" | "rename" | "trash"
            ),
            "Неизвестная команда отмены"
        );
        for p in [
            &Some(self.source.clone()),
            &Some(self.target.clone()),
            &self.parking,
            &self.backup.as_ref().map(|b| b.path.clone()),
            &self.info_parking,
        ]
        .into_iter()
        .filter_map(|p| p.as_deref())
        {
            let p = path(p);
            ensure!(
                p.is_absolute()
                    && p.file_name().is_some()
                    && !bytes(&p).contains(&0)
                    && !p
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)),
                "Недопустимый путь плана отмены"
            );
        }
        let current = if redo {
            if matches!(
                self.kind.as_str(),
                "copy" | "duplicate" | "create" | "mkdir"
            ) {
                path(
                    self.parking
                        .as_ref()
                        .context("Нет сохранённой копии для повтора")?,
                )
            } else {
                path(&self.source)
            }
        } else {
            path(&self.target)
        };
        check(&current, &self.expected, cancellation)?;
        if let Some(backup) = &self.backup {
            check(
                &if redo {
                    path(&self.target)
                } else {
                    path(&backup.path)
                },
                &backup.snapshot,
                cancellation,
            )?;
            if redo {
                absent(&path(&backup.path))?;
            }
        } else if redo && self.kind != "trash" {
            absent(&path(&self.target))?;
        }
        if !redo {
            if matches!(self.kind.as_str(), "move" | "rename" | "trash") {
                absent(&path(&self.source))?;
            }
            if let Some(parking) = &self.parking {
                absent(&path(parking))?;
            }
            if self.kind == "trash" {
                let locator = self.trash.as_ref().context("Нет метаданных корзины")?;
                ensure!(
                    locator.data == self.target,
                    "Адрес корзины не соответствует снимку"
                );
                check(&path(&locator.info), &locator.metadata, cancellation)?;
            }
        }
        Ok(())
    }
    fn execute(&mut self, redo: bool, execution: &mut Execution<'_, '_>) -> Result<()> {
        let original = self.expected.clone();
        if redo && matches!(self.kind.as_str(), "move" | "rename") {
            self.origin = Some(original.clone());
        }
        if self.kind == "trash" {
            let source = path(&self.source);
            if redo {
                let search = trash::Search::new(&source)?;
                execution
                    .recorder
                    .set_cancellation(execution.cancellation.clone());
                execution.recorder.protect(&source, &original);
                let step = execution
                    .recorder
                    .before_step("trash", Some(&source), None, false)?;
                crate::fs::operation_checkpoint(execution.cancellation)?;
                let result = crate::fs::trash_one(&source);
                execution.recorder.after_step(step, "trash", None, &result);
                result?;
                execution.changed = true;
                execution.complete = true;
                let (locator, actual) = search.locate(&source, &original, &Default::default())?;
                self.target = locator.data.clone();
                self.expected = actual;
                self.trash = Some(locator);
                self.info_parking = None;
            } else {
                let locator = self.trash.as_mut().context("Нет адреса корзины")?;
                let info = path(
                    self.info_parking
                        .as_ref()
                        .context("Нет резервного пути метаданных")?,
                );
                self.expected =
                    execution.move_one(&path(&self.target), &source, &original, false)?;
                locator.metadata =
                    execution.move_one(&path(&locator.info), &info, &locator.metadata, true)?;
            }
        } else {
            let copy = matches!(
                self.kind.as_str(),
                "copy" | "duplicate" | "create" | "mkdir"
            );
            let holding = if copy {
                path(
                    self.parking
                        .as_ref()
                        .context("Нет места для сохранённых данных")?,
                )
            } else {
                path(&self.source)
            };
            let target = path(&self.target);
            if redo {
                if let Some(backup) = &mut self.backup {
                    backup.snapshot = execution.move_one(
                        &target,
                        &path(&backup.path),
                        &backup.snapshot,
                        false,
                    )?;
                }
                self.expected = execution.move_one(&holding, &target, &original, true)?;
            } else {
                self.expected =
                    execution.move_one(&target, &holding, &original, self.backup.is_none())?;
                if let Some(backup) = &mut self.backup {
                    backup.snapshot =
                        execution.move_one(&path(&backup.path), &target, &backup.snapshot, true)?;
                }
            }
        }
        execution.recorder.target(&if redo {
            path(&self.target)
        } else if self.kind == "trash" || matches!(self.kind.as_str(), "move" | "rename") {
            path(&self.source)
        } else {
            path(self.parking.as_ref().unwrap())
        });
        Ok(())
    }
}
struct Execution<'a, 'db> {
    cancellation: &'a Cancellation,
    notify: &'a mut dyn FnMut(&Progress),
    recorder: &'a mut Recorder<'db>,
    report: &'a mut Report,
    changed: bool,
    complete: bool,
    moves: Vec<(PathBuf, PathBuf, Option<Identity>)>,
}
impl Execution<'_, '_> {
    fn move_one(
        &mut self,
        source: &Path,
        target: &Path,
        expected: &snapshot::Snapshot,
        last: bool,
    ) -> Result<snapshot::Snapshot> {
        let control = if self.changed {
            Cancellation::default()
        } else {
            self.cancellation.clone()
        };
        let before = snapshot::Snapshot::read(source, &control)?;
        ensure!(
            expected.unchanged(&before),
            "Файл изменился перед отменой/повтором: {}",
            source.display()
        );
        self.recorder.set_cancellation(control.clone());
        self.recorder.protect(source, &before);
        absent(target)?;
        let base = self.report.progress.clone();
        let notify = &mut self.notify;
        let mut merged_notify = |progress: &Progress| {
            let mut merged = progress.clone();
            merged.items_total = base.items_total;
            merged.items_done = base.items_done;
            merged.bytes_done = merged.bytes_done.saturating_add(base.bytes_done);
            merged.entries_done = merged.entries_done.saturating_add(base.entries_done);
            notify(&merged);
        };
        let moved = crate::fs::relocate_recorded(
            source,
            target,
            &control,
            &mut merged_notify,
            self.recorder,
        );
        self.report.progress.bytes_done = self
            .report
            .progress
            .bytes_done
            .saturating_add(moved.progress.bytes_done);
        self.report.progress.entries_done = self
            .report
            .progress
            .entries_done
            .saturating_add(moved.progress.entries_done);
        self.report.retained_copies.extend(moved.retained_copies);
        self.report
            .destination_recoveries
            .extend(moved.destination_recoveries);
        if !moved.completed.is_empty() {
            self.changed = true;
            self.complete = last;
            self.moves.push((
                source.into(),
                target.into(),
                Identity::read(target).ok().flatten(),
            ));
        }
        if !self.report.retained_copies.is_empty() {
            self.changed = true;
        }
        if let Some(error) = moved.error {
            anyhow::bail!("{error}");
        }
        if moved.cancelled {
            crate::fs::operation_checkpoint(self.cancellation)?;
            anyhow::bail!("Отмена шага");
        }
        let actual = snapshot::Snapshot::read(target, &Default::default())?;
        let copied = before.nodes[0].identity.device != actual.nodes[0].identity.device;
        ensure!(
            before.relocated(&actual, copied),
            "Данные изменились при отмене/повторе: {}",
            target.display()
        );
        Ok(actual)
    }
    fn rollback(&mut self) -> Result<()> {
        // Only unfinished logical items are restored. Published, completed undo
        // results survive a later recording error just like ordinary operations.
        if self.complete || !self.report.retained_copies.is_empty() {
            return Ok(());
        }
        for (source, target, expected) in self.moves.iter().rev() {
            let same = expected.as_ref().is_some_and(|e| {
                Identity::read(target)
                    .ok()
                    .flatten()
                    .is_some_and(|a| a.device == e.device && a.inode == e.inode)
            });
            ensure!(same, "Объект для возврата изменился: {}", target.display());
            tracked_rename(
                Some(self.recorder),
                "replay_restore",
                target,
                source,
                true,
                None,
            )?;
        }
        if !self.moves.is_empty() {
            self.changed = false;
        }
        Ok(())
    }
}
fn check(path: &Path, expected: &snapshot::Snapshot, cancellation: &Cancellation) -> Result<()> {
    ensure!(
        expected.unchanged(
            &snapshot::Snapshot::read(path, cancellation)
                .with_context(|| format!("Не удалось проверить {}", path.display()))?
        ),
        "Файл или содержимое папки изменились; отмена/повтор остановлены: {}",
        path.display()
    );
    Ok(())
}
fn absent(path: &Path) -> Result<()> {
    ensure!(
        Identity::read(path)?.is_none(),
        "Цель уже существует; отмена/повтор ничего не перезаписывают: {}",
        path.display()
    );
    Ok(())
}

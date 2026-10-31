use super::*;
use crate::fs::Report;

struct Current {
    index: usize,
    spec: Spec,
    before: Option<Identity>,
    target: Option<PathBuf>,
    published: bool,
    replaced: bool,
}
pub struct Recorder<'a> {
    database: &'a mut Database,
    pub id: i64,
    requested: Vec<Spec>,
    current: Option<Current>,
    problem: Option<String>,
    started: Instant,
}
impl<'a> Recorder<'a> {
    pub(super) fn new(database: &'a mut Database, id: i64, requested: Vec<Spec>) -> Self {
        Self {
            database,
            id,
            requested,
            current: None,
            problem: None,
            started: Instant::now(),
        }
    }
    pub fn check(&self) -> Result<()> {
        if let Some(error) = &self.problem {
            anyhow::bail!("Журнал: {error}");
        }
        Ok(())
    }
    fn remember(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.problem = Some(format!("{error:#}"));
        }
    }
    pub fn begin_item(&mut self, index: usize) -> Result<()> {
        self.check()?;
        let spec = self
            .requested
            .get(index)
            .context("Нет элемента журнала")?
            .clone();
        let before = Identity::read(&spec.source)?;
        self.database.connection.execute(
            "UPDATE items SET state='running',before=?3 WHERE operation_id=?1 AND sequence=?2",
            params![self.id, index as i64, serde_json::to_vec(&before)?],
        )?;
        self.current = Some(Current {
            index,
            target: spec.target.clone(),
            spec,
            before,
            published: false,
            replaced: false,
        });
        Ok(())
    }
    pub fn before_step(
        &mut self,
        kind: &str,
        source: Option<&Path>,
        destination: Option<&Path>,
        recovery: bool,
    ) -> Result<Option<i64>> {
        let result = (|| {
            self.check()?;
            let current = self
                .current
                .as_mut()
                .context("Нет текущего элемента журнала")?;
            if kind == "publish" {
                current.target = destination.map(Path::to_path_buf);
            }
            if kind == "replace_claim" {
                current.replaced = true;
            }
            let before_source = source.map(Identity::read).transpose()?.flatten();
            let before_destination = destination.map(Identity::read).transpose()?.flatten();
            let tx = self
                .database
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO steps(operation_id,item,kind,source,destination,before_source,before_destination,state) VALUES (?1,?2,?3,?4,?5,?6,?7,'planned')",params![self.id,current.index as i64,kind,source.map(bytes),destination.map(bytes),serde_json::to_vec(&before_source)?,serde_json::to_vec(&before_destination)?])?;
            if kind == "publish" {
                tx.execute(
                    "UPDATE items SET destination=?3 WHERE operation_id=?1 AND sequence=?2",
                    params![self.id, current.index as i64, destination.map(bytes)],
                )?;
            }
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(id)
        })();
        match result {
            Ok(id) => Ok(Some(id)),
            Err(error) if recovery => {
                self.remember(Err(error));
                Ok(None)
            }
            Err(error) => Err(error.context("Не удалось записать намерение; изменение не начато")),
        }
    }
    pub fn after_step(
        &mut self,
        id: Option<i64>,
        kind: &str,
        destination: Option<&Path>,
        result: &Result<()>,
    ) {
        if kind == "publish"
            && result.is_ok()
            && let Some(current) = &mut self.current
        {
            current.published = true;
        }
        if let Some(id) = id {
            let recorded = (|| {
                let after = destination.map(Identity::read).transpose()?.flatten();
                self.database.connection.execute(
                    "UPDATE steps SET state=?2,after_destination=?3,issue=?4 WHERE id=?1",
                    params![
                        id,
                        if result.is_ok() { "done" } else { "failed" },
                        serde_json::to_vec(&after)?,
                        result.as_ref().err().map(|e| format!("{e:#}"))
                    ],
                )?;
                Ok(())
            })();
            self.remember(recorded);
        }
    }
    pub fn finish_item(
        &mut self,
        completed: bool,
        skipped: bool,
        issue: Option<String>,
    ) -> Result<()> {
        let Some(current) = self.current.take() else {
            return Ok(());
        };
        let after = current
            .target
            .as_deref()
            .map(Identity::read)
            .transpose()?
            .flatten();
        let inverse = if completed {
            let blocked = if current.replaced {
                Some(
                    "Замена прежней цели: требуется ручная проверка сохранности старых данных"
                        .into(),
                )
            } else if current.spec.kind == "trash" {
                Some("Для восстановления из корзины нужен точный адрес элемента".into())
            } else if after.as_ref().is_some_and(Identity::directory)
                && current.spec.kind != "mkdir"
            {
                Some("Не записан полный снимок содержимого папки".into())
            } else if after.is_none() {
                Some("Идентичность результата не подтверждена".into())
            } else {
                None
            };
            Some(Inverse {
                action: match current.spec.kind {
                    "move" | "rename" => "move_back",
                    "trash" => "restore_trash",
                    "mkdir" => "remove_empty_directory",
                    _ => "remove_created",
                }
                .into(),
                from: current.target.as_deref().map(bytes),
                to: matches!(current.spec.kind, "move" | "rename" | "trash")
                    .then(|| bytes(&current.spec.source)),
                expected: after.clone(),
                original: current.before.clone(),
                blocked,
            })
        } else {
            None
        };
        self.database.connection.execute("UPDATE items SET state=?3,destination=?4,after=?5,inverse=?6,issue=?7 WHERE operation_id=?1 AND sequence=?2",params![self.id,current.index as i64,if completed{"done"}else if skipped{"skipped"}else if current.published{"partial"}else{"failed"},current.target.as_deref().map(bytes),serde_json::to_vec(&after)?,inverse.as_ref().map(serde_json::to_vec).transpose()?,issue])?;
        self.check()
    }
    pub fn finish(&mut self, report: &mut Report) {
        if let Some(error) = &self.problem {
            append_error(report, &format!("Журнал: {error}"));
        }
        if self.current.is_some() {
            let result = self.finish_item(false, false, report.error.clone());
            if let Err(e) = result {
                append_error(report, &format!("Журнал: {e:#}"));
            }
        }
        let result: Result<()> = (|| {
            let summary = Summary::new(report);
            let tx = self
                .database
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("UPDATE operations SET finished_ms=?2,elapsed_ms=?3,state=?4,summary=?5 WHERE id=?1",params![self.id,now(),self.started.elapsed().as_millis().min(i64::MAX as u128) as i64,if report.error.is_some(){"failed"}else if report.cancelled{"cancelled"}else{"done"},serde_json::to_vec(&summary)?])?;
            tx.execute("UPDATE items SET state='uncertain',issue='Не удалось подтвердить результат шага' WHERE operation_id=?1 AND state='running'",[self.id])?;
            tx.execute(
                "UPDATE steps SET state='uncertain' WHERE operation_id=?1 AND state='planned'",
                [self.id],
            )?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            append_error(
                report,
                &format!("Не удалось завершить запись журнала: {error:#}"),
            );
        }
        let items = match self.database.items(self.id) {
            Ok(items) => items,
            Err(error) => {
                append_error(
                    report,
                    &format!("Не удалось прочитать запись журнала: {error:#}"),
                );
                Vec::new()
            }
        };
        report.journal = Some(Receipt { id: self.id, items });
    }
}
pub fn append_error(report: &mut Report, error: &str) {
    if report
        .error
        .as_deref()
        .is_some_and(|old| old.contains(error))
    {
        return;
    }
    report.error = Some(match report.error.take() {
        Some(old) => format!("{old} · {error}"),
        None => error.into(),
    });
}

pub fn tracked_rename(
    mut recorder: Option<&mut Recorder<'_>>,
    kind: &str,
    source: &Path,
    destination: &Path,
    recovery: bool,
    cancellation: Option<&crate::operations::Cancellation>,
) -> Result<()> {
    let id = match recorder.as_mut() {
        Some(r) => r.before_step(kind, Some(source), Some(destination), recovery)?,
        None => None,
    };
    let result = cancellation
        .map_or(Ok(()), crate::fs::operation_checkpoint)
        .and_then(|()| crate::fs::rename_new(source, destination));
    if let Some(recorder) = recorder {
        recorder.after_step(id, kind, Some(destination), &result);
    }
    result
}

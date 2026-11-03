use super::*;
use crate::fs::Report;

struct Current {
    index: usize,
    spec: Spec,
    before: Option<Identity>,
    target: Option<PathBuf>,
    published: bool,
    replaced: bool,
    expected: Option<snapshot::Snapshot>,
    origin: Option<snapshot::Snapshot>,
    backup: Option<undo::Backup>,
    trash: Option<trash::Search>,
    undo_issue: Option<String>,
    restore: Option<(trash::Locator, Vec<u8>)>,
}
pub struct Recorder<'a> {
    pub(super) database: &'a mut Database,
    pub id: i64,
    requested: Vec<Spec>,
    current: Option<Current>,
    problem: Option<String>,
    started: Instant,
    cancellation: crate::operations::Cancellation,
    replaying: bool,
    guard: Option<(PathBuf, snapshot::Snapshot)>,
}
impl<'a> Recorder<'a> {
    pub(crate) fn restore_metadata(
        &mut self,
        entry: &crate::trash::Entry,
        parked: &Path,
    ) -> Result<()> {
        let metadata = snapshot::Snapshot::read(&entry.info, &self.cancellation)?;
        let source = snapshot::Snapshot::read(&entry.data, &self.cancellation)?;
        entry.validate(&self.cancellation)?;
        self.guard = Some((entry.data.clone(), source.clone()));
        let current = self
            .current
            .as_mut()
            .context("Нет текущего элемента журнала")?;
        current.origin = Some(source.clone());
        current.expected = Some(source);
        current.restore = Some((
            trash::Locator {
                data: bytes(&entry.data),
                info: bytes(&entry.info),
                metadata,
            },
            bytes(parked),
        ));
        Ok(())
    }
    pub(super) fn new(database: &'a mut Database, id: i64, requested: Vec<Spec>) -> Self {
        Self {
            database,
            id,
            requested,
            current: None,
            problem: None,
            started: Instant::now(),
            cancellation: Default::default(),
            replaying: false,
            guard: None,
        }
    }
    pub(crate) fn destructive_started(&mut self) {
        if let Some(current) = &mut self.current {
            current.published = true;
        }
    }
    pub fn set_cancellation(&mut self, cancellation: crate::operations::Cancellation) {
        self.cancellation = cancellation;
    }
    pub(super) fn replaying(&mut self) {
        self.replaying = true;
    }
    pub(super) fn protect(&mut self, path: &Path, expected: &snapshot::Snapshot) {
        self.guard = Some((path.into(), expected.clone()));
    }
    pub(super) fn target(&mut self, path: &Path) {
        if let Some(current) = &mut self.current {
            current.target = Some(path.into());
        }
    }
    pub(crate) fn published_mode(&mut self, mode: u32) {
        if let Some(current) = &mut self.current
            && let Some(expected) = &mut current.expected
            && let Some(root) = expected.nodes.first_mut()
        {
            root.identity.mode = mode;
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
        let irreversible = matches!(spec.kind, "delete_trash" | "cleanup_history");
        self.current = Some(Current {
            index,
            target: spec.target.clone(),
            spec,
            before,
            published: false,
            replaced: false,
            expected: None,
            origin: None,
            backup: None,
            trash: None,
            undo_issue: irreversible.then(|| "Безвозвратное удаление не отменяется".into()),
            restore: None,
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
            if !self.replaying
                && matches!(kind, "publish" | "replace_claim" | "trash")
                && let Some(source) = source
            {
                match snapshot::Snapshot::read(source, &self.cancellation) {
                    Ok(snapshot) if kind == "replace_claim" => {
                        current.backup = Some(undo::Backup {
                            path: bytes(destination.context("Нет резервного пути")?),
                            snapshot,
                        });
                    }
                    Ok(snapshot) => {
                        if matches!(current.spec.kind, "move" | "rename")
                            && source == current.spec.source
                            && current.origin.is_none()
                        {
                            current.origin = Some(snapshot.clone());
                        }
                        current.expected = Some(snapshot);
                    }
                    Err(e) => current.undo_issue = Some(format!("Отмена недоступна: {e:#}")),
                }
                crate::fs::operation_checkpoint(&self.cancellation)?;
                if kind == "trash" {
                    match trash::Search::new(source) {
                        Ok(search) => current.trash = Some(search),
                        Err(e) => {
                            current.undo_issue =
                                Some(format!("Точный адрес корзины недоступен: {e:#}"))
                        }
                    }
                }
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
            if !recovery
                && let Some((path, expected)) = &self.guard
                && source == Some(path.as_path())
                && matches!(kind, "publish" | "claim_source" | "trash")
            {
                ensure!(
                    expected.unchanged(&snapshot::Snapshot::read(path, &self.cancellation)?),
                    "Файл изменился перед отменой/повтором: {}",
                    path.display()
                );
            }
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
        let Some(mut current) = self.current.take() else {
            return Ok(());
        };
        let mut locator = None;
        if completed
            && !self.replaying
            && current.spec.kind == "trash"
            && let (Some(search), Some(expected)) = (&current.trash, &current.expected)
        {
            match search.locate(&current.spec.source, expected, &Default::default()) {
                Ok((found, actual)) => {
                    current.target = Some(path(&found.data));
                    current.expected = Some(actual);
                    locator = Some(found);
                }
                Err(e) => current.undo_issue = Some(format!("{e:#}")),
            }
        }
        let plan = if completed && !self.replaying {
            let prepared: Result<undo::Plan> = (|| {
                ensure!(
                    current.undo_issue.is_none(),
                    "{}",
                    current.undo_issue.as_deref().unwrap_or_default()
                );
                let target = current.target.as_deref().context("Нет адреса результата")?;
                let actual = snapshot::Snapshot::read(target, &Default::default())?;
                if current.spec.kind == "create" || current.spec.kind == "mkdir" {
                    ensure!(
                        actual.nodes.len() == 1 && actual.nodes[0].identity.size == 0
                            || current.spec.kind == "mkdir" && actual.nodes.len() == 1,
                        "Созданный файл/папка уже изменён"
                    );
                } else {
                    let expected = current
                        .expected
                        .as_ref()
                        .context("Не сохранён снимок перед изменением")?;
                    ensure!(
                        expected.relocated(&actual, false),
                        "Результат изменился до записи снимка отмены"
                    );
                }
                let backup = if let Some(mut backup) = current.backup.take() {
                    let actual =
                        snapshot::Snapshot::read(&path(&backup.path), &Default::default())?;
                    ensure!(
                        backup.snapshot.relocated(&actual, false),
                        "Резервная копия прежней цели изменилась"
                    );
                    backup.snapshot = actual;
                    Some(backup)
                } else {
                    None
                };
                ensure!(
                    !current.replaced || backup.is_some(),
                    "Не сохранена прежняя цель замены"
                );
                let plan = undo::Plan {
                    version: 1,
                    kind: current.spec.kind.into(),
                    source: bytes(&current.spec.source),
                    target: bytes(target),
                    expected: actual,
                    origin: current.origin.take(),
                    backup,
                    parking: None,
                    trash: if let Some((mut locator, parked)) = current.restore.clone() {
                        let actual = snapshot::Snapshot::read(&path(&parked), &Default::default())?;
                        ensure!(
                            locator.metadata.relocated(&actual, false),
                            "Метаданные восстановленного элемента изменились"
                        );
                        locator.metadata = actual;
                        Some(locator)
                    } else {
                        locator
                    },
                    info_parking: current.restore.as_ref().map(|(_, parked)| parked.clone()),
                };
                ensure!(
                    serde_json::to_vec(&plan)?.len() <= 32 * 1024 * 1024,
                    "План отмены слишком большой"
                );
                Ok(plan)
            })();
            match prepared {
                Ok(plan) => Some(plan),
                Err(e) => {
                    current.undo_issue = Some(format!("{e:#}"));
                    None
                }
            }
        } else {
            None
        };
        let after = current
            .target
            .as_deref()
            .map(Identity::read)
            .transpose()?
            .flatten();
        let inverse = if completed {
            let blocked = current.undo_issue.clone();
            Some(Inverse {
                action: match current.spec.kind {
                    "move" | "rename" | "restore" => "move_back",
                    "trash" => "restore_trash",
                    "delete_trash" => "permanent_delete",
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
        let tx = self
            .database
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE items SET state=?3,destination=?4,after=?5,inverse=?6,issue=?7 WHERE operation_id=?1 AND sequence=?2",params![self.id,current.index as i64,if completed{"done"}else if skipped{"skipped"}else if current.published{"partial"}else{"failed"},current.target.as_deref().map(bytes),serde_json::to_vec(&after)?,inverse.as_ref().map(serde_json::to_vec).transpose()?,issue])?;
        if completed && !self.replaying {
            tx.execute("INSERT INTO undo_items(operation_id,sequence,data,state,issue,changed_ms) VALUES (?1,?2,?3,'applied',?4,?5)",params![self.id,current.index as i64,serde_json::to_vec(&plan)?,current.undo_issue,now()])?;
        }
        tx.commit()?;
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
        let history = self.database.history_state(self.id).unwrap_or_default();
        report.journal = Some(Receipt {
            id: self.id,
            items,
            history,
        });
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

use super::clean;
use crate::operations::{Cancellation, Phase, Progress};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

mod conflicts;
#[cfg(target_os = "linux")]
mod cross_device;

#[derive(Debug, Clone)]
pub struct RetainedCopy {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// Remaining source data when restoring its original path was blocked.
    pub recovery: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Operation {
    Create {
        parent: PathBuf,
        name: String,
        directory: bool,
    },
    Rename {
        source: PathBuf,
        name: String,
    },
    Transfer {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        cut: bool,
    },
    Duplicate {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    Trash {
        sources: Vec<PathBuf>,
    },
}
impl Operation {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Create {
                directory: true, ..
            } => "Создание папки",
            Self::Create { .. } => "Создание файла",
            Self::Rename { .. } => "Переименование",
            Self::Transfer { cut: false, .. } => "Копирование",
            Self::Transfer { .. } => "Перемещение",
            Self::Duplicate { .. } => "Дублирование",
            Self::Trash { .. } => "Отправка в корзину",
        }
    }
    pub fn total(&self) -> usize {
        match self {
            Self::Transfer { sources, .. }
            | Self::Duplicate { sources, .. }
            | Self::Trash { sources } => sources.len(),
            _ => 1,
        }
    }
    pub fn destination(&self) -> Option<PathBuf> {
        match self {
            Self::Transfer { destination, .. } | Self::Duplicate { destination, .. } => {
                Some(destination.clone())
            }
            Self::Create { parent, name, .. } => Some(parent.join(name)),
            Self::Rename { source, name } => source.parent().map(|parent| parent.join(name)),
            Self::Trash { .. } => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub journal: Option<crate::journal::Receipt>,
    pub completed: Vec<PathBuf>,
    pub total: usize,
    pub error: Option<String>,
    pub cancelled: bool,
    pub progress: Progress,
    pub retained_copies: Vec<RetainedCopy>,
    pub skipped: Vec<PathBuf>,
    pub destination_recoveries: Vec<crate::conflicts::DestinationRecovery>,
}

#[derive(Debug)]
struct Cancelled;
impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Операция отменена")
    }
}
impl std::error::Error for Cancelled {}
pub(crate) fn operation_checkpoint(cancellation: &Cancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Cancelled.into());
    }
    Ok(())
}
#[derive(Debug)]
struct CleanupFailure(String);
impl fmt::Display for CleanupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

struct Control<'a, 'db> {
    cancellation: &'a Cancellation,
    progress: Progress,
    notify: &'a mut dyn FnMut(&Progress),
    replacement: Option<conflicts::Replacement>,
    recoveries: Vec<crate::conflicts::DestinationRecovery>,
    journal: Option<&'a mut crate::journal::Recorder<'db>>,
}
impl Control<'_, '_> {
    fn begin_item(&mut self, index: usize) -> Result<()> {
        if let Some(journal) = &mut self.journal {
            journal.begin_item(index)?;
        }
        Ok(())
    }
    fn before_step(
        &mut self,
        kind: &str,
        source: Option<&Path>,
        target: Option<&Path>,
    ) -> Result<Option<i64>> {
        match &mut self.journal {
            Some(journal) => journal.before_step(kind, source, target, false),
            None => Ok(None),
        }
    }
    fn after_step(
        &mut self,
        id: Option<i64>,
        kind: &str,
        target: Option<&Path>,
        result: &Result<()>,
    ) {
        if let Some(journal) = &mut self.journal {
            journal.after_step(id, kind, target, result);
        }
    }
    fn rename(&mut self, kind: &str, source: &Path, target: &Path, recovery: bool) -> Result<()> {
        crate::journal::tracked_rename(
            self.journal.as_deref_mut(),
            kind,
            source,
            target,
            recovery,
            (!recovery).then_some(self.cancellation),
        )
    }
    fn conclude_item(&mut self, outcome: Result<()>, completed: bool, skipped: bool) -> Result<()> {
        let recorded = match &mut self.journal {
            Some(journal) => journal.finish_item(
                completed,
                skipped,
                outcome.as_ref().err().map(|e| format!("{e:#}")),
            ),
            None => Ok(()),
        };
        match (outcome, recorded) {
            (Err(error), Err(recording)) => Err(error.context(format!("Журнал: {recording:#}"))),
            (Err(error), _) => Err(error),
            (_, result) => result,
        }
    }
    fn checkpoint(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(Cancelled.into());
        }
        Ok(())
    }
    fn emit(&mut self) {
        (self.notify)(&self.progress);
    }
    fn current(&mut self, path: &Path) -> Result<()> {
        self.checkpoint()?;
        self.progress.current = Some(path.to_path_buf());
        self.emit();
        self.checkpoint()
    }
    fn entry_done(&mut self, path: &Path) {
        self.progress.current = Some(path.to_path_buf());
        self.progress.entries_done = self.progress.entries_done.saturating_add(1);
        if let Some(total) = &mut self.progress.entries_total {
            *total = (*total).max(self.progress.entries_done);
        }
        self.emit();
    }
    fn publish(&mut self, source: &Path, destination: &Path) -> Result<()> {
        self.checkpoint()?;
        match &mut self.replacement {
            Some(replacement) => replacement.publish(
                source,
                destination,
                self.journal.as_deref_mut(),
                self.cancellation,
            ),
            None => self.rename("publish", source, destination, false),
        }
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    let mut parts = Path::new(name).components();
    ensure!(
        !name.is_empty()
            && !name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            && matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none(),
        "Укажите имя без пути, /, \\ и управляющих символов"
    );
    Ok(())
}

/// No-replace publication is also the final cancellation boundary of a staged copy.
pub fn rename_new(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            source,
            rustix::fs::CWD,
            destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .with_context(|| {
            format!(
                "Перемещение {} → {} (без перезаписи)",
                source.display(),
                destination.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        bail!("Безопасное перемещение пока поддерживается только в Linux")
    }
}

fn measure(path: &Path, control: &mut Control<'_, '_>) -> Result<(u64, u64)> {
    control.current(path)?;
    let metadata = fs::symlink_metadata(path)?;
    let mut entries = 1u64;
    let mut bytes = if metadata.is_file() {
        metadata.len()
    } else {
        0
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            control.checkpoint()?;
            let (count, size) = measure(&entry?.path(), control)?;
            entries = entries.saturating_add(count);
            bytes = bytes.saturating_add(size);
        }
    }
    Ok((entries, bytes))
}

fn validate_transfer(source: &Path, destination: &Path) -> Result<PathBuf> {
    let parent = validate_paths(source, destination)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => return Err(conflicts::TargetExists(destination.into()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(parent)
}

fn validate_paths(source: &Path, destination: &Path) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .context("Нет родительской папки")?
        .canonicalize()?;
    let meta = fs::symlink_metadata(source)?;
    if meta.is_dir() {
        ensure!(
            !parent.starts_with(source.canonicalize()?),
            "Нельзя поместить папку в неё саму или её подпапку"
        );
    }
    Ok(parent)
}

fn copy_tree(source: &Path, destination: &Path, control: &mut Control<'_, '_>) -> Result<()> {
    control.current(source)?;
    let meta = fs::symlink_metadata(source)?;
    if meta.is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        #[cfg(not(unix))]
        bail!("Копирование ссылок пока поддерживается только в Unix");
    } else if meta.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            control.checkpoint()?;
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()), control)?;
        }
        control.checkpoint()?;
        fs::set_permissions(destination, meta.permissions())?;
    } else if meta.is_file() {
        copy_file(source, destination, &meta, control, None)?;
    } else {
        bail!("Специальные файлы не копируются: {}", source.display());
    }
    control.entry_done(source);
    Ok(())
}

fn open_regular(path: &Path) -> Result<File> {
    #[cfg(target_os = "linux")]
    let input = File::from(rustix::fs::openat(
        rustix::fs::CWD,
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    #[cfg(not(target_os = "linux"))]
    let input = File::open(path)?;
    ensure!(
        input.metadata()?.is_file(),
        "Тип файла изменился: {}",
        path.display()
    );
    Ok(input)
}

fn copy_file(
    source: &Path,
    destination: &Path,
    meta: &fs::Metadata,
    control: &mut Control<'_, '_>,
    limit: Option<u64>,
) -> Result<()> {
    let mut input = open_regular(source)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = input.metadata()?;
        ensure!(
            opened.dev() == meta.dev() && opened.ino() == meta.ino(),
            "Источник заменён: {}",
            source.display()
        );
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut buffer = vec![0; 256 * 1024];
    let mut remaining = limit;
    loop {
        control.checkpoint()?;
        let capacity = remaining.map_or(buffer.len(), |n| n.min(buffer.len() as u64) as usize);
        if capacity == 0 {
            break;
        }
        let count = match input.read(&mut buffer[..capacity]) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        if let Some(remaining) = &mut remaining {
            *remaining -= count as u64;
        }
        let mut written = 0;
        while written < count {
            control.checkpoint()?;
            let size = match output.write(&buffer[written..count]) {
                Ok(size) => size,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            ensure!(
                size > 0,
                "Запись файла остановилась: {}",
                destination.display()
            );
            written += size;
            control.progress.bytes_done = control.progress.bytes_done.saturating_add(size as u64);
            if let Some(total) = &mut control.progress.bytes_total {
                *total = (*total).max(control.progress.bytes_done);
            }
            control.emit();
        }
    }
    if let Some(remaining) = remaining {
        ensure!(
            remaining == 0 && input.read(&mut [0])? == 0,
            "Размер источника изменился: {}",
            source.display()
        );
    }
    control.checkpoint()?;
    output.flush()?;
    fs::set_permissions(destination, meta.permissions())?;
    Ok(())
}

pub fn transfer_one(source: &Path, destination: &Path, cut: bool) -> Result<()> {
    let cancellation = Cancellation::default();
    let mut notify = |_: &Progress| {};
    let mut control = Control {
        cancellation: &cancellation,
        progress: Progress {
            phase: Phase::Running,
            ..Default::default()
        },
        notify: &mut notify,
        replacement: None,
        recoveries: Vec::new(),
        journal: None,
    };
    transfer(source, destination, cut, &mut control)
}

fn transfer(
    source: &Path,
    destination: &Path,
    cut: bool,
    control: &mut Control<'_, '_>,
) -> Result<()> {
    control.checkpoint()?;
    let parent = if control.replacement.is_some() {
        validate_paths(source, destination)?
    } else {
        validate_transfer(source, destination)?
    };
    if cut {
        control.checkpoint()?;
        match control.publish(source, destination) {
            Ok(()) => {
                control.entry_done(source);
                return Ok(());
            }
            #[cfg(target_os = "linux")]
            Err(error)
                if error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::XDEV)
                    && !conflicts::has_recovery(control) =>
            {
                return cross_device::transfer(source, destination, &parent, control);
            }
            Err(error) => return Err(error),
        }
    }
    let staging = tempfile::Builder::new()
        .prefix(".basalgrim-copy-")
        .tempdir_in(parent)?;
    let staging_id = control.before_step("staging", Some(source), Some(staging.path()))?;
    let staged = staging.path().join("item");
    let result = copy_tree(source, &staged, control).and_then(|()| {
        control.checkpoint()?;
        control.publish(&staged, destination)
    });
    let staging_path = staging.path().to_path_buf();
    if result.is_err() {
        control.progress.phase = Phase::CleaningUp;
        control.emit();
    }
    // Cleanup cannot itself be cancelled; read-only staging never changes the source.
    let cleanup = if result.is_err() {
        writable_directories(&staged)
    } else {
        Ok(())
    }
    .and_then(|()| staging.close().map_err(Into::into));
    control.after_step(staging_id, "staging", Some(&staging_path), &cleanup);
    match (result, cleanup) {
        (Ok(()), _) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(error.context(CleanupFailure(format!(
            "Не удалось очистить временную папку {}: {cleanup:#}",
            staging_path.display()
        )))),
    }
}

fn writable_directories(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(metadata.permissions().mode() | 0o700),
            )?;
        }
        for entry in fs::read_dir(path)? {
            writable_directories(&entry?.path())?;
        }
    }
    Ok(())
}

pub fn operate(operation: Operation) -> Report {
    operate_with_control(operation, &Cancellation::default(), &mut |_| {})
}

pub fn operate_with_control(
    operation: Operation,
    cancellation: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
) -> Report {
    operate_inner(operation, cancellation, notify, None, None)
}

pub fn operate_with_conflicts(
    operation: Operation,
    cancellation: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision,
) -> Report {
    operate_inner(operation, cancellation, notify, Some(resolve), None)
}

pub fn operate_with_journal(
    operation: Operation,
    cancellation: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision,
    journal: &mut crate::journal::Recorder<'_>,
) -> Report {
    let mut report = operate_inner(
        operation,
        cancellation,
        notify,
        Some(resolve),
        Some(journal),
    );
    journal.finish(&mut report);
    report
}

fn operate_inner(
    operation: Operation,
    cancellation: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    mut resolve: Option<&mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision>,
    journal: Option<&mut crate::journal::Recorder<'_>>,
) -> Report {
    let total = operation.total();
    let cut = matches!(operation, Operation::Transfer { cut: true, .. });
    let duplicate = matches!(operation, Operation::Duplicate { .. });
    let mut control = Control {
        cancellation,
        progress: Progress {
            items_total: total,
            ..Default::default()
        },
        notify,
        replacement: None,
        recoveries: Vec::new(),
        journal,
    };
    let mut policy = duplicate.then_some(crate::conflicts::Choice::KeepBoth);
    let mut report = Report {
        total,
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        control.emit();
        control.checkpoint()?;
        if let Operation::Transfer {
            sources,
            destination,
            ..
        }
        | Operation::Duplicate {
            sources,
            destination,
        } = &operation
        {
            let (mut entries, mut bytes, mut known) = (0u64, 0u64, true);
            for source in sources {
                control.checkpoint()?;
                let measured = (|| {
                    let name = source.file_name().context("Нельзя копировать корень")?;
                    if resolve.is_some() || duplicate {
                        validate_paths(source, &destination.join(name))?;
                    } else {
                        validate_transfer(source, &destination.join(name))?;
                    }
                    #[cfg(unix)]
                    if cut {
                        use std::os::unix::fs::MetadataExt;
                        if fs::symlink_metadata(source)?.dev() == fs::metadata(destination)?.dev() {
                            return Ok((1, 0));
                        }
                    }
                    measure(source, &mut control)
                })();
                match measured {
                    Ok((count, size)) => {
                        entries = entries.saturating_add(count);
                        bytes = bytes.saturating_add(size);
                    }
                    Err(error) if error.is::<Cancelled>() => return Err(error),
                    // A later unreadable source must not prevent earlier batch items from completing.
                    Err(_) => known = false,
                }
            }
            if known {
                control.progress.entries_total = Some(entries);
                control.progress.bytes_total = (!cut || bytes > 0).then_some(bytes);
            }
        } else {
            control.progress.entries_total = Some(total as u64);
        }
        control.progress.phase = Phase::Running;
        control.emit();
        let mut complete = |path: PathBuf, control: &mut Control<'_, '_>| {
            report.completed.push(path);
            control.progress.items_done = report.completed.len();
            control.emit();
        };
        match operation {
            Operation::Create {
                parent,
                name,
                directory,
            } => {
                let path = parent.join(&name);
                control.current(&path)?;
                validate_name(&name)?;
                control.begin_item(0)?;
                let step = control.before_step("create", None, Some(&path))?;
                control.checkpoint()?;
                let result = if directory {
                    fs::create_dir(&path).map_err(Into::into)
                } else {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map(|_| ())
                        .map_err(Into::into)
                };
                control.after_step(step, "create", Some(&path), &result);
                result?;
                control.entry_done(&path);
                complete(path, &mut control);
                control.conclude_item(Ok(()), true, false)?;
            }
            Operation::Rename { source, name } => {
                control.current(&source)?;
                validate_name(&name)?;
                let destination = source
                    .parent()
                    .context("Нельзя переименовать корень")?
                    .join(name);
                control.begin_item(0)?;
                let outcome = conflicts::run(
                    conflicts::Request {
                        source: &source,
                        destination: &destination,
                        cut: true,
                        rename: true,
                        remaining: 0,
                    },
                    &mut policy,
                    &mut resolve,
                    &mut control,
                );
                let skipped = matches!(outcome, Ok(false));
                let completed = matches!(outcome, Ok(true));
                if skipped {
                    report.skipped.push(source);
                    control.progress.items_skipped = report.skipped.len();
                } else if outcome.as_ref().is_ok_and(|done| *done) {
                    control.entry_done(&source);
                    complete(source, &mut control);
                }
                let outcome = conflicts::conclude(outcome, &mut control);
                control.conclude_item(outcome, completed, skipped)?;
            }
            Operation::Transfer {
                sources,
                destination,
                ..
            }
            | Operation::Duplicate {
                sources,
                destination,
            } => {
                let total = sources.len();
                for (index, source) in sources.into_iter().enumerate() {
                    control.current(&source)?;
                    let name = source.file_name().context("Нельзя копировать корень")?;
                    control.begin_item(index)?;
                    let outcome = conflicts::run(
                        conflicts::Request {
                            source: &source,
                            destination: &destination.join(name),
                            cut,
                            rename: false,
                            remaining: total - index - 1,
                        },
                        &mut policy,
                        &mut resolve,
                        &mut control,
                    );
                    // The whole item succeeded, including source removal for a move.
                    // Count it before observing any later cancellation.
                    let skipped = matches!(outcome, Ok(false));
                    let completed = matches!(outcome, Ok(true));
                    if skipped {
                        report.skipped.push(source);
                        control.progress.items_skipped = report.skipped.len();
                        control.emit();
                    } else if outcome.as_ref().is_ok_and(|done| *done) {
                        complete(source, &mut control);
                    }
                    let outcome = conflicts::conclude(outcome, &mut control);
                    control.conclude_item(outcome, completed, skipped)?;
                }
            }
            Operation::Trash { sources } => {
                for (index, source) in sources.into_iter().enumerate() {
                    control.current(&source)?;
                    control.begin_item(index)?;
                    let step = control.before_step("trash", Some(&source), None)?;
                    control.checkpoint()?;
                    let result = trash_one(&source);
                    control.after_step(step, "trash", None, &result);
                    result?;
                    control.entry_done(&source);
                    complete(source, &mut control);
                    control.conclude_item(Ok(()), true, false)?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        #[cfg(target_os = "linux")]
        if let Some(partial) = error.downcast_ref::<cross_device::PartialMove>() {
            report.retained_copies.push(partial.0.clone());
        }
        report.cancelled = error.is::<Cancelled>();
        if !report.cancelled || error.downcast_ref::<CleanupFailure>().is_some() {
            report.error = Some(format!("{error:#}"));
        }
    }
    if report.error.is_none() && !report.cancelled {
        // A source can change after the estimate. Completed progress reflects
        // the data actually processed, rather than a stale pre-scan size.
        if control.progress.bytes_total.is_some() {
            control.progress.bytes_total = Some(control.progress.bytes_done);
        }
        if control.progress.entries_total.is_some() {
            control.progress.entries_total = Some(control.progress.entries_done);
        }
    }
    control.progress.phase = Phase::Finished;
    control.emit();
    report.progress = control.progress;
    report.destination_recoveries = control.recoveries;
    report
}

pub fn trash_one(path: &Path) -> Result<()> {
    let output = std::process::Command::new("gio")
        .args(["trash", "--"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .context("Для системной корзины установите gio (Debian/Ubuntu: libglib2.0-bin)")?;
    ensure!(
        output.status.success(),
        "Корзина: {}",
        clean(&String::from_utf8_lossy(&output.stderr))
    );
    Ok(())
}

use super::*;

pub(super) fn run(
    operation: &Operation,
    policy: &mut Option<crate::conflicts::Choice>,
    resolve: &mut Option<&mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision>,
    control: &mut Control<'_, '_>,
) -> Result<bool> {
    let destination = operation.destination().context("Нет папки назначения")?;
    let parent = destination.parent().context("Нет папки назначения")?;
    ensure!(
        parent.is_dir(),
        "Папка назначения недоступна: {}",
        parent.display()
    );
    let source = match operation {
        Operation::ArchiveCreate { sources, .. } => {
            sources.first().context("Нет выбранных файлов")?
        }
        Operation::ArchiveExtract { request, .. } => &request.source,
        _ => unreachable!(),
    };
    control.checkpoint()?;
    control.progress.entries_total = None;
    let staging = tempfile::Builder::new()
        .prefix(".basalgrim-archive-")
        .tempdir_in(parent)?;
    let staging_path = staging.path().to_path_buf();
    let id = control.before_step("staging", Some(source), Some(&staging_path))?;
    let staged = staging
        .path()
        .join(destination.file_name().context("Нет имени результата")?);
    let result = (|| {
        control.checkpoint()?;
        let cancel = control.cancellation.clone();
        let mut notify = |path: &Path, bytes: u64| -> Result<()> {
            control.checkpoint()?;
            control.progress.current = Some(path.into());
            control.progress.bytes_done = control.progress.bytes_done.saturating_add(bytes);
            if bytes == 0 {
                control.progress.entries_done += 1;
            }
            control.emit();
            control.checkpoint()
        };
        match operation {
            Operation::ArchiveCreate {
                sources, format, ..
            } => crate::archive::create(sources, &staged, *format, &cancel, &mut notify)?,
            Operation::ArchiveExtract { request, .. } => {
                crate::archive::extract(request, &staged, &cancel, &mut notify)?
            }
            _ => unreachable!(),
        }
        control.checkpoint()?;
        let has_resolver = resolve.is_some();
        let mut display_resolver = |conflict: &crate::conflicts::Conflict| {
            let mut display = conflict.clone();
            display.source = source.clone();
            resolve.as_mut().map_or(
                crate::conflicts::Decision {
                    choice: crate::conflicts::Choice::KeepBoth,
                    remaining: false,
                },
                |resolver| resolver(&display),
            )
        };
        let mut display_resolve: Option<
            &mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision,
        > = if has_resolver {
            Some(&mut display_resolver)
        } else {
            None
        };
        conflicts::run(
            conflicts::Request {
                source: &staged,
                destination: &destination,
                cut: true,
                rename: true,
                remaining: 0,
            },
            policy,
            &mut display_resolve,
            control,
        )
    })();
    let result = result.map_err(|e| {
        if control.cancellation.is_cancelled() {
            Cancelled.into()
        } else {
            e
        }
    });
    let completed = matches!(result, Ok(true));
    if !completed {
        control.progress.phase = Phase::CleaningUp;
        control.emit();
    }
    let cleanup = writable_directories(&staged).and_then(|()| staging.close().map_err(Into::into));
    control.after_step(id, "staging", Some(&staging_path), &cleanup);
    match (result, cleanup) {
        (Ok(true), _) => Ok(true),
        (result, Ok(())) => result,
        (result, Err(cleanup)) => Err(result
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("Распаковка пропущена"))
            .context(format!(
                "Не удалось очистить временные данные {}: {cleanup:#}",
                staging_path.display()
            ))),
    }
}

use super::*;

pub(super) fn run(
    entry: &crate::trash::Entry,
    destination: Option<&Path>,
    remaining: usize,
    policy: &mut Option<crate::conflicts::Choice>,
    resolve: &mut Option<&mut dyn FnMut(&crate::conflicts::Conflict) -> crate::conflicts::Decision>,
    control: &mut Control<'_, '_>,
) -> Result<bool> {
    entry.validate(control.cancellation)?;
    let target = entry.target(destination)?;
    let holding = tempfile::Builder::new()
        .prefix(".basalgrim-restore-")
        .permissions({
            use std::os::unix::fs::PermissionsExt;
            fs::Permissions::from_mode(0o700)
        })
        .tempdir_in(&entry.root.directory)?
        .keep();
    let parked = holding.join("info");
    let prepare = (|| -> Result<()> {
        if let Some(journal) = &mut control.journal {
            journal.restore_metadata(entry, &parked)?;
        }
        let step = control.before_step("trash_info_claim", Some(&entry.info), Some(&parked))?;
        // Recheck after a journal commit which might have waited on another writer.
        entry.validate(control.cancellation)?;
        let result = rename_new(&entry.info, &parked);
        control.after_step(step, "trash_info_claim", Some(&parked), &result);
        result
    })();
    if let Err(error) = prepare {
        let _ = fs::remove_dir(&holding);
        return Err(error);
    }
    control.restore_guard = Some((entry.clone(), parked.clone()));
    let outcome = (|| {
        if let Some(journal) = &control.journal {
            journal.check()?;
        }
        conflicts::run(
            conflicts::Request {
                source: &entry.data,
                destination: &target,
                cut: true,
                rename: false,
                remaining,
            },
            policy,
            resolve,
            control,
        )
    })();
    control.restore_guard = None;
    if matches!(outcome, Ok(true)) {
        // Retain exact metadata for undo. Never auto-delete a completed restore.
        return outcome;
    }
    let recovery = control.rename("trash_info_restore", &parked, &entry.info, true);
    if let Err(recovery) = recovery {
        let error = outcome
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("Восстановление пропущено"));
        return Err(error.context(CleanupFailure(format!(
            "Метаданные корзины сохранены в {}; возврат не удался: {recovery:#}",
            parked.display()
        ))));
    }
    let _ = fs::remove_dir(holding);
    outcome
}

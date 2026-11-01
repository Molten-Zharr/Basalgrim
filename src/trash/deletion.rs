//! Exact, no-follow objects authorized for permanent deletion from owned trash roots.
use super::*;

#[derive(Clone, Debug)]
pub struct Deletion {
    pub(crate) root: Root,
    pub(crate) objects: Vec<(PathBuf, Identity)>,
    guards: Vec<(PathBuf, Identity)>,
}
impl Deletion {
    pub fn path(&self) -> &Path {
        &self.objects[0].0
    }
    pub(crate) fn validate(&self, cancellation: &Cancellation) -> Result<()> {
        crate::fs::operation_checkpoint(cancellation)?;
        self.validate_roots()?;
        for (path, expected) in &self.objects {
            ensure!(
                Identity::read(path)?.as_ref() == Some(expected),
                "Элемент корзины изменился: {}",
                path.display()
            );
        }
        Ok(())
    }
    pub(crate) fn validate_roots(&self) -> Result<()> {
        let current = self.root.guard()?;
        ensure!(
            current.len() == self.guards.len()
                && current
                    .iter()
                    .zip(&self.guards)
                    .all(|((p, a), (q, b))| p == q
                        && a.device == b.device
                        && a.inode == b.inode
                        && a.mode == b.mode
                        && a.uid == b.uid),
            "Папки корзины изменились перед удалением"
        );
        Ok(())
    }
}

/// Separate from the filtered/bounded display list: empty includes orphan metadata
/// and files beyond the first screen/catalog page. Never scan history holding areas.
pub(super) fn read(roots: &[Root], cancellation: &Cancellation) -> Result<Vec<Deletion>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut budget = 64 * 1024 * 1024usize;
    for root in roots {
        crate::fs::operation_checkpoint(cancellation)?;
        if !seen.insert(root.directory.clone()) {
            continue;
        }
        let Ok(guards) = root.guard() else {
            continue;
        };
        let mut paired = BTreeSet::new();
        for child in fs::read_dir(root.directory.join("files"))? {
            crate::fs::operation_checkpoint(cancellation)?;
            let child = child?;
            let Some(id) = Identity::read(&child.path())? else {
                continue;
            };
            let mut info_name = child.file_name();
            info_name.push(".trashinfo");
            paired.insert(info_name.clone());
            let info = root.directory.join("info").join(info_name);
            let mut objects = vec![(child.path(), id)];
            if let Some(id) = Identity::read(&info)? {
                objects.push((info, id));
            }
            append(
                &mut result,
                &mut budget,
                Deletion {
                    root: root.clone(),
                    objects,
                    guards: guards.clone(),
                },
            )?;
        }
        for child in fs::read_dir(root.directory.join("info"))? {
            crate::fs::operation_checkpoint(cancellation)?;
            let child = child?;
            if paired.contains(&child.file_name()) {
                continue;
            }
            if let Some(id) = Identity::read(&child.path())? {
                append(
                    &mut result,
                    &mut budget,
                    Deletion {
                        root: root.clone(),
                        objects: vec![(child.path(), id)],
                        guards: guards.clone(),
                    },
                )?;
            }
        }
    }
    Ok(result)
}
fn append(result: &mut Vec<Deletion>, budget: &mut usize, target: Deletion) -> Result<()> {
    let bytes = target
        .objects
        .iter()
        .map(|(path, _)| super::bytes(path).len() + 512)
        .sum::<usize>();
    ensure!(
        result.len() < 100_000,
        "Список удаления превышает 100000 элементов; очистка не начата"
    );
    *budget = budget
        .checked_sub(bytes)
        .context("Список удаления превышает 64 MiB; очистка не начата")?;
    result.push(target);
    Ok(())
}

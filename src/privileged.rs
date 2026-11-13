//! Ordinary operations request a short-lived pkexec helper only when access is missing. The GUI,
//! clipboard, session and user SQLite database never run with elevated rights.
use crate::{
    details::PermissionChange,
    fs::Operation,
    journal::{Identity, bytes, path},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

mod trash;
pub use trash::UserTrash;
mod automatic;
#[doc(hidden)]
pub use automatic::execute_with_launcher;
pub use automatic::{execute, prepare};
mod transport;
pub use transport::{helper_entry, run};
#[doc(hidden)]
pub use transport::{run_with_command, serve};

pub const LIMIT: usize = 64;
pub const UNDO: &str = "Операция администратора: автоматическая отмена и повтор недоступны";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Copy,
    Duplicate,
    Move,
    Rename,
    File,
    Folder,
    Permissions,
    Trash,
}
impl Kind {
    pub fn available(self, count: usize) -> bool {
        match self {
            Self::File | Self::Folder => true,
            Self::Copy | Self::Move | Self::Duplicate | Self::Trash => (1..=LIMIT).contains(&count),
            Self::Rename | Self::Permissions => count == 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub kind: Kind,
    pub sources: Vec<Vec<u8>>,
    pub directory: Vec<u8>,
    pub text: String,
    #[serde(default)]
    pub destination_directory: Option<Vec<u8>>,
    #[serde(default)]
    pub trash: Option<UserTrash>,
}
impl Request {
    pub fn total(&self) -> usize {
        match self.kind {
            Kind::Copy | Kind::Move | Kind::Duplicate | Kind::Trash => self.sources.len(),
            _ => 1,
        }
    }
    pub fn destination(&self) -> PathBuf {
        match self.kind {
            Kind::Copy | Kind::Move | Kind::Duplicate => self
                .destination_directory
                .as_deref()
                .map(path)
                .unwrap_or_else(|| self.text.clone().into()),
            Kind::File | Kind::Folder => path(&self.directory).join(&self.text),
            Kind::Rename => self
                .sources
                .first()
                .map(|p| path(p).with_file_name(&self.text))
                .unwrap_or_default(),
            Kind::Permissions => self.sources.first().map(|p| path(p)).unwrap_or_default(),
            Kind::Trash => self
                .trash
                .as_ref()
                .map(|t| path(&t.data).join("Trash"))
                .unwrap_or_default(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.sources.len() <= LIMIT && self.kind.available(self.sources.len()),
            "Выберите подходящие файлы (не более 64)"
        );
        ensure!(
            self.sources.iter().map(Vec::len).sum::<usize>()
                + self.directory.len()
                + self.text.len()
                + self.destination_directory.as_ref().map_or(0, Vec::len)
                <= 128 * 1024,
            "Слишком длинный список путей"
        );
        for raw in self.sources.iter().chain(std::iter::once(&self.directory)) {
            valid_path(&path(raw))?;
        }
        match self.kind {
            Kind::Copy | Kind::Move | Kind::Duplicate => {
                if let Some(raw) = &self.destination_directory {
                    ensure!(
                        path(raw).to_string_lossy() == self.text,
                        "Адрес назначения не соответствует подтверждённому пути"
                    );
                }
                valid_path(&self.destination())?;
            }
            Kind::Rename | Kind::File | Kind::Folder => crate::fs::validate_name(&self.text)?,
            Kind::Permissions => {
                self.mode()?;
            }
            Kind::Trash => {
                let context = self
                    .trash
                    .as_ref()
                    .context("Нет пользовательской корзины")?;
                valid_path(&path(&context.home))?;
                valid_path(&path(&context.data))?;
                ensure!(
                    context.home.len() + context.data.len() <= 128 * 1024,
                    "Слишком длинный путь корзины"
                );
            }
        }
        ensure!(
            self.kind == Kind::Trash || self.trash.is_none(),
            "Контекст корзины не соответствует операции"
        );
        Ok(())
    }
    fn mode(&self) -> Result<u32> {
        ensure!(
            self.text.len() == 3 && self.text.bytes().all(|b| (b'0'..=b'7').contains(&b)),
            "Введите три восьмеричные цифры, например 644 или 755"
        );
        Ok(u32::from_str_radix(&self.text, 8)?)
    }
    pub fn specs(&self) -> Vec<crate::journal::Spec> {
        let target = self.destination();
        let sources: Vec<_> = match self.kind {
            Kind::File | Kind::Folder => vec![target.clone()],
            _ => self.sources.iter().map(|p| path(p)).collect(),
        };
        sources
            .into_iter()
            .map(|source| crate::journal::Spec {
                kind: "privileged",
                target: (self.kind != Kind::Trash).then(|| {
                    if matches!(self.kind, Kind::Copy | Kind::Move | Kind::Duplicate) {
                        target.join(source.file_name().unwrap_or_default())
                    } else {
                        target.clone()
                    }
                }),
                source,
            })
            .collect()
    }
}

// An inaccessible component is not an absent component. Keep the accessible
// ancestor evidence; the helper fills only permission-denied gaps after auth.
fn capture_identity(p: &Path) -> Result<Option<Identity>> {
    match Identity::read(p) {
        Ok(Some(id)) => Ok(Some(id)),
        Ok(None) => anyhow::bail!("Объект исчез: {}", p.display()),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn valid_path(p: &Path) -> Result<()> {
    ensure!(
        p.is_absolute() && !bytes(p).contains(&0),
        "Нужен абсолютный путь к объекту"
    );
    ensure!(
        p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        "Путь не должен содержать . или .."
    );
    Ok(())
}

fn native_filesystem(p: &Path) -> Result<()> {
    // Linux uapi linux/magic.h: ext2/3/4, Btrfs, XFS, F2FS, tmpfs,
    // ramfs, overlayfs and eCryptfs. No GVfs/FUSE, network or pseudo FS.
    let kind = rustix::fs::statfs(p)?.f_type as u64;
    ensure!(
        matches!(
            kind,
            0xef53
                | 0x9123683e
                | 0x58465342
                | 0xf2f52010
                | 0x01021994
                | 0x858458f6
                | 0x794c7630
                | 0xf15f
        ),
        "Повышение прав для этой файловой системы не поддерживается: {}",
        p.display()
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    request: Request,
    // Every path component is captured before authentication and checked again
    // afterwards. Directory contents/mtime may change; ownership and inode may not.
    parents: Vec<(Vec<u8>, Option<Identity>)>,
    sources: Vec<Option<Identity>>,
}
impl std::ops::Deref for Plan {
    type Target = Request;
    fn deref(&self) -> &Request {
        &self.request
    }
}
impl Plan {
    pub fn capture(request: Request) -> Result<Self> {
        request.validate()?;
        let mut parents = std::collections::BTreeSet::new();
        parents.insert(path(&request.directory));
        if let Some(trash) = &request.trash {
            parents.extend(trash.parents()?);
        }
        let mut sources = Vec::new();
        for raw in &request.sources {
            let p = path(raw);
            ensure!(
                p != Path::new("/"),
                "Нельзя изменять корень файловой системы"
            );
            parents.insert(p.parent().context("Нет родительской папки")?.into());
            let id = capture_identity(&p)?;
            if let Some(id) = &id {
                if id.link.is_none() {
                    native_filesystem(&p)?;
                }
                ensure!(
                    matches!(id.mode & 0o170000, 0o100000 | 0o040000 | 0o120000),
                    "Специальные файлы не поддерживаются"
                );
            }
            sources.push(id);
        }
        if matches!(request.kind, Kind::Copy | Kind::Move | Kind::Duplicate) {
            parents.insert(request.destination());
        }
        let mut components = std::collections::BTreeSet::new();
        let mut budget = 256 * 1024usize;
        for p in parents {
            for ancestor in p.ancestors() {
                if !components.contains(ancestor) {
                    budget = budget
                        .checked_sub(bytes(ancestor).len())
                        .context("Слишком длинные родительские пути")?;
                    ensure!(components.len() < 4096, "Слишком много компонентов пути");
                    components.insert(ancestor.to_path_buf());
                }
            }
        }
        let parents = components
            .into_iter()
            .map(|p| {
                let id = capture_identity(&p)?;
                if let Some(id) = &id {
                    native_filesystem(&p)?;
                    ensure!(
                        id.directory() && id.link.is_none(),
                        "Ссылки в родительском пути не поддерживаются: {}",
                        p.display()
                    );
                }
                Ok((bytes(&p), id))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            request,
            parents,
            sources,
        })
    }
    fn validate(&self, uid: u32) -> Result<()> {
        self.authorize(uid).map(|_| ())
    }
    fn authorize(&self, uid: u32) -> Result<Self> {
        self.request.validate()?;
        ensure!(
            self.parents.len() <= 4096 && self.sources.len() == self.request.sources.len(),
            "Некорректный снимок операции"
        );
        let fresh = Self::capture(self.request.clone())?;
        if let Some(trash) = &self.trash {
            trash.validate(uid)?;
        }
        ensure!(
            fresh.sources.len() == self.sources.len()
                && self
                    .sources
                    .iter()
                    .zip(&fresh.sources)
                    .all(|(old, new)| new.is_some()
                        && old.as_ref().is_none_or(|v| Some(v) == new.as_ref()))
                && fresh.parents.len() == self.parents.len(),
            "Исходные объекты изменились после запроса операции"
        );
        for ((name, before), (new_name, now)) in self.parents.iter().zip(&fresh.parents) {
            let now = now
                .as_ref()
                .context("Нет доступа к родительской папке после авторизации")?;
            ensure!(
                name == new_name
                    && before
                        .as_ref()
                        .is_none_or(|before| before.device == now.device
                            && before.inode == now.inode
                            && before.mode == now.mode
                            && before.uid == now.uid
                            && before.gid == now.gid),
                "Родительская папка изменилась после запроса операции"
            );
            // A third-party writer must not redirect a privileged pathname. Sticky
            // shared ancestors (/tmp) are okay only for root/caller-owned descendants.
            ensure!(
                (now.uid == 0 || now.uid == uid)
                    && (now.mode & 0o022 == 0 || now.mode & 0o1000 != 0),
                "Повышение прав в папке с групповой/общей записью или чужим владельцем не поддерживается: {}",
                path(name).display()
            );
        }
        Ok(fresh)
    }
    fn operation(&self) -> Result<Operation> {
        let r = &self.request;
        let sources: Vec<_> = r.sources.iter().map(|p| path(p)).collect();
        Ok(match r.kind {
            Kind::Duplicate => Operation::Duplicate {
                sources,
                destination: r.destination(),
            },
            Kind::Copy | Kind::Move => Operation::Transfer {
                sources,
                destination: r.destination(),
                cut: r.kind == Kind::Move,
            },
            Kind::Rename => Operation::Rename {
                source: sources[0].clone(),
                name: r.text.clone(),
            },
            Kind::File | Kind::Folder => Operation::Create {
                parent: path(&r.directory),
                name: r.text.clone(),
                directory: r.kind == Kind::Folder,
            },
            Kind::Trash => Operation::Trash { sources },
            Kind::Permissions => {
                use std::os::unix::fs::MetadataExt;
                let m = fs::symlink_metadata(sources[0].parent().unwrap())?;
                Operation::Permissions(PermissionChange {
                    path: sources[0].clone(),
                    before: self.sources[0]
                        .clone()
                        .context("Нет снимка объекта для изменения прав")?,
                    parent: (m.dev(), m.ino()),
                    mode: r.mode()?,
                })
            }
        })
    }

    fn check_trees(&self, uid: u32, cancel: &crate::operations::Cancellation) -> Result<()> {
        if !matches!(
            self.request.kind,
            Kind::Copy | Kind::Move | Kind::Duplicate | Kind::Rename | Kind::Trash
        ) {
            return Ok(());
        }
        for raw in &self.request.sources {
            let p = path(raw);
            if fs::symlink_metadata(&p)?.is_dir() {
                let file = fs::File::from(rustix::fs::open(
                    &p,
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::NOFOLLOW
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )?);
                trusted_tree(&file, uid, 0, cancel)?;
            }
        }
        Ok(())
    }
}

// A privileged recursive walk may not traverse directories another user can
// replace between the common engine's checks. Walk pinned descriptors here;
// links remain links and are never followed, even during this preflight.
fn trusted_tree(
    dir: &fs::File,
    uid: u32,
    depth: usize,
    cancel: &crate::operations::Cancellation,
) -> Result<()> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    crate::fs::operation_checkpoint(cancel)?;
    ensure!(
        depth <= 128,
        "Операция администратора: более 128 уровней папок"
    );
    let m = dir.metadata()?;
    let mount = rustix::fs::statx(
        dir,
        "",
        rustix::fs::AtFlags::EMPTY_PATH,
        rustix::fs::StatxFlags::MNT_ID,
    )?;
    ensure!(
        mount.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0,
        "Не удалось определить точку монтирования"
    );
    ensure!(
        (m.uid() == 0 || m.uid() == uid) && m.mode() & 0o022 == 0,
        "Внутри источника есть папка с групповой/общей записью или чужим владельцем"
    );
    for entry in fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
        crate::fs::operation_checkpoint(cancel)?;
        let entry = entry?;
        let entry_mount = rustix::fs::statx(
            dir,
            entry.file_name(),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            rustix::fs::StatxFlags::MNT_ID,
        )?;
        ensure!(
            entry_mount.stx_mnt_id == mount.stx_mnt_id,
            "Вложенные точки монтирования не поддерживаются"
        );
        if entry.file_type()?.is_dir() {
            let child = fs::File::from(openat(
                dir,
                entry.file_name(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            ensure!(
                child.metadata()?.dev() == m.dev()
                    && rustix::fs::statx(
                        &child,
                        "",
                        rustix::fs::AtFlags::EMPTY_PATH,
                        rustix::fs::StatxFlags::MNT_ID
                    )?
                    .stx_mnt_id
                        == mount.stx_mnt_id,
                "Вложенные точки монтирования не поддерживаются"
            );
            trusted_tree(&child, uid, depth + 1, cancel)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn rejects_source_and_parent_replacement_after_authentication() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let p = root.path().join("file");
        fs::write(&p, b"before").unwrap();
        let r = Request {
            kind: Kind::Rename,
            sources: vec![bytes(&p)],
            directory: bytes(root.path()),
            text: "new".into(),
            destination_directory: None,
            trash: None,
        };
        let plan = Plan::capture(r.clone()).unwrap();
        fs::write(&p, b"changed").unwrap();
        assert!(plan.validate(rustix::process::geteuid().as_raw()).is_err());
        let plan = Plan::capture(r).unwrap();
        fs::rename(&p, root.path().join("parked")).unwrap();
        symlink(root.path().join("parked"), &p).unwrap();
        assert!(plan.validate(rustix::process::geteuid().as_raw()).is_err());
    }
}

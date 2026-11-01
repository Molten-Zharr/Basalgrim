//! No-follow, bounded tree evidence. File contents are streamed, never retained.
use super::*;
use crate::operations::Cancellation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, os::fd::AsRawFd};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub name: Vec<u8>,
    pub identity: Identity,
    pub hash: Option<[u8; 32]>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
}
impl Snapshot {
    pub fn read(path: &Path, cancellation: &Cancellation) -> Result<Self> {
        let mut snapshot = Self { nodes: Vec::new() };
        let mount = mount(path)?;
        let mut budget = 4 * 1024 * 1024;
        snapshot.walk(path, Vec::new(), mount, 0, &mut budget, cancellation)?;
        ensure!(
            serde_json::to_vec(&snapshot)?.len() <= 32 * 1024 * 1024,
            "Снимок папки слишком большой для отмены"
        );
        Ok(snapshot)
    }
    fn walk(
        &mut self,
        path: &Path,
        name: Vec<u8>,
        root_mount: u64,
        depth: usize,
        budget: &mut usize,
        cancellation: &Cancellation,
    ) -> Result<()> {
        crate::fs::operation_checkpoint(cancellation)?;
        ensure!(
            depth <= 128 && self.nodes.len() < 100_000,
            "Превышен предел снимка: 100000 элементов / 128 уровней"
        );
        *budget = budget
            .checked_sub(name.len())
            .context("Слишком длинные пути снимка")?;
        ensure!(
            mount(path)? == root_mount,
            "Отмена не поддерживает вложенные точки монтирования"
        );
        let before = Identity::read(path)?.context("Файл исчез во время снимка")?;
        let mut hash = None;
        let index = self.nodes.len();
        self.nodes.push(Node {
            name: name.clone(),
            identity: before.clone(),
            hash: None,
        });
        if before.link.is_none() {
            let mut flags = rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC;
            if before.directory() {
                flags |= rustix::fs::OFlags::DIRECTORY;
            }
            ensure!(
                before.directory() || before.mode & 0o170000 == 0o100000,
                "Специальный файл: безопасная отмена недоступна"
            );
            let mut file = File::from(rustix::fs::open(path, flags, rustix::fs::Mode::empty())?);
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            ensure!(
                metadata.dev() == before.device && metadata.ino() == before.inode,
                "Файл подменён во время снимка"
            );
            if before.directory() {
                // Pin the parent directory even if another process renames it.
                let pinned = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
                let mut children = Vec::new();
                for entry in fs::read_dir(&pinned)? {
                    crate::fs::operation_checkpoint(cancellation)?;
                    ensure!(
                        children.len() + self.nodes.len() < 100_000,
                        "Слишком много элементов для отмены"
                    );
                    children.push(entry?.file_name());
                }
                children.sort();
                for child in children {
                    let mut relative = name.clone();
                    if !relative.is_empty() {
                        relative.push(b'/');
                    }
                    use std::os::unix::ffi::OsStrExt;
                    relative.extend_from_slice(child.as_bytes());
                    self.walk(
                        &pinned.join(child),
                        relative,
                        root_mount,
                        depth + 1,
                        budget,
                        cancellation,
                    )?;
                }
            } else {
                let mut digest = Sha256::new();
                let mut buffer = vec![0; 256 * 1024];
                let mut reader = (&mut file).take(before.size.saturating_add(1));
                loop {
                    crate::fs::operation_checkpoint(cancellation)?;
                    let read = reader.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    digest.update(&buffer[..read]);
                }
                hash = Some(digest.finalize().into());
            }
        }
        ensure!(
            Identity::read(path)?.as_ref() == Some(&before),
            "Файл изменился во время снимка: {}",
            path.display()
        );
        self.nodes[index].hash = hash;
        Ok(())
    }
    /// Own later operations can change ctime and directory metadata even when
    /// their effects have been undone. Full manifests, contents and inodes still match.
    pub fn unchanged(&self, other: &Self) -> bool {
        self.nodes.len() == other.nodes.len()
            && self.nodes.iter().zip(&other.nodes).all(|(a, b)| {
                if a.name != b.name || a.hash != b.hash {
                    return false;
                }
                let mut identity = a.identity.clone();
                identity.changed = b.identity.changed;
                if identity.directory() {
                    identity.modified = b.identity.modified;
                    identity.size = b.identity.size;
                }
                identity == b.identity
            })
    }
    /// Publication may change the root ctime. Cross-device moves recreate inodes.
    pub fn relocated(&self, other: &Self, copied: bool) -> bool {
        self.nodes.len() == other.nodes.len()
            && self
                .nodes
                .iter()
                .zip(&other.nodes)
                .enumerate()
                .all(|(index, (a, b))| {
                    if a.name != b.name || a.hash != b.hash {
                        return false;
                    }
                    let mut identity = a.identity.clone();
                    if copied {
                        return identity.mode == b.identity.mode
                            && identity.link == b.identity.link
                            && (identity.directory() || identity.size == b.identity.size);
                    }
                    if index == 0 {
                        identity.changed = b.identity.changed;
                    }
                    identity == b.identity
                })
    }
}
pub fn mount(path: &Path) -> Result<u64> {
    let stat = rustix::fs::statx(
        rustix::fs::CWD,
        path,
        rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        rustix::fs::StatxFlags::MNT_ID,
    )?;
    ensure!(
        stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0,
        "Не удалось определить точку монтирования"
    );
    Ok(stat.stx_mnt_id)
}

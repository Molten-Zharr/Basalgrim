//! Explicit file inspection and installed-application discovery, outside the UI thread.
use crate::{archive, journal::Identity, operations::Cancellation};
use anyhow::{Context, Result, ensure};
use gio::prelude::*;
use std::{
    ffi::OsString,
    fs::{self, File},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
};

#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub archive: Option<Arc<archive::Catalog>>,
    pub readonly: bool,
}
#[derive(Clone, Debug)]
pub struct Properties {
    pub source: Source,
    pub identity: Option<Identity>,
    pub parent: Option<(u64, u64)>,
    pub kind: String,
    pub mime: String,
    pub size: u64,
    pub allocated: Option<u64>,
    pub modified: Option<std::time::SystemTime>,
    pub link: Option<PathBuf>,
    pub mode: u32,
}
impl Properties {
    pub fn editable(&self) -> bool {
        !self.source.readonly
            && self.identity.as_ref().is_some_and(|i| {
                i.link.is_none() && matches!(i.mode & 0o170000, 0o100000 | 0o040000)
            })
    }
    pub fn change(&self, mode: u32) -> Result<PermissionChange> {
        ensure!(
            self.editable(),
            "Права этого объекта доступны только для чтения"
        );
        ensure!(mode <= 0o777, "Некорректные права доступа");
        Ok(PermissionChange {
            path: self.source.path.clone(),
            before: self.identity.clone().unwrap(),
            parent: self.parent.unwrap(),
            mode,
        })
    }
}
#[derive(Clone, Debug)]
pub struct PermissionChange {
    pub path: PathBuf,
    pub before: Identity,
    pub parent: (u64, u64),
    pub mode: u32,
}
impl PermissionChange {
    pub(crate) fn pin(&self) -> Result<Pinned> {
        ensure!(
            self.mode <= 0o777
                && self.before.link.is_none()
                && matches!(self.before.mode & 0o170000, 0o100000 | 0o040000),
            "Права ссылок и специальных файлов не изменяются"
        );
        let pinned = Pinned::open(&self.path)?;
        ensure!(
            pinned.identity()? == self.before && pinned.parent_id()? == self.parent,
            "Файл или его права изменились. Обновите свойства перед применением"
        );
        Ok(pinned)
    }
    pub fn requested_mode(&self) -> u32 {
        (self.before.mode & 0o7000) | self.mode
    }
}
pub(crate) struct Pinned {
    file: File,
    parent: File,
    name: OsString,
}
impl Pinned {
    fn open(path: &Path) -> Result<Self> {
        use rustix::fs::{Mode, OFlags, openat};
        ensure!(path.is_absolute(), "Нужен абсолютный путь");
        let mut parent = File::from(rustix::fs::open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let mut parts = Vec::new();
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(p) => parts.push(p),
                _ => anyhow::bail!("Путь содержит неоднозначный компонент"),
            }
        }
        let name = parts.pop().unwrap_or_else(|| std::ffi::OsStr::new("."));
        for part in parts {
            parent = File::from(openat(
                &parent,
                part,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
        }
        let file = File::from(openat(
            &parent,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        Ok(Self {
            file,
            parent,
            name: name.into(),
        })
    }
    fn proc_path(&self) -> PathBuf {
        format!("/proc/self/fd/{}", self.file.as_raw_fd()).into()
    }
    fn parent_id(&self) -> Result<(u64, u64)> {
        let m = self.parent.metadata()?;
        Ok((m.dev(), m.ino()))
    }
    fn identity(&self) -> Result<Identity> {
        let m = self.file.metadata()?;
        let link = if m.is_symlink() {
            Some(
                rustix::fs::readlinkat(&self.parent, &self.name, Vec::new())?
                    .to_bytes()
                    .to_vec(),
            )
        } else {
            None
        };
        Ok(Identity {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
            size: m.len(),
            modified: [m.mtime(), m.mtime_nsec()],
            changed: [m.ctime(), m.ctime_nsec()],
            uid: m.uid(),
            gid: m.gid(),
            link,
        })
    }
    pub(crate) fn apply(&self, change: &PermissionChange) -> Result<()> {
        // Resolve again after the durable intent, then chmod the pinned inode. Neither
        // a replaced leaf nor a replaced/symlinked parent can retarget this operation.
        let current = change.pin()?;
        ensure!(
            self.identity()? == current.identity()?,
            "Объект изменился перед применением прав"
        );
        fs::set_permissions(
            self.proc_path(),
            fs::Permissions::from_mode(change.requested_mode()),
        )
        .context("Не удалось изменить права доступа")
    }
}
fn mime(path: &Path, data: Option<&[u8]>) -> String {
    let (content, _) = gio::content_type_guess(Some(path), data);
    gio::content_type_get_mime_type(&content)
        .map(|s| s.into())
        .unwrap_or_else(|| "application/octet-stream".into())
}
pub fn inspect(source: &Source, cancel: &Cancellation) -> Result<Properties> {
    archive::check(cancel)?;
    if let Some(catalog) = &source.archive {
        ensure!(catalog.valid(), "Архив изменился; откройте его снова");
        let relative = source.path.strip_prefix(&catalog.source)?;
        let root = archive::Member {
            path: PathBuf::new(),
            kind: archive::Kind::Directory,
            size: 0,
            mode: 0o555,
            modified: None,
        };
        let member = if relative.as_os_str().is_empty() {
            &root
        } else {
            catalog
                .members
                .get(relative)
                .context("Элемент архива не найден")?
        };
        return Ok(Properties {
            source: source.clone(),
            identity: None,
            parent: None,
            kind: match member.kind {
                archive::Kind::Directory => "Папка в архиве",
                archive::Kind::File => "Файл в архиве",
                _ => "Ссылка в архиве",
            }
            .into(),
            mime: if member.kind == archive::Kind::Directory {
                "inode/directory".into()
            } else {
                mime(&source.path, None)
            },
            size: member.size,
            allocated: None,
            modified: member.modified,
            link: match &member.kind {
                archive::Kind::Symlink(p) | archive::Kind::Hardlink(p) => Some(p.clone()),
                _ => None,
            },
            mode: member.mode,
        });
    }
    let pinned = Pinned::open(&source.path)?;
    let identity = pinned.identity()?;
    let meta = pinned.file.metadata()?;
    let mut data = Vec::new();
    if meta.is_file()
        && let Ok(file) = File::open(pinned.proc_path())
    {
        file.take(8192).read_to_end(&mut data)?;
    }
    archive::check(cancel)?;
    ensure!(
        pinned.identity()? == identity,
        "Объект изменился во время чтения свойств"
    );
    Ok(Properties {
        source: source.clone(),
        parent: Some(pinned.parent_id()?),
        kind: if meta.is_symlink() {
            "Ссылка"
        } else if meta.is_dir() {
            "Папка"
        } else if meta.is_file() {
            "Файл"
        } else {
            "Специальный файл"
        }
        .into(),
        mime: if meta.is_dir() {
            "inode/directory".into()
        } else if meta.is_symlink() {
            "inode/symlink".into()
        } else if meta.is_file() {
            mime(&source.path, (!data.is_empty()).then_some(&data))
        } else {
            "application/octet-stream".into()
        },
        size: meta.len(),
        allocated: Some(meta.blocks().saturating_mul(512)),
        modified: meta.modified().ok(),
        link: identity.link.as_deref().map(crate::journal::path),
        mode: meta.mode(),
        identity: Some(identity),
    })
}
#[derive(Clone, Debug)]
pub struct Application {
    pub name: String,
    pub description: String,
    pub default: bool,
}
struct Registered {
    info: gio::AppInfo,
    path: PathBuf,
    digest: [u8; 32],
}
fn desktop_digest(path: &Path) -> Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let f = File::open(path)?;
    ensure!(
        f.metadata()?.is_file() && f.metadata()?.len() <= 256 * 1024,
        "Описание приложения слишком большое"
    );
    let mut bytes = Vec::new();
    f.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 256 * 1024,
        "Описание приложения слишком большое"
    );
    Ok(Sha256::digest(&bytes).into())
}
fn applications(
    props: &Properties,
    all: bool,
    cancel: &Cancellation,
) -> Result<(Vec<Application>, Vec<Registered>)> {
    let infos = if all {
        gio::AppInfo::all()
    } else {
        gio::AppInfo::all_for_type(&props.mime)
    };
    ensure!(
        infos.len() <= 4096,
        "Слишком много установленных приложений"
    );
    let default = gio::AppInfo::default_for_type(&props.mime, false);
    let mut pairs = Vec::new();
    for info in infos {
        archive::check(cancel)?;
        if !info.should_show() || (!info.supports_files() && !info.supports_uris()) {
            continue;
        }
        let Some(desktop) = info.downcast_ref::<gio::DesktopAppInfo>() else {
            continue;
        };
        let Some(path) = desktop.filename() else {
            continue;
        };
        let Ok(digest) = desktop_digest(&path) else {
            continue;
        };
        let row = Application {
            name: crate::fs::clean(&info.display_name().chars().take(256).collect::<String>()),
            description: crate::fs::clean(
                &info
                    .description()
                    .unwrap_or_default()
                    .chars()
                    .take(512)
                    .collect::<String>(),
            ),
            default: default.as_ref().is_some_and(|d| d.equal(&info)),
        };
        pairs.push((row, Registered { info, path, digest }));
    }
    pairs.sort_by_key(|(row, _)| (!row.default, row.name.to_lowercase()));
    Ok(pairs.into_iter().unzip())
}
pub enum Event {
    Properties(u64, Result<Properties, String>),
    Applications(u64, Result<(Properties, Vec<Application>), String>),
    Launched(u64, Option<PathBuf>, Result<(), String>),
}
enum Job {
    Properties(u64, Source, Cancellation),
    Applications(u64, Source, bool, Cancellation),
    Launch(u64, usize, Cancellation),
}
struct Cache {
    token: u64,
    properties: Properties,
    apps: Vec<Registered>,
}
struct Runtime {
    tx: mpsc::SyncSender<Job>,
    rx: mpsc::Receiver<Event>,
    stop: Cancellation,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
#[derive(Default)]
pub struct Service {
    runtime: Option<Runtime>,
}
impl Service {
    fn runtime(&mut self) -> &Runtime {
        self.runtime.get_or_insert_with(|| {
            let (tx, rx) = mpsc::sync_channel::<Job>(2);
            let (out, events) = mpsc::channel();
            let stop = Cancellation::default();
            let stopped = stop.clone();
            thread::spawn(move || run(rx, out, stopped));
            Runtime {
                tx,
                rx: events,
                stop,
            }
        })
    }

    pub fn properties(&mut self, id: u64, source: Source, cancel: Cancellation) -> Result<()> {
        self.runtime()
            .tx
            .try_send(Job::Properties(id, source, cancel))
            .map_err(|_| anyhow::anyhow!("Чтение ещё занято; повторите"))
    }
    pub fn applications(
        &mut self,
        id: u64,
        source: Source,
        all: bool,
        cancel: Cancellation,
    ) -> Result<()> {
        self.runtime()
            .tx
            .try_send(Job::Applications(id, source, all, cancel))
            .map_err(|_| anyhow::anyhow!("Чтение ещё занято; повторите"))
    }
    pub fn launch(&mut self, id: u64, index: usize, cancel: Cancellation) -> Result<()> {
        self.runtime()
            .tx
            .try_send(Job::Launch(id, index, cancel))
            .map_err(|_| anyhow::anyhow!("Запуск ещё занят; повторите"))
    }
    pub fn poll(&self) -> Vec<Event> {
        self.runtime
            .as_ref()
            .map(|r| r.rx.try_iter().collect())
            .unwrap_or_default()
    }
}

fn run(rx: mpsc::Receiver<Job>, out: mpsc::Sender<Event>, stopped: Cancellation) {
    let mut cache: Option<Cache> = None;
    // External programs may read their file after launch returns. Keep materialized
    // members for this application lifetime, with explicit count and byte bounds.
    let mut retained = Vec::new();
    let mut bytes = 0u64;
    while let Ok(job) = rx.recv() {
        if stopped.is_cancelled() {
            break;
        }
        let event = match job {
            Job::Properties(id, source, cancel) => {
                cache = None;
                if cancel.is_cancelled() {
                    continue;
                }
                let result = inspect(&source, &cancel).map_err(|e| format!("{e:#}"));
                if cancel.is_cancelled() {
                    continue;
                }
                Event::Properties(id, result)
            }
            Job::Applications(id, source, all, cancel) => {
                cache = None;
                if cancel.is_cancelled() {
                    continue;
                }
                let result = (|| -> Result<_> {
                    let props = inspect(&source, &cancel)?;
                    ensure!(
                        props.link.is_none()
                            && (props
                                .identity
                                .as_ref()
                                .is_some_and(|i| i.mode & 0o170000 == 0o100000)
                                || (source.archive.is_some() && props.kind == "Файл в архиве")),
                        "Выберите обычный файл; ссылки и специальные файлы не запускаются"
                    );
                    let (rows, apps) = applications(&props, all, &cancel)?;
                    cache = Some(Cache {
                        token: id,
                        properties: props.clone(),
                        apps,
                    });
                    Ok((props, rows))
                })()
                .map_err(|e| format!("{e:#}"));
                if cancel.is_cancelled() {
                    cache = None;
                    continue;
                }
                Event::Applications(id, result)
            }
            Job::Launch(id, index, cancel) => {
                let mut recent = None;
                let result = (|| -> Result<()> {
                    archive::check(&cancel)?;
                    let cached = cache
                        .take()
                        .context("Список приложений устарел; откройте его снова")?;
                    ensure!(cached.token == id, "Список приложений изменился");
                    let app = cached.apps.get(index).context("Приложение не найдено")?;
                    ensure!(
                        desktop_digest(&app.path)? == app.digest,
                        "Приложение изменилось; обновите список"
                    );
                    let props = &cached.properties;
                    let source = &props.source;
                    let path = if let Some(archive) = &source.archive {
                        ensure!(
                            retained.len() < 32,
                            "Открыто 32 временных файла; используйте распаковку"
                        );
                        let dir = tempfile::tempdir()?;
                        let path = archive.materialize(
                            &source.path,
                            dir.path(),
                            (512 * 1024 * 1024u64).saturating_sub(bytes),
                            &cancel,
                        )?;
                        bytes += fs::metadata(&path)?.len();
                        retained.push(dir);
                        path
                    } else {
                        let pinned = Pinned::open(&source.path)?;
                        ensure!(
                            Some(pinned.identity()?) == props.identity
                                && Some(pinned.parent_id()?) == props.parent,
                            "Файл изменился; откройте список приложений снова"
                        );
                        recent = Some(source.path.clone());
                        source.path.clone()
                    };
                    archive::check(&cancel)?;
                    archive::check(&stopped)?;
                    app.info
                        .launch(
                            &[gio::File::for_path(&path)],
                            None::<&gio::AppLaunchContext>,
                        )
                        .context("Не удалось запустить выбранную программу")?;
                    Ok(())
                })()
                .map_err(|e| format!("{e:#}"));
                Event::Launched(id, recent, result)
            }
        };
        if out.send(event).is_err() {
            break;
        }
    }
}

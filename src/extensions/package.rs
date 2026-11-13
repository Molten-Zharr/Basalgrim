use super::*;
use anyhow::Context as _;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
};

const MAX_PACKAGE: usize = 8 * 1024 * 1024;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    api: u32,
    enabled: BTreeMap<String, String>,
}
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
type Files = Vec<(PathBuf, Vec<u8>, u32)>;

fn no_links(p: &Path) -> Result<()> {
    ensure!(p.is_absolute(), "Требуется полный путь пакета");
    for part in p.ancestors() {
        ensure!(
            !fs::symlink_metadata(part)?.is_symlink(),
            "Ссылка в пути пакета: {}",
            part.display()
        );
    }
    Ok(())
}
fn read_file(p: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(
            rustix::fs::OFlags::NOFOLLOW.bits() as i32 | rustix::fs::OFlags::NONBLOCK.bits() as i32,
        )
        .open(p)?;
    ensure!(
        file.metadata()?.is_file(),
        "Пакет содержит специальный файл"
    );
    let mut data = Vec::new();
    Read::by_ref(&mut file)
        .take(limit as u64 + 1)
        .read_to_end(&mut data)?;
    ensure!(data.len() <= limit, "Пакет или файл слишком велик");
    Ok(data)
}
fn collect(
    root: &Path,
    rel: &Path,
    files: &mut Files,
    budget: &mut usize,
    count: &mut usize,
    cancel: &Cancellation,
) -> Result<()> {
    crate::fs::operation_checkpoint(cancel)?;
    ensure!(rel.components().count() <= 8, "Пакет глубже 8 уровней");
    no_links(&root.join(rel))?;
    for e in fs::read_dir(root.join(rel))? {
        crate::fs::operation_checkpoint(cancel)?;
        let e = e?;
        *count += 1;
        ensure!(*count <= 128, "Пакет содержит больше 128 записей");
        let child = rel.join(e.file_name());
        let meta = fs::symlink_metadata(e.path())?;
        if meta.is_dir() {
            collect(root, &child, files, budget, count, cancel)?;
        } else {
            ensure!(
                meta.is_file() && !meta.is_symlink(),
                "Пакет содержит ссылку или специальный файл"
            );
            let data = read_file(&e.path(), *budget)?;
            *budget -= data.len();
            let after = fs::symlink_metadata(e.path())?;
            ensure!(
                meta.dev() == after.dev()
                    && meta.ino() == after.ino()
                    && meta.len() == after.len()
                    && meta.modified()? == after.modified()?,
                "Пакет изменился во время чтения"
            );
            files.push((child, data, meta.mode() & 0o111));
        }
    }
    Ok(())
}
fn tree(root: &Path, cancel: &Cancellation) -> Result<(Files, String)> {
    no_links(root)?;
    let mut files = Vec::new();
    let mut budget = MAX_PACKAGE;
    collect(root, Path::new(""), &mut files, &mut budget, &mut 0, cancel)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut digest = Sha256::new();
    for (p, data, mode) in &files {
        let raw = bytes(p);
        digest.update((raw.len() as u64).to_le_bytes());
        digest.update(raw);
        digest.update(mode.to_le_bytes());
        digest.update((data.len() as u64).to_le_bytes());
        digest.update(data);
    }
    Ok((
        files,
        digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}
fn package(root: &Path, cancel: &Cancellation) -> Result<(Manifest, Files, String)> {
    let (files, digest) = tree(root, cancel)?;
    let manifest = files
        .iter()
        .find(|f| f.0 == Path::new("basalgrim-plugin.json"))
        .context("Нет basalgrim-plugin.json")?;
    ensure!(manifest.1.len() <= 32 * 1024, "Манифест больше 32 KiB");
    let manifest: Manifest = serde_json::from_slice(&manifest.1).context("Ошибка манифеста")?;
    manifest.validate()?;
    ensure!(
        files
            .iter()
            .any(|f| f.0 == Path::new(&manifest.executable) && f.2 != 0),
        "Исполнитель отсутствует или не имеет права запуска"
    );
    Ok((manifest, files, digest))
}
impl Store {
    pub fn desktop() -> Option<Self> {
        Some(Self {
            root: dirs::data_dir()?.join("molten-zharr/basalgrim/extensions"),
        })
    }
    fn guard(&self) -> Result<()> {
        no_links(&self.root)?;
        let m = fs::symlink_metadata(&self.root)?;
        ensure!(
            m.is_dir() && m.uid() == rustix::process::getuid().as_raw() && m.mode() & 0o022 == 0,
            "Небезопасная папка расширений"
        );
        Ok(())
    }
    fn registry(&self) -> Result<Registry> {
        self.guard()?;
        let file = self.root.join("registry.json");
        match read_file(&file, 16 * 1024) {
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(Registry {
                    api: 1,
                    ..Default::default()
                })
            }
            Err(e) => Err(e),
            Ok(data) => {
                let r: Registry = serde_json::from_slice(&data)
                    .context("Повреждён реестр расширений; файл сохранён")?;
                ensure!(
                    r.api == 1
                        && r.enabled.len() <= MAX_PLUGINS
                        && r.enabled
                            .iter()
                            .all(|(k, v)| identifier(k) && v.len() == 64),
                    "Неизвестный формат реестра расширений"
                );
                Ok(r)
            }
        }
    }
    fn save(&self, registry: &Registry) -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(&self.root)?;
        temp.write_all(&serde_json::to_vec(registry)?)?;
        temp.as_file().sync_all()?;
        temp.persist(self.root.join("registry.json"))?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    fn lock(&self, cancel: &Cancellation) -> Result<File> {
        if !self.root.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.root)?;
        }
        self.guard()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(self.root.join("registry.lock"))?;
        ensure!(
            lock.metadata()?.is_file()
                && lock.metadata()?.uid() == rustix::process::getuid().as_raw(),
            "Некорректная блокировка реестра"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            crate::fs::operation_checkpoint(cancel)?;
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => return Ok(lock),
                Err(rustix::io::Errno::WOULDBLOCK) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    pub fn load(&self, cancel: &Cancellation) -> Result<Vec<Plugin>> {
        let _lock = self.lock(cancel)?;
        self.catalog(cancel)
    }
    fn catalog(&self, cancel: &Cancellation) -> Result<Vec<Plugin>> {
        let registry = self.registry()?;
        let mut plugins = Vec::new();
        let mut entries = 0;
        for e in fs::read_dir(&self.root)? {
            let e = e?;
            entries += 1;
            ensure!(entries <= 128, "Слишком много записей в папке расширений");
            let name = e.file_name().to_string_lossy().into_owned();
            if !identifier(&name) || !e.file_type()?.is_dir() {
                continue;
            }
            ensure!(plugins.len() < MAX_PLUGINS, "Допускается до 32 расширений");
            let loaded = package(&e.path(), cancel).and_then(|p| {
                ensure!(p.0.id == name, "Имя папки не совпадает с id расширения");
                Ok(p)
            });
            let (manifest, digest, error) = match loaded {
                Ok((m, _, digest)) => (m, digest, None),
                Err(e) => {
                    crate::fs::operation_checkpoint(cancel)?;
                    let digest = tree(&self.root.join(&name), cancel)
                        .map(|(_, d)| d)
                        .unwrap_or_default();
                    (
                        Manifest {
                            api: 1,
                            id: name.clone(),
                            name: name.clone(),
                            version: "?".into(),
                            description: String::new(),
                            executable: String::new(),
                            commands: vec![],
                            preview: vec![],
                            events: vec![],
                        },
                        digest,
                        Some(format!("{e:#}")),
                    )
                }
            };
            let approved = registry.enabled.get(&name);
            let changed = approved.is_some_and(|old| old != &digest);
            plugins.push(Plugin {
                manifest,
                directory: e.path(),
                digest,
                enabled: approved.is_some() && !changed && error.is_none(),
                error: error.or_else(|| {
                    changed.then(|| "Пакет изменён. Проверьте его и включите заново".into())
                }),
                lease: Cancellation::default(),
            });
        }
        plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        Ok(plugins)
    }
    pub fn install(&self, source: &Path, cancel: &Cancellation) -> Result<String> {
        let (manifest, files, _) = package(source, cancel)?;
        let _lock = self.lock(cancel)?;
        self.registry()?;
        ensure!(
            self.catalog(cancel)?.len() < MAX_PLUGINS,
            "Допускается до 32 расширений"
        );
        let target = self.root.join(&manifest.id);
        ensure!(
            !target.try_exists()?,
            "Расширение уже установлено; сначала удалите прежнюю версию"
        );
        let stage = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.root)?;
        for (rel, data, mode) in files {
            crate::fs::operation_checkpoint(cancel)?;
            let dest = stage.path().join(rel);
            fs::create_dir_all(dest.parent().unwrap())?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600 | mode)
                .open(dest)?;
            file.write_all(&data)?;
            file.sync_all()?;
        }
        crate::fs::operation_checkpoint(cancel)?;
        crate::fs::rename_new(stage.path(), &target)?;
        File::open(&self.root)?.sync_all()?;
        Ok(format!("{} установлено и выключено", manifest.name))
    }
    pub fn toggle(&self, plugin: &Plugin, cancel: &Cancellation) -> Result<String> {
        let _lock = self.lock(cancel)?;
        self.check_package(plugin, cancel)?;
        let mut registry = self.registry()?;
        if plugin.enabled {
            registry.enabled.remove(&plugin.manifest.id);
        } else {
            registry
                .enabled
                .insert(plugin.manifest.id.clone(), plugin.digest.clone());
        }
        self.save(&registry)?;
        Ok(if plugin.enabled {
            "Расширение выключено"
        } else {
            "Расширение включено"
        }
        .into())
    }
    pub fn remove(&self, plugin: &Plugin, cancel: &Cancellation) -> Result<String> {
        let _lock = self.lock(cancel)?;
        ensure!(
            plugin.directory == self.root.join(&plugin.manifest.id),
            "Пакет вне каталога расширений"
        );
        ensure!(
            !plugin.digest.is_empty() && tree(&plugin.directory, cancel)?.1 == plugin.digest,
            "Пакет изменился; обновите список"
        );
        let mut registry = self.registry()?;
        registry.enabled.remove(&plugin.manifest.id);
        self.save(&registry)?;
        crate::fs::operation_checkpoint(cancel)?;
        // Only a validated managed copy is removed; installation sources stay outside it.
        let parked = tempfile::Builder::new()
            .prefix(".remove-")
            .tempdir_in(&self.root)?;
        crate::fs::rename_new(&plugin.directory, &parked.path().join("package"))?;
        parked.close()?;
        Ok("Расширение удалено".into())
    }
    fn check_package(&self, plugin: &Plugin, cancel: &Cancellation) -> Result<()> {
        ensure!(
            plugin.directory == self.root.join(&plugin.manifest.id),
            "Пакет вне каталога расширений"
        );
        let (_, _, digest) = package(&plugin.directory, cancel)?;
        ensure!(digest == plugin.digest, "Пакет изменился; обновите список");
        Ok(())
    }
    pub(super) fn authorized(plugin: &Plugin, cancel: &Cancellation) -> Result<()> {
        let store = Self {
            root: plugin
                .directory
                .parent()
                .context("Нет папки расширений")?
                .into(),
        };
        store.check_package(plugin, cancel)?;
        ensure!(
            store.registry()?.enabled.get(&plugin.manifest.id) == Some(&plugin.digest),
            "Расширение выключено или изменено"
        );
        Ok(())
    }
}

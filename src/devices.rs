//! UDisks discovery and explicit mount/eject requests, off the UI thread.
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
mod udisks;
pub use udisks::UDisks;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Drive {
    pub path: String,
    pub identity: String,
    pub detected: u64,
    pub media_detected: u64,
    pub label: String,
    pub removable: bool,
    pub media_available: bool,
    pub power_off: bool,
    pub eject: bool,
    pub sibling: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Volume {
    pub path: String,
    pub drive: String,
    pub device: PathBuf,
    pub number: u64,
    pub uuid: String,
    pub label: String,
    pub size: u64,
    pub fs: String,
    pub filesystem: bool,
    pub system: bool,
    pub ignored: bool,
    pub mounts: Vec<PathBuf>,
}
impl Volume {
    pub fn protected(&self) -> bool {
        self.system
            || self.mounts.iter().any(|p| {
                matches!(
                    p.to_str(),
                    Some(
                        "/" | "/home"
                            | "/usr"
                            | "/var"
                            | "/boot"
                            | "/boot/efi"
                            | "/run"
                            | "/etc"
                            | "/dev"
                            | "/proc"
                            | "/sys"
                    )
                )
            })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub owner: String,
    pub drives: Vec<Drive>,
    pub volumes: Vec<Volume>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub key: String,
    pub label: String,
    pub volume: Option<Volume>,
    pub drive: Option<Drive>,
}
impl Device {
    pub fn mounted(&self) -> bool {
        self.volume.as_ref().is_some_and(|v| !v.mounts.is_empty())
    }
    pub fn openable(&self) -> bool {
        self.volume
            .as_ref()
            .is_some_and(|v| v.filesystem && !v.protected())
    }
    pub fn ejectable(&self) -> bool {
        self.drive
            .as_ref()
            .is_some_and(|d| d.removable && (d.power_off || d.eject))
    }
    pub fn state(&self) -> &'static str {
        if self.mounted() {
            "Подключён"
        } else if self.openable() {
            "Не подключён"
        } else if self.drive.as_ref().is_some_and(|d| !d.media_available) {
            "Нет носителя"
        } else {
            "Нет доступного раздела"
        }
    }
}
impl Snapshot {
    pub fn devices(&self) -> Vec<Device> {
        let mut result = Vec::new();
        for v in &self.volumes {
            if v.ignored || v.protected() || !v.filesystem {
                continue;
            }
            let drive = self.drives.iter().find(|d| d.path == v.drive).cloned();
            result.push(Device {
                key: v.path.clone(),
                label: v.label.clone(),
                volume: Some(v.clone()),
                drive,
            });
        }
        for d in &self.drives {
            if d.removable && !result.iter().any(|r| r.drive.as_ref() == Some(d)) {
                result.push(Device {
                    key: d.path.clone(),
                    label: d.label.clone(),
                    volume: None,
                    drive: Some(d.clone()),
                });
            }
        }
        result.sort_by(|a, b| a.label.cmp(&b.label).then(a.key.cmp(&b.key)));
        result
    }
    fn group(&self, target: &Device, eject: bool) -> (Vec<Drive>, Vec<Volume>) {
        if eject && let Some(d) = &target.drive {
            let drives: Vec<_> = self
                .drives
                .iter()
                .filter(|other| {
                    other.path == d.path
                        || (d.power_off && !d.sibling.is_empty() && other.sibling == d.sibling)
                })
                .cloned()
                .collect();
            let volumes = self
                .volumes
                .iter()
                .filter(|v| drives.iter().any(|d| d.path == v.drive))
                .cloned()
                .collect();
            (drives, volumes)
        } else {
            (
                target.drive.iter().cloned().collect(),
                target.volume.iter().cloned().collect(),
            )
        }
    }
    pub fn plan(&self, key: &str, kind: Kind) -> Result<Plan> {
        let device = self
            .devices()
            .into_iter()
            .find(|d| d.key == key)
            .context("Накопитель больше не доступен")?;
        ensure!(
            match kind {
                Kind::Open | Kind::Mount => device.openable(),
                Kind::Unmount => device.mounted(),
                Kind::Eject => device.ejectable(),
            },
            "Действие недоступно для накопителя"
        );
        let (drives, volumes) = self.group(&device, kind == Kind::Eject);
        ensure!(
            !volumes.iter().any(Volume::protected),
            "Устройство содержит системный раздел"
        );
        Ok(Plan {
            owner: self.owner.clone(),
            device,
            drives,
            volumes,
            kind,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Open,
    Mount,
    Unmount,
    Eject,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub owner: String,
    pub device: Device,
    pub drives: Vec<Drive>,
    pub volumes: Vec<Volume>,
    pub kind: Kind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Mount,
    Unmount,
    PowerOff,
    Eject,
}
impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mount => "Mount",
            Self::Unmount => "Unmount",
            Self::PowerOff => "PowerOff",
            Self::Eject => "Eject",
        }
    }
}
pub trait Backend: Send {
    fn snapshot(&mut self) -> Result<Snapshot>;
    fn invoke(&mut self, owner: &str, path: &str, method: Method) -> Result<()>;
}
#[derive(Default, Debug)]
pub struct Outcome {
    pub opened: Option<PathBuf>,
    pub disconnected: Vec<PathBuf>,
    pub error: Option<String>,
}
fn revalidate(backend: &mut dyn Backend, plan: &Plan) -> Result<()> {
    let fresh = backend.snapshot()?;
    ensure!(
        fresh.owner == plan.owner,
        "Служба накопителей перезапущена; повторите действие"
    );
    let current = fresh.plan(&plan.device.key, plan.kind)?;
    let (drives, volumes) = (current.drives, current.volumes);
    ensure!(
        drives == plan.drives && volumes == plan.volumes,
        "Накопитель или его разделы изменились; повторите действие"
    );
    Ok(())
}
pub fn execute(backend: &mut dyn Backend, mut plan: Plan) -> Outcome {
    let mut outcome = Outcome::default();
    let result = (|| -> Result<()> {
        revalidate(backend, &plan)?;
        if matches!(plan.kind, Kind::Mount | Kind::Open) {
            let volume = plan
                .device
                .volume
                .as_ref()
                .context("Нет файловой системы")?;
            if volume.mounts.is_empty() {
                backend.invoke(&plan.owner, &volume.path, Method::Mount)?;
            }
            let after = backend.snapshot()?;
            ensure!(
                after.owner == plan.owner,
                "Служба накопителей изменилась после подключения"
            );
            let current = after
                .volumes
                .iter()
                .find(|v| v.path == volume.path)
                .context("Накопитель отключён после подключения")?;
            let mut expected = volume.clone();
            expected.mounts = current.mounts.clone();
            ensure!(
                &expected == current
                    && after
                        .drives
                        .iter()
                        .filter(|d| plan.drives.iter().any(|p| p.path == d.path))
                        .cloned()
                        .collect::<Vec<_>>()
                        == plan.drives,
                "Накопитель заменён после подключения"
            );
            let path = current
                .mounts
                .first()
                .context("Система не сообщила папку подключённого раздела")?;
            if plan.kind == Kind::Open {
                outcome.opened = Some(path.clone());
            }
            return Ok(());
        }
        for index in 0..plan.volumes.len() {
            if plan.volumes[index].mounts.is_empty() {
                continue;
            }
            revalidate(backend, &plan)?;
            let volume = &plan.volumes[index];
            backend.invoke(&plan.owner, &volume.path, Method::Unmount)?;
            outcome.disconnected.extend(volume.mounts.iter().cloned());
            plan.volumes[index].mounts.clear();
        }
        if plan.kind == Kind::Eject {
            revalidate(backend, &plan)?;
            let drive = plan.device.drive.as_ref().context("Нет устройства")?;
            backend.invoke(
                &plan.owner,
                &drive.path,
                if drive.power_off {
                    Method::PowerOff
                } else {
                    Method::Eject
                },
            )?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        outcome.error = Some(format!("{error:#}"));
    }
    outcome
}

enum Command {
    Refresh,
    Run(u64, Box<Plan>),
}
pub struct Completed {
    pub id: u64,
    pub kind: Kind,
    pub outcome: Outcome,
}
struct Runtime {
    sender: Option<mpsc::SyncSender<Command>>,
    snapshot: Arc<Mutex<Option<Result<Snapshot, String>>>>,
    completed: mpsc::Receiver<Completed>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Default)]
pub struct Monitor {
    pub snapshot: Snapshot,
    pub revision: u64,
    pub error: Option<String>,
    pub ready: bool,
    pub working: Option<u64>,
    pub departed: Vec<PathBuf>,
    runtime: Option<Runtime>,
}
impl Monitor {
    pub fn attach(&mut self, mut backend: Box<dyn Backend>) {
        let (sender, receiver) = mpsc::sync_channel(2);
        let snapshot = Arc::new(Mutex::new(None));
        let shared = snapshot.clone();
        let (done, completed) = mpsc::channel();
        let thread = thread::spawn(move || {
            loop {
                let result = backend.snapshot().map_err(|e| format!("{e:#}"));
                *shared.lock().unwrap() = Some(result);
                match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(Command::Run(id, plan)) => {
                        let kind = plan.kind;
                        let outcome = execute(backend.as_mut(), *plan);
                        let result = backend.snapshot().map_err(|e| format!("{e:#}"));
                        *shared.lock().unwrap() = Some(result);
                        if done.send(Completed { id, kind, outcome }).is_err() {
                            break;
                        }
                    }
                    Ok(Command::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        *self = Self {
            runtime: Some(Runtime {
                sender: Some(sender),
                snapshot,
                completed,
                thread: Some(thread),
            }),
            ..Default::default()
        };
    }
    pub fn refresh(&self) {
        if let Some(r) = &self.runtime {
            let _ = r.sender.as_ref().unwrap().try_send(Command::Refresh);
        }
    }
    pub fn run(&mut self, id: u64, plan: Plan) -> bool {
        if self.working.is_some() {
            return false;
        }
        let Some(r) = &self.runtime else {
            return false;
        };
        if r.sender
            .as_ref()
            .unwrap()
            .try_send(Command::Run(id, Box::new(plan)))
            .is_err()
        {
            return false;
        }
        self.working = Some(id);
        true
    }
    pub fn poll(&mut self) -> Vec<Completed> {
        let mut result = Vec::new();
        if let Some(r) = &self.runtime {
            while let Ok(event) = r.completed.try_recv() {
                if self.working == Some(event.id) {
                    self.working = None;
                    result.push(event);
                }
            }
            if let Some(snapshot) = r.snapshot.lock().unwrap().take() {
                match snapshot {
                    Ok(snapshot) => {
                        if snapshot != self.snapshot || self.error.is_some() || !self.ready {
                            self.revision += 1;
                            let roots: std::collections::BTreeSet<_> =
                                snapshot.volumes.iter().flat_map(|v| &v.mounts).collect();
                            self.departed.extend(
                                self.snapshot
                                    .volumes
                                    .iter()
                                    .flat_map(|v| &v.mounts)
                                    .filter(|p| !roots.contains(p))
                                    .cloned(),
                            );
                        }
                        self.snapshot = snapshot;
                        self.error = None;
                        self.ready = true;
                    }
                    Err(error) => {
                        if self.error.as_ref() != Some(&error) {
                            self.revision += 1;
                        }
                        self.error = Some(error);
                        self.ready = false;
                    }
                }
            }
        }
        result
    }
}

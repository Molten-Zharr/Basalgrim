use super::*;
use gio::{AskPasswordFlags as Flags, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

#[derive(Default)]
pub struct Gvfs {
    monitor: Option<gio::VolumeMonitor>,
    serial: u64,
    mounts: Vec<(gio::Mount, String)>,
}
impl Gvfs {
    fn mounts(&mut self) -> Vec<gio::Mount> {
        self.monitor
            .get_or_insert_with(gio::VolumeMonitor::get)
            .mounts()
    }
    fn identity(&mut self, mount: &gio::Mount) -> String {
        if let Some((_, id)) = self.mounts.iter().find(|(m, _)| m == mount) {
            return id.clone();
        }
        self.serial += 1;
        let id = self.serial.to_string();
        self.mounts.push((mount.clone(), id.clone()));
        id
    }
    fn location(&mut self, mount: &gio::Mount, path: Option<PathBuf>) -> Result<Location> {
        let root = mount.root();
        let uri = address(root.uri().as_str())?;
        let root = root
            .path()
            .context("Нет доступа к папке через GVfs. Установите gvfs-fuse и подключитесь снова")?;
        ensure!(root.is_absolute(), "GVfs вернул некорректную папку");
        Ok(Location {
            uri,
            label: crate::fs::clean(&mount.name().chars().take(256).collect::<String>()),
            path: path.unwrap_or_else(|| root.clone()),
            root,
            identity: self.identity(mount),
        })
    }
}

struct Prompts {
    busy: Rc<Cell<bool>>,
    pending: Rc<RefCell<Option<(u64, gio::MountOperation)>>>,
}
impl Prompts {
    fn attach(op: &gio::MountOperation, io: &Interaction) -> Self {
        let pending = Rc::new(RefCell::new(None));
        let serial = Rc::new(Cell::new(0));
        let out = io.events.clone();
        let operation = io.operation;
        let (p, s) = (pending.clone(), serial.clone());
        op.connect_ask_password(move |op, message, user, domain, flags| {
            op.stop_signal_emission_by_name("ask-password");
            let id = s.get() + 1;
            s.set(id);
            if id > 16 {
                op.reply(gio::MountOperationResult::Aborted);
                return;
            }
            *p.borrow_mut() = Some((id, op.clone()));
            if message.len() > 16384 || user.len() > 4096 || domain.len() > 4096 {
                op.reply(gio::MountOperationResult::Aborted);
                p.borrow_mut().take();
                return;
            }
            let value = Prompt::Password {
                message: message
                    .lines()
                    .map(crate::fs::clean)
                    .collect::<Vec<_>>()
                    .join("\n"),
                user: user.into(),
                domain: domain.into(),
                need_user: flags.contains(Flags::NEED_USERNAME),
                need_password: flags.contains(Flags::NEED_PASSWORD),
                need_domain: flags.contains(Flags::NEED_DOMAIN),
                anonymous: flags.contains(Flags::ANONYMOUS_SUPPORTED),
            };
            if out
                .send(Event::Prompt {
                    operation,
                    prompt: id,
                    value,
                })
                .is_err()
            {
                op.reply(gio::MountOperationResult::Aborted);
            }
        });
        let (p, s, out) = (pending.clone(), serial.clone(), io.events.clone());
        op.connect_local("ask-question", false, move |values| {
            let op = values[0].get::<gio::MountOperation>().unwrap();
            op.stop_signal_emission_by_name("ask-question");
            let message = values[1].get::<String>().unwrap();
            let choices = values[2].get::<glib::StrV>().unwrap();
            if message.len() > 16384
                || choices.is_empty()
                || choices.len() > 8
                || choices.iter().any(|s| s.chars().count() > 60)
            {
                op.reply(gio::MountOperationResult::Aborted);
                return None;
            }
            let id = s.get() + 1;
            s.set(id);
            if id > 16 {
                op.reply(gio::MountOperationResult::Aborted);
                return None;
            }
            *p.borrow_mut() = Some((id, op.clone()));
            let value = Prompt::Question {
                message: message
                    .lines()
                    .map(crate::fs::clean)
                    .collect::<Vec<_>>()
                    .join("\n"),
                choices: choices.iter().map(|s| crate::fs::clean(s)).collect(),
            };
            if out
                .send(Event::Prompt {
                    operation,
                    prompt: id,
                    value,
                })
                .is_err()
            {
                op.reply(gio::MountOperationResult::Aborted);
            }
            None
        });
        // Never offer a force-unmount fallback, even if the service asks for it.
        let busy = Rc::new(Cell::new(false));
        let was_busy = busy.clone();
        op.connect_local("show-processes", false, move |values| {
            was_busy.set(true);
            values[0]
                .get::<gio::MountOperation>()
                .unwrap()
                .reply(gio::MountOperationResult::Aborted);
            None
        });
        let (p, out) = (pending.clone(), io.events.clone());
        op.connect_aborted(move |_| {
            if let Some((prompt, _)) = p.borrow_mut().take() {
                let _ = out.send(Event::Withdraw { operation, prompt });
            }
        });
        Self { pending, busy }
    }
}
fn wait<T>(
    done: Rc<RefCell<Option<Result<T, glib::Error>>>>,
    io: &Interaction,
    cancel: &gio::Cancellable,
    prompts: &Prompts,
) -> Result<T> {
    let context = glib::MainContext::ref_thread_default();
    let mut phase_start = Instant::now();
    let mut asking = false;
    let mut aborted = None;
    loop {
        for _ in 0..64 {
            if !context.pending() {
                break;
            }
            context.iteration(false);
        }
        if let Some(result) = done.borrow_mut().take() {
            if io.cancel.is_cancelled() {
                anyhow::bail!("Подключение отменено");
            }
            return result.map_err(|e| anyhow::anyhow!("{}", e.message()));
        }
        let now_asking = prompts.pending.borrow().is_some();
        if asking != now_asking {
            phase_start = Instant::now();
            asking = now_asking;
        }
        if io.cancel.is_cancelled()
            || phase_start.elapsed() > Duration::from_secs(if asking { 300 } else { 120 })
        {
            cancel.cancel();
            let pending = prompts.pending.borrow_mut().take();
            if let Some((_, op)) = pending {
                op.reply(gio::MountOperationResult::Aborted);
            }
            if aborted.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
                anyhow::bail!("Подключение отменено или сервер не ответил вовремя");
            }
        }
        if let Some((id, answer)) = io.answer() {
            let current = prompts.pending.borrow().as_ref().map(|(id, _)| *id);
            if current == Some(id) && !cancel.is_cancelled() {
                let (_, op) = prompts.pending.borrow_mut().take().unwrap();
                match answer {
                    Answer::Credentials {
                        user,
                        password,
                        domain,
                        anonymous,
                    } => {
                        op.set_username(Some(&user));
                        op.set_password(Some(&password));
                        op.set_domain(Some(&domain));
                        op.set_anonymous(anonymous);
                        op.set_password_save(gio::PasswordSave::Never);
                    }
                    Answer::Choice(choice) => op.set_choice(choice as i32),
                }
                op.reply(gio::MountOperationResult::Handled);
                op.set_password(None);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}
impl Backend for Gvfs {
    fn list(&mut self) -> Result<Vec<Location>> {
        let context = glib::MainContext::ref_thread_default();
        for _ in 0..64 {
            if !context.pending() {
                break;
            }
            context.iteration(false);
        }
        let mounts = self.mounts();
        ensure!(mounts.len() <= 1024, "Слишком много сетевых подключений");
        self.mounts.retain(|(m, _)| mounts.contains(m));
        Ok(mounts
            .iter()
            .filter_map(|m| self.location(m, None).ok())
            .collect())
    }
    fn connect(&mut self, uri: &str, io: &Interaction) -> Result<Location> {
        let uri = address(uri)?;
        let file = gio::File::for_uri(&uri);
        let op = gio::MountOperation::new();
        op.set_password_save(gio::PasswordSave::Never);
        let prompts = Prompts::attach(&op, io);
        let cancel = gio::Cancellable::new();
        let done = Rc::new(RefCell::new(None));
        let result = done.clone();
        file.mount_enclosing_volume(
            gio::MountMountFlags::NONE,
            Some(&op),
            Some(&cancel),
            move |r| {
                *result.borrow_mut() = Some(match r {
                    Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => Ok(()),
                    other => other,
                });
            },
        );
        wait(done, io, &cancel, &prompts).context("Не удалось подключиться через GVfs (нужны gvfs-backends и шина пользовательской сессии)")?;
        let done = Rc::new(RefCell::new(None));
        let result = done.clone();
        file.query_info_async(
            "standard::type",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
            Some(&cancel),
            move |r| *result.borrow_mut() = Some(r),
        );
        let info = wait(done, io, &cancel, &prompts)?;
        ensure!(
            info.file_type() == gio::FileType::Directory,
            "Этот адрес указывает на файл; укажите папку"
        );
        let mount = file.find_enclosing_mount(Some(&cancel))?;
        let path = file.path().context(
            "Подключение установлено, но нет доступа к папке. Установите gvfs-fuse и повторите",
        )?;
        let location = self.location(&mount, Some(path))?;
        ensure!(
            location.path.starts_with(&location.root),
            "Папка находится вне подключённого ресурса"
        );
        Ok(location)
    }
    fn disconnect(&mut self, location: &Location, io: &Interaction) -> Result<()> {
        let current = self.list()?;
        ensure!(
            current.iter().any(|l| l.identity == location.identity
                && l.uri == location.uri
                && l.root == location.root),
            "Подключение изменилось; повторите действие"
        );
        let mount = self
            .mounts
            .iter()
            .find(|(_, id)| id == &location.identity)
            .context("Подключение закрыто")?
            .0
            .clone();
        let op = gio::MountOperation::new();
        let prompts = Prompts::attach(&op, io);
        let cancel = gio::Cancellable::new();
        let done = Rc::new(RefCell::new(None));
        let result = done.clone();
        mount.unmount_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&op),
            Some(&cancel),
            move |r| *result.borrow_mut() = Some(r),
        );
        let result = wait(done, io, &cancel, &prompts);
        ensure!(
            !prompts.busy.get(),
            "Сетевая папка занята. Закройте файлы и повторите отключение"
        );
        result
    }
}

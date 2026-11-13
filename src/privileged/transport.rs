use super::*;
use crate::{
    conflicts::{Conflict, Decision, ItemInfo},
    fs::Report,
    operations::{Cancellation, Progress},
};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
enum Reply {
    Cancel,
    Decision { choice: u8, remaining: bool },
}
#[derive(Serialize, Deserialize)]
enum Event {
    Ready,
    Progress(Progress, Option<Vec<u8>>),
    Conflict {
        source: Vec<u8>,
        destination: Vec<u8>,
        incoming: Info,
        existing: Info,
        remaining: usize,
        replace: bool,
        keep: bool,
    },
    Done(Box<Outcome>),
}
#[derive(Serialize, Deserialize)]
struct Info {
    kind: u8,
    bytes: Option<u64>,
    modified: Option<std::time::SystemTime>,
}
impl Info {
    fn from(i: &ItemInfo) -> Self {
        Self {
            kind: match i.kind {
                "Папка" => 1,
                "Ссылка" => 2,
                _ => 0,
            },
            bytes: i.bytes,
            modified: i.modified,
        }
    }
    fn into(self) -> ItemInfo {
        ItemInfo {
            kind: match self.kind {
                1 => "Папка",
                2 => "Ссылка",
                _ => "Файл",
            },
            bytes: self.bytes,
            modified: self.modified,
        }
    }
}
type Retained = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);
#[derive(Serialize, Deserialize)]
struct Outcome {
    completed: Vec<Vec<u8>>,
    skipped: Vec<Vec<u8>>,
    total: usize,
    cancelled: bool,
    error: Option<String>,
    progress: Progress,
    retained: Vec<Retained>,
    recoveries: Vec<(Vec<u8>, Vec<u8>)>,
    renamed: Option<(Vec<u8>, Vec<u8>)>,
    items: Vec<crate::journal::Item>,
    root_id: i64,
}
impl Outcome {
    fn from(r: Report) -> Self {
        let root_id = r.journal.as_ref().map_or(0, |r| r.id);
        Self {
            root_id,
            completed: r.completed.iter().map(|p| bytes(p)).collect(),
            skipped: r.skipped.iter().map(|p| bytes(p)).collect(),
            total: r.total,
            cancelled: r.cancelled,
            error: r.error,
            progress: r.progress,
            retained: r
                .retained_copies
                .into_iter()
                .map(|r| {
                    (
                        bytes(&r.source),
                        bytes(&r.destination),
                        r.recovery.as_deref().map(bytes),
                    )
                })
                .collect(),
            recoveries: r
                .destination_recoveries
                .into_iter()
                .map(|r| (bytes(&r.destination), bytes(&r.backup)))
                .collect(),
            renamed: r.renamed.map(|(s, d)| (bytes(&s), bytes(&d))),
            items: r.journal.map(|r| r.items).unwrap_or_default(),
        }
    }
    fn report(self) -> Report {
        Report {
            total: self.total,
            completed: self.completed.iter().map(|p| path(p)).collect(),
            skipped: self.skipped.iter().map(|p| path(p)).collect(),
            error: self.error,
            cancelled: self.cancelled,
            progress: self.progress,
            retained_copies: self
                .retained
                .into_iter()
                .map(|(s, d, r)| crate::fs::RetainedCopy {
                    source: path(&s),
                    destination: path(&d),
                    recovery: r.as_deref().map(path),
                })
                .collect(),
            destination_recoveries: self
                .recoveries
                .into_iter()
                .map(|(d, b)| crate::conflicts::DestinationRecovery {
                    destination: path(&d),
                    backup: path(&b),
                })
                .collect(),
            renamed: self.renamed.map(|(s, d)| (path(&s), path(&d))),
            journal: Some(crate::journal::Receipt {
                id: self.root_id,
                items: self.items,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}
fn write_frame(w: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "Слишком большой ответ помощника");
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}
fn read_frame<T: serde::de::DeserializeOwned>(r: &mut impl Read) -> Result<T> {
    let mut size = [0; 4];
    r.read_exact(&mut size)?;
    let len = u32::from_be_bytes(size) as usize;
    ensure!(len <= MAX_FRAME, "Слишком большой запрос помощника");
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn failed(total: usize, error: impl std::fmt::Display) -> Report {
    Report {
        total,
        error: Some(error.to_string()),
        ..Default::default()
    }
}

pub fn run(
    plan: Plan,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&Conflict) -> Decision,
) -> Report {
    let total = plan.total();
    let command = (|| -> Result<_> {
        let exe = std::env::current_exe()?;
        ensure!(
            Path::new("/usr/bin/pkexec").is_file(),
            "Установите polkit (pkexec) и агент аутентификации"
        );
        let mut command = Command::new("/usr/bin/pkexec");
        command
            .args(["--disable-internal-agent"])
            .arg(exe)
            .arg("--privileged-helper");
        Ok(command)
    })();
    match command {
        Ok(command) => run_plan(plan, cancel, notify, resolve, command),
        Err(e) => {
            let mut r = failed(total, e);
            r.journal = Some(Default::default());
            r
        }
    }
}

/// Injectable process boundary for isolated tests. Production always uses pkexec above.
#[doc(hidden)]
pub fn run_with_command(
    request: Request,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&Conflict) -> Decision,
    command: Command,
) -> Report {
    let total = request.total();
    match Plan::capture(request) {
        Ok(plan) => run_plan(plan, cancel, notify, resolve, command),
        Err(e) => failed(total, format!("{e:#}")),
    }
}
fn run_plan(
    plan: Plan,
    cancel: &Cancellation,
    notify: &mut dyn FnMut(&Progress),
    resolve: &mut dyn FnMut(&Conflict) -> Decision,
    mut command: Command,
) -> Report {
    let total = plan.total();
    let result = (|| -> Result<Report> {
        let items = plan.specs();
        crate::fs::operation_checkpoint(cancel)?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Не удалось запустить помощник администратора")?;
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let errors = thread::spawn(move || {
            let mut saved = Vec::new();
            let mut chunk = [0; 1024];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let keep = n.min(8192usize.saturating_sub(saved.len()));
                saved.extend_from_slice(&chunk[..keep]);
            }
            saved
        });
        // Pipe EOF cancels the helper; bounded channel prevents output accumulation.
        let (tx, rx) = mpsc::sync_channel(2);
        let reader = thread::spawn(move || {
            loop {
                let result = read_frame::<Event>(&mut output).map_err(|e| e.to_string());
                let stop = result.is_err() || matches!(result, Ok(Event::Done(_)));
                if tx.send(result).is_err() || stop {
                    break;
                }
            }
        });
        let started = Instant::now();
        let mut ready = false;
        let mut sent_cancel = false;
        let mut observed = Report {
            total,
            ..Default::default()
        };
        let mut report = loop {
            if (cancel.is_cancelled() || !ready && started.elapsed() > Duration::from_secs(120))
                && !sent_cancel
            {
                sent_cancel = true;
                if ready {
                    let _ = write_frame(&mut input, &Reply::Cancel);
                } else {
                    // No request has been sent yet, so killing authentication cannot
                    // interrupt a filesystem mutation or leave an unreported item.
                    let _ = child.kill();
                }
            }
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(Ok(Event::Ready)) if !ready => {
                    ready = true;
                    if sent_cancel {
                        break Report {
                            total,
                            cancelled: true,
                            ..Default::default()
                        };
                    }
                    if let Err(e) = write_frame(&mut input, &plan) {
                        break failed(total, e);
                    }
                }
                Ok(Ok(Event::Progress(mut p, current))) => {
                    p.current = current.as_deref().map(path);
                    let index = p.items_done + p.items_skipped;
                    if index > 0 && index <= items.len() {
                        let spec = &items[index - 1];
                        if p.items_done > observed.completed.len() {
                            observed.completed.push(spec.source.clone());
                        }
                        if p.items_skipped > observed.skipped.len() {
                            observed.skipped.push(spec.source.clone());
                        }
                    }
                    observed.progress = p.clone();
                    notify(&p);
                }
                Ok(Ok(Event::Conflict {
                    source,
                    destination,
                    incoming,
                    existing,
                    remaining,
                    replace,
                    keep,
                })) => {
                    let c = Conflict {
                        source: path(&source),
                        destination: path(&destination),
                        incoming: incoming.into(),
                        existing: existing.into(),
                        remaining,
                        can_replace: replace,
                        can_keep_both: keep,
                    };
                    let d = if cancel.is_cancelled() {
                        Decision::CANCEL
                    } else {
                        resolve(&c)
                    };
                    let choice = match d.choice {
                        crate::conflicts::Choice::Replace => 0,
                        crate::conflicts::Choice::Skip => 1,
                        crate::conflicts::Choice::KeepBoth => 2,
                        crate::conflicts::Choice::Cancel => 3,
                    };
                    if let Err(e) = write_frame(
                        &mut input,
                        &Reply::Decision {
                            choice,
                            remaining: d.remaining,
                        },
                    ) {
                        break failed(total, e);
                    }
                }
                Ok(Ok(Event::Done(outcome))) => break outcome.report(),
                Ok(Ok(Event::Ready)) => break failed(total, "Повторный запуск помощника отклонён"),
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break if sent_cancel && !ready {
                        Report {
                            total,
                            cancelled: true,
                            ..Default::default()
                        }
                    } else {
                        failed(
                            total,
                            if ready {
                                "Связь с помощником потеряна. Результат требует проверки; автоматического повтора нет"
                            } else {
                                "Авторизация не получена. Нужны polkit и работающий агент аутентификации; операция не выполнена"
                            },
                        )
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        drop(input); // cooperative cancellation, including cleanup; never kill after Ready
        drop(rx);
        let status = child.wait();
        let _ = reader.join();
        let stderr = errors.join().unwrap_or_default();
        if report.journal.is_none() && ready {
            report.completed = observed.completed;
            report.skipped = observed.skipped;
            report.progress = observed.progress;
        }
        if !ready && status.as_ref().is_ok_and(|s| s.code() == Some(126)) {
            report.cancelled = true;
            report.error = None;
        } else if report.error.is_some() && !stderr.is_empty() {
            crate::journal::append_error(
                &mut report,
                &crate::fs::clean(String::from_utf8_lossy(&stderr).trim()),
            );
        }
        if !ready {
            report.journal = Some(Default::default());
        }
        Ok(report)
    })();
    match result {
        Ok(r) => r,
        Err(e) => {
            if cancel.is_cancelled() {
                Report {
                    total,
                    cancelled: true,
                    journal: Some(Default::default()),
                    ..Default::default()
                }
            } else {
                let mut r = failed(total, format!("{e:#}"));
                r.journal = Some(Default::default());
                r
            }
        }
    }
}

/// Shared protocol/operation engine, exercised without privileges against private fixtures.
#[doc(hidden)]
pub fn serve(
    mut input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    mut database: crate::journal::Database,
    uid: u32,
) -> Result<()> {
    let output = Arc::new(Mutex::new(output));
    write_frame(&mut *output.lock().unwrap(), &Event::Ready)?;
    let plan: Plan = read_frame(&mut input)?;
    let cancel = Cancellation::default();
    let cancelled = cancel.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    // Exactly one plan; subsequent input can only cancel or answer a conflict.
    thread::spawn(move || {
        loop {
            match read_frame::<Reply>(&mut input) {
                Ok(Reply::Cancel) | Err(_) => {
                    cancelled.cancel();
                    break;
                }
                Ok(decision) => {
                    if tx.try_send(decision).is_err() {
                        cancelled.cancel();
                        break;
                    }
                }
            }
        }
    });
    let mut notify = |p: &Progress| {
        if write_frame(
            &mut *output.lock().unwrap(),
            &Event::Progress(p.clone(), p.current.as_deref().map(bytes)),
        )
        .is_err()
        {
            cancel.cancel();
        }
    };
    let mut resolve = |c: &Conflict| {
        let event = Event::Conflict {
            source: bytes(&c.source),
            destination: bytes(&c.destination),
            incoming: Info::from(&c.incoming),
            existing: Info::from(&c.existing),
            remaining: c.remaining,
            replace: c.can_replace,
            keep: c.can_keep_both,
        };
        if write_frame(&mut *output.lock().unwrap(), &event).is_err() {
            return Decision::CANCEL;
        }
        loop {
            if cancel.is_cancelled() {
                return Decision::CANCEL;
            }
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(Reply::Decision { choice, remaining }) => {
                    return Decision {
                        choice: match choice {
                            0 => crate::conflicts::Choice::Replace,
                            1 => crate::conflicts::Choice::Skip,
                            2 => crate::conflicts::Choice::KeepBoth,
                            _ => crate::conflicts::Choice::Cancel,
                        },
                        remaining,
                    };
                }
                Ok(Reply::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Decision::CANCEL;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    };
    let result = (|| -> Result<Report> {
        // Validate known evidence and fill permission-denied gaps from the same
        // snapshot, then retain it for the second check after journal creation.
        let plan = plan.authorize(uid)?;
        plan.check_trees(uid, &cancel)?;
        let operation = plan.operation()?;
        let mut recorder = database.begin(&operation, 0)?;
        recorder.disable_replay(UNDO);
        if let Some(trash) = &plan.trash {
            recorder.set_trash_user(uid, path(&trash.data));
        }
        // Authentication and durable journal creation may have blocked.
        if let Err(e) = plan.validate(uid) {
            let mut report = failed(plan.total(), format!("{e:#}"));
            recorder.finish(&mut report);
            return Ok(report);
        }
        let mut trash = |source: &Path| {
            plan.trash
                .as_ref()
                .context("Нет контекста корзины")?
                .send(uid, source)
        };
        Ok(crate::fs::operate_with_trash(
            operation,
            &cancel,
            &mut notify,
            &mut resolve,
            &mut recorder,
            (plan.kind == Kind::Trash).then_some(&mut trash as &mut dyn FnMut(&Path) -> Result<()>),
        ))
    })();
    let report = result.unwrap_or_else(|e| {
        if crate::fs::operation_cancelled(&e) {
            Report {
                total: plan.total(),
                cancelled: true,
                ..Default::default()
            }
        } else {
            failed(plan.total(), format!("{e:#}"))
        }
    });
    write_frame(
        &mut *output.lock().unwrap(),
        &Event::Done(Box::new(Outcome::from(report))),
    )
}

pub fn helper_entry() -> Option<Result<()>> {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new("--privileged-helper")) {
        return None;
    }
    Some((|| {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            std::env::args_os().count() == 2 && rustix::process::geteuid().is_root(),
            "Помощник запускается только через pkexec"
        );
        let uid: u32 = std::env::var("PKEXEC_UID")
            .context("Нет инициатора pkexec")?
            .parse()?;
        ensure!(uid != 0, "Не запускайте Basalgrim целиком от root");
        rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o022));
        // Never open the caller's SQLite database as root. Each helper owns a
        // root-only durable log, isolated from both user data and other callers.
        let mut dir = fs::File::open("/var/lib")?;
        for name in [
            "basalgrim".to_string(),
            "privileged".into(),
            uid.to_string(),
        ] {
            use rustix::fs::{Mode, OFlags, mkdirat, openat};
            match mkdirat(&dir, name.as_str(), Mode::from_raw_mode(0o700)) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
            dir = fs::File::from(openat(
                &dir,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            let m = dir.metadata()?;
            ensure!(
                m.uid() == 0 && m.mode() & 0o077 == 0,
                "Каталог системного журнала имеет небезопасные права"
            );
        }
        let database = crate::journal::Database::open(&PathBuf::from(format!(
            "/var/lib/basalgrim/privileged/{uid}/operations.sqlite3"
        )))?;
        serve(std::io::stdin(), std::io::stdout(), database, uid)
    })())
}

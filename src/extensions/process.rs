use super::*;
use anyhow::{Context as _, bail};
use std::{
    io::{Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Command as Process, Stdio},
    time::{Duration, Instant},
};

fn nonblocking(fd: &impl AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}
fn drain(fd: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> Result<bool> {
    let mut buf = [0; 4096];
    loop {
        match fd.read(&mut buf) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                ensure!(
                    output.len() + n <= limit,
                    "Расширение превысило лимит вывода"
                );
                output.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
}
pub(super) fn run(
    plugin: &Plugin,
    request: &Request<'_>,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Response> {
    ensure!(
        !rustix::process::geteuid().is_root(),
        "Расширения не запускаются от root"
    );
    Store::authorized(plugin, cancel)?;
    request.context.validate()?;
    crate::fs::operation_checkpoint(cancel)?;
    crate::fs::operation_checkpoint(&plugin.lease)?;
    let input = serde_json::to_vec(request)?;
    ensure!(
        input.len() <= MAX_OUTPUT,
        "Слишком большой запрос расширения"
    );
    let mut command = Process::new(plugin.directory.join(&plugin.manifest.executable));
    command
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("BASALGRIM_PLUGIN_DIR", &plugin.directory)
        .current_dir(path(&request.context.cwd))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The plugin is trusted user code, not a sandbox. Bounds protect UI resources.
    let parent = rustix::process::getpid();
    unsafe {
        command.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            if rustix::process::getppid() != Some(parent) {
                return Err(std::io::Error::other("Parent exited"));
            }
            use rustix::process::{Resource, Rlimit, setrlimit};
            for (resource, value) in [
                (Resource::As, 512 * 1024 * 1024),
                (Resource::Nofile, 128),
                (Resource::Cpu, 30),
            ] {
                setrlimit(
                    resource,
                    Rlimit {
                        current: Some(value),
                        maximum: Some(value),
                    },
                )?;
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("Не удалось запустить расширение")?;
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    let result = (|| -> Result<Response> {
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        nonblocking(stdin.as_ref().unwrap())?;
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        let mut written = 0;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let deadline = Instant::now() + timeout;
        loop {
            crate::fs::operation_checkpoint(cancel)?;
            crate::fs::operation_checkpoint(&plugin.lease)?;
            ensure!(
                Instant::now() < deadline,
                "Расширение не ответило за {} с",
                timeout.as_secs()
            );
            if let Some(pipe) = &mut stdin {
                match pipe.write(&input[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
                if written == input.len() {
                    stdin = None;
                }
            }
            drain(&mut stdout, &mut out, MAX_OUTPUT)?;
            drain(&mut stderr, &mut err, 8192)?;
            if let Some(status) = child.try_wait()? {
                drain(&mut stdout, &mut out, MAX_OUTPUT)?;
                drain(&mut stderr, &mut err, 8192)?;
                if !status.success() {
                    bail!(
                        "Расширение завершилось с {status}: {}",
                        crate::fs::clean(String::from_utf8_lossy(&err).trim())
                    );
                }
                let response: Response =
                    serde_json::from_slice(&out).context("Расширение вернуло некорректный JSON")?;
                ensure!(
                    response.message.len() <= 2048,
                    "Сообщение расширения больше 2 KiB"
                );
                return Ok(response);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    // Reap the child and its ordinary process group on every exit, including success.
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    let _ = child.kill();
    let _ = child.wait();
    result
}

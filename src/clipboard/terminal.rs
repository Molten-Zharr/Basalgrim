//! Bounded local clipboard helpers, with an OSC 52 fallback for remote terminals.
use super::{Destination, TextCopy};
use std::os::unix::process::CommandExt;
use std::{
    io::{self, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

pub struct ResultCopy {
    pub id: u64,
    pub result: Result<Delivery, String>,
}
pub enum Delivery {
    System,
    Osc52(String),
}
pub struct TerminalClipboard {
    requests: Option<SyncSender<TextCopy>>,
    pub results: Receiver<ResultCopy>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Default for TerminalClipboard {
    fn default() -> Self {
        Self::new()
    }
}
impl TerminalClipboard {
    pub fn new() -> Self {
        let (requests, input) = mpsc::sync_channel::<TextCopy>(1);
        let (send, results) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|s| !s.is_empty());
        let x11 = std::env::var_os("DISPLAY").is_some_and(|s| !s.is_empty());
        let osc = std::env::var("TERM").is_ok_and(|s| !s.is_empty() && s != "dumb" && s != "linux");
        let thread = thread::spawn(move || {
            while let Ok(mut request) = input.recv() {
                for newer in input.try_iter() {
                    request = newer;
                }
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let mut copied = false;
                for (name, args, enabled) in [
                    (
                        "wl-copy",
                        &["--type", "text/plain;charset=utf-8"][..],
                        wayland,
                    ),
                    ("xclip", &["-selection", "clipboard", "-in"][..], x11),
                    ("xsel", &["--clipboard", "--input"][..], x11),
                ] {
                    if enabled && helper(name, args, &request.text, &stopping).is_ok() {
                        copied = true;
                        break;
                    }
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                }
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let result = if copied {
                    Ok(Delivery::System)
                } else if osc {
                    Ok(Delivery::Osc52(osc52(&request.text)))
                } else {
                    Err("нет доступного системного буфера или поддержки OSC 52 в терминале".into())
                };
                if send
                    .send(ResultCopy {
                        id: request.id,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests: Some(requests),
            results,
            stop,
            thread: Some(thread),
        }
    }
    pub fn submit(&self, request: TextCopy) -> Result<(), mpsc::TrySendError<TextCopy>> {
        self.requests.as_ref().unwrap().try_send(request)
    }
}
impl Drop for TerminalClipboard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub fn osc52(text: &str) -> String {
    use crossterm::Command;
    let mut sequence = String::new();
    crossterm::clipboard::CopyToClipboard::to_clipboard_from(text)
        .write_ansi(&mut sequence)
        .expect("String formatting");
    sequence
}
fn stop(child: &mut Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
fn helper(name: &str, args: &[&str], text: &str, stopping: &AtomicBool) -> io::Result<()> {
    let mut child = Command::new(name)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let result = (|| {
        let mut input = child.stdin.take();
        if let Some(input) = &input {
            let flags = rustix::fs::fcntl_getfl(input)?;
            rustix::fs::fcntl_setfl(input, flags | rustix::fs::OFlags::NONBLOCK)?;
        }
        let deadline = Instant::now() + Duration::from_millis(750);
        let mut offset = 0;
        loop {
            if stopping.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "clipboard stopped",
                ));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "clipboard timeout"));
            }
            if let Some(pipe) = &mut input {
                match pipe.write(&text.as_bytes()[offset..]) {
                    Ok(0) if offset < text.len() => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(written) => offset += written,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
                if offset == text.len() {
                    input.take();
                }
            }
            if let Some(status) = child.try_wait()? {
                return if status.success() && offset == text.len() {
                    Ok(())
                } else {
                    Err(io::Error::other("clipboard helper failed"))
                };
            }
            thread::sleep(Duration::from_millis(8));
        }
    })();
    if result.is_err() {
        stop(&mut child);
    }
    result
}
impl Delivery {
    pub fn destination(&self) -> Destination {
        match self {
            Self::System => Destination::System,
            Self::Osc52(_) => Destination::Terminal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn helper_writes_exact_text_without_shell_interpretation() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("clipboard");
        let result = root.path().join("copied");
        std::fs::write(&script, "#!/bin/sh\ncat > \"$1\"\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let text = "/tmp/Кириллица spaces/$(literal)`literal`\n";
        helper(
            script.to_str().unwrap(),
            &[result.to_str().unwrap()],
            text,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(result).unwrap(), text);
    }

    #[test]
    fn stalled_reader_times_out_without_blocking_on_a_full_pipe() {
        let start = Instant::now();
        let error = helper(
            "sh",
            &["-c", "sleep 30"],
            &"x".repeat(1 << 20),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn osc52_encodes_controls_inside_the_payload() {
        assert_eq!(osc52("a\x1b\n"), "\x1b]52;c;YRsK\x1b\\");
    }
}

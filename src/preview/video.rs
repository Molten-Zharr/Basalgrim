//! A single video frame, decoded by a cancellable, bounded FFmpeg child.
use super::{ImagePreview, decode::Decoded};
use crate::operations::Cancellation;
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::{Read, Seek},
    os::{fd::AsFd, unix::process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

pub fn candidate(path: &Path, prefix: &[u8]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "mp4"
                    | "mkv"
                    | "webm"
                    | "avi"
                    | "mov"
                    | "m4v"
                    | "mpeg"
                    | "mpg"
                    | "m2ts"
                    | "ogv"
                    | "flv"
                    | "3gp"
            )
        })
        || prefix.get(4..8) == Some(b"ftyp")
            && prefix.get(8..12).is_some_and(|brand| {
                matches!(
                    brand,
                    b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1" | b"qt  " | b"M4V " | b"dash"
                )
            })
        || [0, 188, 376]
            .into_iter()
            .all(|i| prefix.get(i) == Some(&0x47))
        || prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
        || prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"AVI ")
}

fn command(program: &str, file: &mut File) -> Result<Command> {
    file.rewind()?;
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::from(file.try_clone()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.args([
        "-v",
        "error",
        "-max_alloc",
        "67108864",
        "-protocol_whitelist",
        "fd,pipe",
        "-format_whitelist",
        "mov,matroska,webm,avi,mpeg,mpegts,ogg,flv",
        "-probesize",
        "1048576",
        "-analyzeduration",
        "1000000",
        "-threads",
        "1",
    ]);
    let parent = rustix::process::getpid();
    // Only async-signal-safe syscalls run between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            if rustix::process::getppid() != Some(parent) {
                return Err(rustix::io::Errno::INTR.into());
            }
            use rustix::process::{Resource, Rlimit, setrlimit};
            for (resource, limit) in [(Resource::As, 768 * 1024 * 1024), (Resource::Cpu, 4)] {
                setrlimit(
                    resource,
                    Rlimit {
                        current: Some(limit),
                        maximum: Some(limit),
                    },
                )?;
            }
            Ok(())
        });
    }
    Ok(cmd)
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn nonblocking(fd: &impl AsFd) -> Result<()> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    fcntl_setfl(fd, fcntl_getfl(fd)? | OFlags::NONBLOCK)?;
    Ok(())
}
fn drain(reader: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<()> {
    let mut buffer = [0; 8192];
    // Return to cancellation/deadline checks even for a continuously writing child.
    for _ in 0..32 {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                ensure!(
                    bytes.len() + n <= limit,
                    "Слишком большой ответ декодера видео"
                );
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn run(
    mut command: Command,
    cancel: &Cancellation,
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>> {
    ensure!(!cancel.is_cancelled(), "Предпросмотр отменён");
    let mut child = Running(command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("Для предпросмотра видео установите FFmpeg (ffmpeg и ffprobe)")
        } else {
            e.into()
        }
    })?);
    let mut stdout = child.0.stdout.take().unwrap();
    let mut stderr = child.0.stderr.take().unwrap();
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let started = Instant::now();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        ensure!(!cancel.is_cancelled(), "Предпросмотр отменён");
        ensure!(
            started.elapsed() < timeout,
            "Превышено время подготовки видео"
        );
        drain(&mut stdout, &mut output, limit)?;
        drain(&mut stderr, &mut errors, 64 * 1024)?;
        if let Some(status) = child.0.try_wait()? {
            // The bounded output may still be in the pipe after process exit.
            loop {
                let before = output.len();
                drain(&mut stdout, &mut output, limit)?;
                if before == output.len() {
                    break;
                }
            }
            drain(&mut stderr, &mut errors, 64 * 1024)?;
            if !status.success() {
                bail!(
                    "Не удалось прочитать видео: {}",
                    crate::fs::clean(&String::from_utf8_lossy(&errors))
                );
            }
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn read(file: &mut File, graphics: bool, cancel: &Cancellation) -> Result<Decoded> {
    let mut probe = command("ffprobe", file)?;
    probe.args([
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height,duration:format=duration",
        "-of",
        "json",
        "-i",
        "fd:",
    ]);
    let bytes = run(probe, cancel, 64 * 1024, Duration::from_secs(3))?;
    let info: serde_json::Value = serde_json::from_slice(&bytes)?;
    let stream = info["streams"]
        .as_array()
        .and_then(|s| s.first())
        .context("Видеодорожка не найдена")?;
    let width = stream["width"].as_u64().unwrap_or(0);
    let height = stream["height"].as_u64().unwrap_or(0);
    ensure!(
        width > 0
            && height > 0
            && width <= 16384
            && height <= 16384
            && width * height <= 16 * 1024 * 1024,
        "Видео больше 16 мегапикселей"
    );
    let duration = info["format"]["duration"]
        .as_str()
        .or_else(|| stream["duration"].as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0);
    let position = (duration.unwrap_or(0.0) * 0.1).min(5.0);
    let image = if graphics {
        let mut ffmpeg = command("ffmpeg", file)?;
        ffmpeg.args([
            "-nostdin",
            "-max_pixels",
            "16777216",
            "-ss",
            &format!("{position:.3}"),
            "-i",
            "fd:",
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-frames:v",
            "1",
            "-filter_threads",
            "1",
            "-vf",
            "scale=w='min(768,max(1,768*dar))':h='min(768,max(1,768/dar))',setsar=1",
            "-c:v",
            "png",
            "-threads",
            "1",
            "-f",
            "image2pipe",
            "pipe:1",
        ]);
        let bytes = run(ffmpeg, cancel, 4 * 1024 * 1024, Duration::from_secs(4))?;
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(768);
        limits.max_image_height = Some(768);
        limits.max_alloc = Some(8 * 1024 * 1024);
        reader.limits(limits);
        let rgba = reader
            .decode()
            .context("Не удалось получить кадр видео")?
            .into_rgba8();
        Some(Arc::new(ImagePreview::new(
            rgba.width(),
            rgba.height(),
            rgba.into_raw(),
        )?))
    } else {
        None
    };
    Ok(Decoded {
        details: format!(
            "Видео · {width} × {height}\nДлительность {}",
            duration.map_or_else(|| "неизвестна".into(), |d| format!("{d:.1} с"))
        ),
        image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoder_dies_when_its_supervising_thread_exits() {
        use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("decoder");
        std::fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let child = std::thread::spawn(move || {
            let mut input = tempfile::tempfile().unwrap();
            command(program.to_str().unwrap(), &mut input)
                .unwrap()
                .spawn()
                .unwrap()
        })
        .join()
        .unwrap();
        let mut child = Running(child);
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert_eq!(status.signal(), Some(9));
                break;
            }
            assert!(Instant::now() < end, "Decoder outlived its supervisor");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn missing_decoder_has_an_actionable_error() {
        let cmd = Command::new("basalgrim-test-nonexistent-ffmpeg");
        assert!(
            run(cmd, &Cancellation::default(), 1000, Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("установите FFmpeg")
        );
    }
    #[test]
    fn cancelled_timed_out_and_oversized_children_are_reaped() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec sleep 30"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        assert!(
            run(
                cmd,
                &Cancellation::default(),
                1000,
                Duration::from_millis(50)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec yes x"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert!(
            run(cmd, &Cancellation::default(), 1000, Duration::from_secs(1))
                .unwrap_err()
                .to_string()
                .contains("большой")
        );
        let cancel = Cancellation::default();
        let copy = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            copy.cancel();
        });
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec sleep 30"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert!(
            run(cmd, &cancel, 1000, Duration::from_secs(5))
                .unwrap_err()
                .to_string()
                .contains("отменён")
        );
        thread.join().unwrap();
    }
}

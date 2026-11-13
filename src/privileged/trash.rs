//! Keep gio's caller identity and XDG trash while granting only filesystem access.
use super::*;
use std::{
    io::Read,
    os::unix::fs::MetadataExt,
    process::{Command, Stdio},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserTrash {
    pub home: Vec<u8>,
    pub data: Vec<u8>,
}
impl UserTrash {
    pub fn capture() -> Result<Self> {
        Ok(Self {
            home: bytes(&dirs::home_dir().context("Не определена домашняя папка")?),
            data: bytes(&dirs::data_dir().context("Не определена пользовательская корзина")?),
        })
    }
    pub(super) fn parents(&self) -> Result<Vec<PathBuf>> {
        let mut paths = vec![path(&self.home)];
        let data = path(&self.data);
        for p in [
            data.clone(),
            data.join("Trash"),
            data.join("Trash/files"),
            data.join("Trash/info"),
        ] {
            valid_path(&p)?;
            let mut existing = p.as_path();
            loop {
                match fs::symlink_metadata(existing) {
                    Ok(_) => {
                        paths.push(existing.into());
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        existing = existing.parent().context("Нет родителя корзины")?
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(paths)
    }
    pub(super) fn validate(&self, uid: u32) -> Result<u32> {
        ensure!(
            self.home.len() + self.data.len() <= 128 * 1024,
            "Слишком длинный путь корзины"
        );
        valid_path(&path(&self.home))?;
        valid_path(&path(&self.data))?;
        let home = fs::symlink_metadata(path(&self.home))?;
        ensure!(
            home.is_dir() && home.uid() == uid && home.mode() & 0o022 == 0,
            "Домашняя папка не принадлежит инициатору операции или доступна для чужой записи"
        );
        for root in [
            path(&self.data).join("Trash"),
            path(&self.data).join("Trash/files"),
            path(&self.data).join("Trash/info"),
        ] {
            match fs::symlink_metadata(&root) {
                Ok(m) => ensure!(
                    m.is_dir() && m.uid() == uid && m.mode() & 0o022 == 0,
                    "Небезопасный владелец или ссылка пользовательской корзины: {}",
                    root.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(home.gid())
    }
    fn command(&self, uid: u32) -> Result<Command> {
        let gid = self.validate(uid)?;
        let elevated = rustix::process::geteuid().is_root();
        let mut command = if elevated {
            for exe in ["/usr/bin/setpriv", "/usr/bin/gio"] {
                let m = fs::symlink_metadata(exe)
                    .with_context(|| format!("Для корзины нужен {exe}"))?;
                ensure!(
                    m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0,
                    "Небезопасный исполнитель корзины: {exe}"
                );
            }
            let mut command = Command::new("/usr/bin/setpriv");
            command.args([
                "--reuid",
                &uid.to_string(),
                "--regid",
                &gid.to_string(),
                "--clear-groups",
                "--bounding-set=-all,+dac_override,+fowner",
                "--inh-caps=-all,+dac_override,+fowner",
                "--ambient-caps=-all,+dac_override,+fowner",
                "--no-new-privs",
                "/usr/bin/gio",
            ]);
            command
        } else {
            // Isolated protocol tests use the actual gio with their own identity.
            ensure!(
                rustix::process::geteuid().as_raw() == uid,
                "Не совпадает инициатор корзины"
            );
            Command::new("/usr/bin/gio")
        };
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("HOME", path(&self.home))
            .env("XDG_DATA_HOME", path(&self.data))
            .env("XDG_CONFIG_HOME", path(&self.home).join(".config"))
            .env("GIO_USE_VFS", "local")
            .env("GIO_USE_VOLUME_MONITOR", "unix")
            .current_dir("/")
            .args(["trash", "--"]);
        Ok(command)
    }
    pub(super) fn send(&self, uid: u32, source: &Path) -> Result<()> {
        let mut child = self
            .command(uid)?
            .arg(source)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Не удалось запустить системную корзину")?;
        // No killing after gio starts: its atomic trash rename may already have
        // succeeded. Drain output with a fixed retained limit and always reap.
        let mut stderr = child.stderr.take().unwrap();
        let mut saved = Vec::new();
        let mut buf = [0u8; 1024];
        let read = (|| -> std::io::Result<()> {
            loop {
                let n = stderr.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                let keep = n.min(8192usize.saturating_sub(saved.len()));
                saved.extend_from_slice(&buf[..keep]);
            }
        })();
        let status = child.wait()?;
        ensure!(
            status.success(),
            "Корзина: {}",
            crate::fs::clean(String::from_utf8_lossy(&saved).trim())
        );
        // A diagnostic read failure cannot undo a successful atomic trash action.
        let _ = read;
        Ok(())
    }
}

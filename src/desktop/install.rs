use super::*;
use gio::prelude::*;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};
const MARKER: &str = "# Basalgrim-managed v1\n";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallAction {
    Install,
    Default,
    Restore,
}
#[derive(Clone)]
pub struct Config {
    pub data: PathBuf,
    pub state: PathBuf,
    pub executable: PathBuf,
}
impl Config {
    pub fn current() -> Result<Self> {
        let mut executable = std::env::current_exe()?;
        if executable.file_name().is_some_and(|n| n == "basalgrim-tui") {
            executable.set_file_name("basalgrim");
        }
        Ok(Self {
            data: dirs::data_dir().context("Нет XDG_DATA_HOME")?,
            state: dirs::state_dir()
                .context("Нет XDG_STATE_HOME")?
                .join("molten-zharr/basalgrim/desktop"),
            executable,
        })
    }
}
pub fn is_default() -> bool {
    gio::AppInfo::default_for_type("inode/directory", false)
        .and_then(|a| a.id())
        .is_some_and(|id| id == DESKTOP_ID)
}
fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(meta) => ensure!(
            meta.is_file() && !meta.is_symlink() && meta.len() <= 65536,
            "Небезопасный или слишком большой файл регистрации: {}",
            path.display()
        ),
        Err(e) => return Err(e.into()),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65536, "Файл регистрации слишком большой");
    Ok(Some(bytes))
}
fn write(path: &Path, content: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("Нет родительской папки")?)?;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temp.write_all(content)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
fn owned(path: &Path) -> Result<()> {
    if let Some(bytes) = read(path)? {
        ensure!(
            bytes.starts_with(MARKER.as_bytes()),
            "Чужой файл сохранён без изменений: {}",
            path.display()
        );
    }
    Ok(())
}
fn quote(path: &Path, desktop: bool) -> Result<String> {
    let s = path
        .to_str()
        .context("Путь к программе должен быть UTF-8 для desktop Exec")?;
    ensure!(
        !s.chars().any(char::is_control),
        "Управляющий символ в пути программы"
    );
    let mut q = String::from("\"");
    for c in s.chars() {
        match c {
            '%' if desktop => q.push_str("%%"),
            '\\' => q.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                q.push_str("\\\\");
                q.push(c);
            }
            _ => q.push(c),
        }
    }
    q.push('"');
    Ok(q)
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Previous {
    desktop: Option<String>,
}
pub fn integrate(config: &Config, action: InstallAction) -> Result<String> {
    ensure!(
        config.data.is_absolute() && config.state.is_absolute() && config.executable.is_absolute(),
        "Регистрация требует абсолютных путей"
    );
    fs::create_dir_all(&config.state)?;
    use std::os::unix::fs::OpenOptionsExt;
    let lock = fs::OpenOptions::new()
        .create(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .truncate(false)
        .write(true)
        .open(config.state.join("lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
    let desktop = config.data.join("applications").join(DESKTOP_ID);
    let service = config
        .data
        .join("dbus-1/services/org.freedesktop.FileManager1.service");
    let previous = config.state.join("previous.json");
    if action == InstallAction::Restore {
        ensure!(
            is_default(),
            "Менеджер по умолчанию уже изменён; его настройка сохранена"
        );
        let bytes = read(&previous)?.context("Нет сохранённого предыдущего менеджера")?;
        let saved: Previous = serde_json::from_slice(&bytes)?;
        let id = saved.desktop.context(
            "Ранее менеджер не был назначен. Выберите другой в настройках рабочего стола",
        )?;
        let app =
            gio::DesktopAppInfo::new(&id).context("Предыдущий менеджер больше не установлен")?;
        owned(&service)?;
        app.set_as_default_for_type("inode/directory")?;
        if service.exists() {
            fs::remove_file(&service)?;
        }
        fs::remove_file(previous)?;
        return Ok("Предыдущий файловый менеджер восстановлен".into());
    }
    ensure!(
        config.executable.is_file(),
        "Не найден GUI-файл basalgrim рядом с программой"
    );
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        fs::metadata(&config.executable)?.permissions().mode() & 0o111 != 0,
        "GUI-файл не исполняемый"
    );
    owned(&desktop)?;
    if action == InstallAction::Default {
        owned(&service)?;
    }
    let text = include_str!("../../assets/desktop/org.moltenzharr.Basalgrim.desktop").replace(
        "Exec=basalgrim ",
        &format!("Exec=/usr/bin/env -- {} ", quote(&config.executable, true)?),
    );
    let icon = config
        .data
        .join("icons/hicolor/scalable/apps/org.moltenzharr.Basalgrim.svg");
    if let Some(bytes) = read(&icon)? {
        ensure!(
            bytes == include_bytes!("../../assets/desktop/org.moltenzharr.Basalgrim.svg"),
            "Чужая иконка сохранена: {}",
            icon.display()
        );
    }
    write(
        &icon,
        include_bytes!("../../assets/desktop/org.moltenzharr.Basalgrim.svg"),
    )?;
    write(&desktop, text.as_bytes())?;
    if action == InstallAction::Default {
        if !is_default() {
            let saved = Previous {
                desktop: gio::AppInfo::default_for_type("inode/directory", false)
                    .and_then(|a| a.id())
                    .map(|id| id.to_string()),
            };
            write(&previous, &serde_json::to_vec(&saved)?)?;
        }
        let app =
            gio::DesktopAppInfo::from_filename(&desktop).context("Desktop-файл не принят GIO")?;
        let old_service = read(&service)?;
        write(
            &service,
            format!(
                "{MARKER}[D-BUS Service]\nName={FILE_MANAGER}\nExec={} --file-manager\n",
                quote(&config.executable, false)?
            )
            .as_bytes(),
        )?;
        if let Err(e) = app.set_as_default_for_type("inode/directory") {
            if let Some(bytes) = old_service {
                write(&service, &bytes)?;
            } else {
                fs::remove_file(&service)?;
            }
            return Err(e.into());
        }
        return Ok("Basalgrim назначен для открытия папок".into());
    }
    Ok("Ярлык Basalgrim установлен в меню приложений".into())
}

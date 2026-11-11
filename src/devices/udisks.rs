use super::*;
use std::{collections::BTreeMap, os::unix::ffi::OsStringExt};
use zbus::{
    blocking::{Connection, connection::Builder},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const FS: &str = "org.freedesktop.UDisks2.Filesystem";
type Properties = BTreeMap<String, OwnedValue>;
type Objects = std::collections::HashMap<OwnedObjectPath, BTreeMap<String, Properties>>;

/// Explicit address injection is for independent private-bus tests. Normal startup uses system().
#[derive(Default)]
pub struct UDisks {
    address: Option<String>,
    connection: Option<Connection>,
}
impl UDisks {
    pub fn at(address: String) -> Self {
        Self {
            address: Some(address),
            connection: None,
        }
    }
    fn connection(&mut self) -> Result<&Connection> {
        if self.connection.is_none() {
            let builder = if let Some(address) = &self.address {
                Builder::address(address.as_str())?
            } else {
                Builder::system()?
            };
            self.connection = Some(
                builder
                    .method_timeout(Duration::from_secs(30))
                    .build()
                    .context("Не удалось связаться с системной службой накопителей")?,
            );
        }
        Ok(self.connection.as_ref().unwrap())
    }
}
fn property<T>(properties: &Properties, key: &str) -> Result<T>
where
    T: TryFrom<OwnedValue>,
    T::Error: std::fmt::Display,
{
    let value = properties
        .get(key)
        .with_context(|| format!("Нет свойства {key}"))?
        .try_clone()?;
    T::try_from(value).map_err(|e| anyhow::anyhow!("Некорректное свойство {key}: {e}"))
}
fn string(p: &Properties, key: &str) -> Result<String> {
    let s: String = property(p, key)?;
    ensure!(s.len() <= 65536, "Слишком длинное свойство {key}");
    Ok(s)
}
fn path(mut raw: Vec<u8>) -> Result<PathBuf> {
    ensure!(raw.last() == Some(&0), "Некорректный путь накопителя");
    raw.pop();
    ensure!(
        raw.starts_with(b"/") && !raw.contains(&0) && raw.len() <= 65536,
        "Некорректный путь накопителя"
    );
    Ok(std::ffi::OsString::from_vec(raw).into())
}
fn decode(owner: String, objects: Objects) -> Result<Snapshot> {
    ensure!(objects.len() <= 4096, "Слишком много объектов накопителей");
    let mut snapshot = Snapshot {
        owner,
        ..Default::default()
    };
    let mut backing = BTreeMap::new();
    for (object, interfaces) in &objects {
        ensure!(
            object.as_str().starts_with(&format!("{ROOT}/")),
            "Посторонний объект накопителя"
        );
        if let Some(p) = interfaces.get(DRIVE) {
            let model = string(p, "Model")?;
            let vendor = string(p, "Vendor")?;
            let label = crate::fs::clean(format!("{vendor} {model}").trim());
            snapshot.drives.push(Drive {
                path: object.to_string(),
                identity: string(p, "Id")?,
                detected: property(p, "TimeDetected")?,
                media_detected: property(p, "TimeMediaDetected")?,
                label: if label.is_empty() {
                    "Накопитель".into()
                } else {
                    label
                },
                removable: property(p, "Removable")?,
                media_available: property(p, "MediaAvailable")?,
                power_off: property(p, "CanPowerOff")?,
                eject: property(p, "Ejectable")?,
                sibling: string(p, "SiblingId")?,
            });
        }
        if let Some(p) = interfaces.get(BLOCK) {
            let device = path(property(p, "Device")?)?;
            let drive: OwnedObjectPath = property(p, "Drive")?;
            let parent: OwnedObjectPath = property(p, "CryptoBackingDevice")?;
            backing.insert(object.to_string(), parent.to_string());
            let filesystem = interfaces.get(FS);
            let mounts = if let Some(fs) = filesystem {
                let raw: Vec<Vec<u8>> = property(fs, "MountPoints")?;
                ensure!(raw.len() <= 128, "Слишком много точек монтирования");
                raw.into_iter().map(path).collect::<Result<Vec<_>>>()?
            } else {
                vec![]
            };
            let label = string(p, "IdLabel")?;
            snapshot.volumes.push(Volume {
                path: object.to_string(),
                drive: drive.to_string(),
                number: property(p, "DeviceNumber")?,
                uuid: string(p, "IdUUID")?,
                label: if label.is_empty() {
                    crate::fs::clean(&device.to_string_lossy())
                } else {
                    crate::fs::clean(&label)
                },
                device,
                size: property(p, "Size")?,
                fs: string(p, "IdType")?,
                filesystem: filesystem.is_some(),
                system: property(p, "HintSystem")?,
                ignored: property(p, "HintIgnore")?,
                mounts,
            });
        }
    }
    // Decrypted mappings can have Drive="/". They still belong to the underlying physical disk.
    for _ in 0..64 {
        let mut changed = false;
        for i in 0..snapshot.volumes.len() {
            if snapshot.volumes[i].drive != "/" {
                continue;
            }
            if let Some(parent) = backing.get(&snapshot.volumes[i].path)
                && let Some(drive) = snapshot
                    .volumes
                    .iter()
                    .find(|v| &v.path == parent && v.drive != "/")
                    .map(|v| v.drive.clone())
            {
                snapshot.volumes[i].drive = drive;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    snapshot.drives.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.volumes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(snapshot)
}
fn friendly(error: zbus::Error) -> anyhow::Error {
    let text = error.to_string();
    let message = if text.contains("DeviceBusy") {
        "Накопитель занят. Завершите работу с его файлами и повторите."
    } else if text.contains("NotAuthorized") || text.contains("AccessDenied") {
        "Недостаточно прав для работы с накопителем."
    } else if text.contains("ServiceUnknown") || text.contains("NameHasNoOwner") {
        "Служба UDisks2 недоступна. Установите или запустите udisks2."
    } else {
        "Ошибка службы накопителей"
    };
    anyhow::anyhow!("{message} {text}")
}
impl UDisks {
    fn read(&mut self) -> Result<Snapshot> {
        let conn = self.connection()?;
        let owner: String = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(SERVICE,),
            )
            .map_err(friendly)?
            .body()
            .deserialize()?;
        let reply = conn
            .call_method(
                Some(owner.as_str()),
                ROOT,
                Some("org.freedesktop.DBus.ObjectManager"),
                "GetManagedObjects",
                &(),
            )
            .map_err(friendly)?;
        ensure!(
            reply.body().data().len() <= 8 * 1024 * 1024,
            "Каталог накопителей превышает 8 МиБ"
        );
        decode(owner, reply.body().deserialize()?)
    }
}
impl Backend for UDisks {
    fn snapshot(&mut self) -> Result<Snapshot> {
        let result = self.read();
        if result.is_err() {
            self.connection = None;
        }
        result
    }
    fn invoke(&mut self, owner: &str, object: &str, method: Method) -> Result<()> {
        let interface = if matches!(method, Method::Mount | Method::Unmount) {
            FS
        } else {
            DRIVE
        };
        let mut options = BTreeMap::from([("auth.no_user_interaction", Value::from(false))]);
        if method == Method::Unmount {
            options.insert("force", Value::from(false));
        }
        self.connection()?
            .call_method(
                Some(owner),
                object,
                Some(interface),
                method.name(),
                &(options,),
            )
            .map_err(friendly)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn udisks_paths_preserve_raw_bytes_and_reject_malformed_values() {
        assert_eq!(
            path(b"/media/disk-\xff\0".to_vec())
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"/media/disk-\xff"
        );
        for raw in [
            b"relative\0".as_slice(),
            b"/missing-nul",
            b"/embedded\0nul\0",
            b"\0",
            b"",
        ] {
            assert!(path(raw.to_vec()).is_err());
        }
        assert!(path([b"/".as_slice(), &vec![b'x'; 65536], b"\0"].concat()).is_err());
    }
}

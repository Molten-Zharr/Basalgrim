//! Linux file-manager clipboard formats. Paths are local Unix bytes, never shell text.
use anyhow::{Context, Result, bail, ensure};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const GNOME: &str = "x-special/gnome-copied-files";
pub const URI_LIST: &str = "text/uri-list";
pub const KDE_CUT: &str = "application/x-kde-cutselection";
pub const MARKER: &str = "application/x-basalgrim-clipboard";
pub const TEXT: &str = "text/plain;charset=utf-8";
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PATHS: usize = 4096;
pub type Formats = BTreeMap<String, Arc<[u8]>>;
pub const READ_FORMATS: [&str; 4] = [GNOME, URI_LIST, KDE_CUT, MARKER];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileList {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

pub fn uri(path: &Path) -> Result<String> {
    ensure!(
        path.is_absolute(),
        "Путь в файловом буфере должен быть абсолютным"
    );
    let bytes = path.as_os_str().as_bytes();
    ensure!(!bytes.contains(&0), "Нулевой байт в пути");
    let mut result = String::from("file://");
    for &b in bytes {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            result.push(b as char);
        } else {
            use std::fmt::Write;
            write!(result, "%{b:02X}")?;
        }
        ensure!(result.len() <= MAX_BYTES, "Путь слишком длинный для буфера");
    }
    Ok(result)
}
pub fn path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file:")
        .context("Поддерживаются только локальные file: URI")?;
    let encoded = if let Some(rest) = rest.strip_prefix("//") {
        let slash = rest.find('/').context("Нет абсолютного пути в file: URI")?;
        ensure!(
            rest[..slash].is_empty() || rest[..slash].eq_ignore_ascii_case("localhost"),
            "Сетевой адрес в файловом буфере не поддерживается"
        );
        &rest[slash..]
    } else {
        rest
    };
    ensure!(
        encoded.starts_with('/')
            && !encoded
                .bytes()
                .any(|b| b <= b' ' || b >= 127 || b"?#".contains(&b)),
        "Некорректный file: URI"
    );
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = bytes
                .get(i + 1..i + 3)
                .context("Неполное кодирование URI")?;
            let high = (pair[0] as char)
                .to_digit(16)
                .context("Некорректное кодирование URI")?;
            let low = (pair[1] as char)
                .to_digit(16)
                .context("Некорректное кодирование URI")?;
            decoded.push((high * 16 + low) as u8);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    ensure!(!decoded.contains(&0), "Нулевой байт в URI");
    Ok(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}
fn paths<'a>(lines: impl Iterator<Item = &'a str>, comments: bool) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for (count, line) in lines.enumerate() {
        ensure!(
            count < MAX_PATHS + 64,
            "Слишком много строк в файловом буфере"
        );
        if comments && (line.is_empty() || line.starts_with('#')) {
            continue;
        }
        ensure!(!line.is_empty(), "Пустой URI в файловом буфере");
        ensure!(count < MAX_PATHS, "В файловом буфере больше 4096 путей");
        let path = path(line)?;
        if seen.insert(path.clone()) {
            result.push(path);
        }
    }
    Ok(result)
}
fn text<'a>(formats: &'a Formats, mime: &str) -> Result<Option<&'a str>> {
    formats
        .get(mime)
        .map(|bytes| {
            ensure!(bytes.len() <= MAX_BYTES, "Файловый буфер превышает 4 MiB");
            Ok(std::str::from_utf8(bytes)?)
        })
        .transpose()
}
pub fn decode(formats: &Formats) -> Result<Option<FileList>> {
    let kde = text(formats, KDE_CUT)?
        .map(|v| match v {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => bail!("Неизвестный признак вырезания KDE"),
        })
        .transpose()?;
    let list = if let Some(value) = text(formats, GNOME)? {
        let mut lines = value.split('\n');
        let cut = match lines.next() {
            Some("cut") => true,
            Some("copy") => false,
            _ => bail!("Неизвестное действие файлового буфера"),
        };
        ensure!(
            kde.is_none_or(|v| v == cut),
            "Форматы буфера противоречат друг другу"
        );
        let paths = paths(lines, false)?;
        if let Some(uris) = text(formats, URI_LIST)? {
            ensure!(
                paths == self::paths(uris.lines(), true)?,
                "Список файлов изменился во время чтения буфера"
            );
        }
        FileList { paths, cut }
    } else if let Some(uris) = text(formats, URI_LIST)? {
        FileList {
            paths: paths(uris.lines(), true)?,
            cut: kde.unwrap_or(false),
        }
    } else {
        return Ok(None);
    };
    Ok((!list.paths.is_empty()).then_some(list))
}
pub fn marker(nonce: &str, generation: u64, kind: &str) -> Arc<[u8]> {
    format!("1\n{nonce}\n{generation}\n{kind}")
        .into_bytes()
        .into()
}
pub fn local_generation(formats: &Formats, nonce: &str) -> Option<u64> {
    let value = std::str::from_utf8(formats.get(MARKER)?).ok()?;
    let mut lines = value.lines();
    (lines.next()? == "1" && lines.next()? == nonce).then_some(())?;
    let generation = lines.next()?.parse().ok()?;
    matches!(lines.next()?, "files" | "text").then_some(())?;
    lines.next().is_none().then_some(generation)
}
pub fn encode(list: &FileList, nonce: &str, generation: u64) -> Result<Formats> {
    ensure!(
        list.paths.len() <= MAX_PATHS,
        "В файловом буфере больше 4096 путей"
    );
    if list.paths.is_empty() {
        let mut formats = encode_text(String::new(), nonce, generation, "files");
        formats.insert(
            GNOME.into(),
            Arc::from(if list.cut {
                b"cut".as_slice()
            } else {
                b"copy".as_slice()
            }),
        );
        formats.insert(URI_LIST.into(), Arc::from([]));
        formats.insert(
            KDE_CUT.into(),
            Arc::from(if list.cut {
                b"1".as_slice()
            } else {
                b"0".as_slice()
            }),
        );
        return Ok(formats);
    }
    let mut encoded = Vec::new();
    let mut budget = 0;
    for p in &list.paths {
        let value = uri(p)?;
        budget += value.len() + 2;
        ensure!(budget + 6 <= MAX_BYTES, "Файловый буфер превышает 4 MiB");
        encoded.push(value);
    }
    let mut formats = Formats::new();
    formats.insert(
        GNOME.into(),
        format!(
            "{}\n{}",
            if list.cut { "cut" } else { "copy" },
            encoded.join("\n")
        )
        .into_bytes()
        .into(),
    );
    formats.insert(
        URI_LIST.into(),
        format!("{}\r\n", encoded.join("\r\n")).into_bytes().into(),
    );
    formats.insert(
        KDE_CUT.into(),
        Arc::from(if list.cut { b"1" } else { b"0" }.as_slice()),
    );
    let plain = list
        .paths
        .iter()
        .zip(&encoded)
        .map(|(p, u)| p.to_str().unwrap_or(u))
        .collect::<Vec<_>>()
        .join("\n");
    for (mime, data) in encode_text(plain, nonce, generation, "files") {
        formats.insert(mime, data);
    }
    Ok(formats)
}
pub fn encode_text(text: String, nonce: &str, generation: u64, kind: &str) -> Formats {
    let bytes: Arc<[u8]> = text.into_bytes().into();
    [TEXT, "text/plain", "UTF8_STRING"]
        .into_iter()
        .map(|m| (m.into(), bytes.clone()))
        .chain([(MARKER.into(), marker(nonce, generation, kind))])
        .collect()
}

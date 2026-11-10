use super::*;
use quick_xml::{Reader, events::Event};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
};

fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let file: File = match OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(file.metadata()?.is_file(), "Ожидался обычный файл");
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 8 * 1024 * 1024, "Файл превышает 8 МиБ");
    Ok(Some(bytes))
}
pub(super) fn bookmarks(path: &Path) -> Result<Vec<Item>> {
    let Some(bytes) = read(path)? else {
        return Ok(vec![]);
    };
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, line) in std::str::from_utf8(&bytes)?.lines().enumerate() {
        ensure!(i < MAX_ITEMS, "Больше {MAX_ITEMS} GTK-закладок");
        if line.is_empty() {
            continue;
        }
        let (uri, label) = line.split_once(' ').unwrap_or((line, ""));
        let Ok(path) = crate::clipboard::files::path(uri) else {
            continue;
        };
        valid_path(&path)?;
        let mut item = Item::new(path);
        if !label.trim().is_empty() {
            valid_label(label)?;
            item.label = label.into();
        }
        item.desktop = true;
        if seen.insert(item.path.clone()) {
            result.push(item);
        }
    }
    Ok(result)
}
pub(super) fn recent(path: &Path) -> Result<Vec<Item>> {
    let Some(bytes) = read(path)? else {
        return Ok(vec![]);
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().expand_empty_elements = true;
    let mut result = Vec::new();
    let mut pending = None;
    let mut private = false;
    let mut depth = 0;
    let mut count = 0;
    let mut root = false;
    loop {
        match reader.read_event()? {
            Event::DocType(_) => anyhow::bail!("DTD в списке недавних файлов не поддерживается"),
            Event::Start(e) => {
                depth += 1;
                ensure!(depth <= 32, "Слишком глубокий XML");
                if depth == 1 {
                    ensure!(!root && e.local_name().as_ref() == b"xbel", "Ожидался XBEL");
                    root = true;
                }
                match e.local_name().as_ref() {
                    b"bookmark" => {
                        ensure!(depth == 2, "Некорректная вложенность bookmark");
                        count += 1;
                        ensure!(count <= MAX_ITEMS, "Больше {MAX_ITEMS} недавних записей");
                        let mut uri = String::new();
                        let mut visited = 0;
                        for a in e.attributes() {
                            let a = a?;
                            let value = a.decoded_and_normalized_value(
                                quick_xml::XmlVersion::Implicit1_0,
                                reader.decoder(),
                            )?;
                            if a.key.as_ref() == b"href" {
                                uri = value.into_owned();
                            } else if matches!(a.key.as_ref(), b"visited" | b"modified" | b"added")
                                && let Ok(date) = chrono::DateTime::parse_from_rfc3339(&value)
                            {
                                visited = visited.max(date.timestamp());
                            }
                        }
                        pending = crate::clipboard::files::path(&uri).ok().map(|p| {
                            let mut i = Item::new(p);
                            i.visited = visited;
                            i.desktop = true;
                            i
                        });
                        private = false;
                    }
                    b"private" => private = true,
                    b"title" if pending.is_some() => {
                        let text = reader.read_text(e.name())?;
                        let text = text.decode()?;
                        let text = quick_xml::escape::unescape(&text)?;
                        if !text.trim().is_empty() {
                            valid_label(&text)?;
                            pending.as_mut().unwrap().label = text.into_owned();
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            Event::Empty(e) if e.local_name().as_ref() == b"private" => private = true,
            Event::End(e) => {
                if e.local_name().as_ref() == b"bookmark"
                    && let Some(item) = pending.take().filter(|_| !private)
                {
                    valid_path(&item.path)?;
                    result.push(item);
                }
                depth -= 1;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(root && depth == 0, "Незавершённый XBEL");
    Ok(result)
}

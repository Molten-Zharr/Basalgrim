use super::*;
use std::collections::BTreeSet;

fn u16_at(bytes: &[u8], i: usize) -> u64 {
    u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()) as u64
}
fn u32_at(bytes: &[u8], i: usize) -> u64 {
    u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as u64
}
fn u64_at(bytes: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap())
}

/// Check central-directory allocation sizes before zip allocates its IndexMap.
/// Pin offset zero so self-extracting/prefixed archives cannot select a different directory.
pub(super) fn zip(file: &mut File, cancel: &Cancellation) -> Result<()> {
    let len = file.metadata()?.len();
    let tail_len = len.min(65557);
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len as usize];
    file.read_exact(&mut tail)?;
    let end = tail
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(i, w)| {
            (w == b"PK\x05\x06"
                && i + 22 <= tail.len()
                && i + 22 + u16_at(&tail, i + 20) as usize == tail.len())
            .then_some(i)
        })
        .context("Нет корректного конца ZIP")?;
    ensure!(
        u16_at(&tail, end + 4) == 0 && u16_at(&tail, end + 6) == 0,
        "Многотомные ZIP не поддерживаются"
    );
    let mut count = u16_at(&tail, end + 10);
    let mut size = u32_at(&tail, end + 12);
    let mut offset = u32_at(&tail, end + 16);
    let eocd = len - tail_len + end as u64;
    if count == u16::MAX as u64 || size == u32::MAX as u64 || offset == u32::MAX as u64 {
        ensure!(eocd >= 20, "Обрезанный ZIP64");
        file.seek(SeekFrom::Start(eocd - 20))?;
        let mut locator = [0; 20];
        file.read_exact(&mut locator)?;
        ensure!(
            &locator[..4] == b"PK\x06\x07" && u32_at(&locator, 4) == 0 && u32_at(&locator, 16) == 1,
            "Неподдерживаемый ZIP64"
        );
        let at = u64_at(&locator, 8);
        ensure!(
            at.checked_add(56).is_some_and(|end| end <= eocd - 20),
            "Неверный адрес ZIP64"
        );
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0; 56];
        file.read_exact(&mut header)?;
        ensure!(
            &header[..4] == b"PK\x06\x06" && u32_at(&header, 16) == 0 && u32_at(&header, 20) == 0,
            "Неверный каталог ZIP64"
        );
        count = u64_at(&header, 32);
        size = u64_at(&header, 40);
        offset = u64_at(&header, 48);
    }
    ensure!(
        count <= MAX_MEMBERS as u64 && size <= MAX_NAMES as u64,
        "Слишком большой каталог ZIP"
    );
    let stop = offset
        .checked_add(size)
        .context("Переполнение каталога ZIP")?;
    ensure!(stop <= eocd, "Каталог ZIP за пределами файла");
    file.seek(SeekFrom::Start(offset))?;
    let mut names = BTreeSet::new();
    for _ in 0..count {
        check(cancel)?;
        let at = file.stream_position()?;
        ensure!(at + 46 <= stop, "Обрезанный каталог ZIP");
        let mut header = [0; 46];
        file.read_exact(&mut header)?;
        ensure!(
            &header[..4] == b"PK\x01\x02",
            "Неверная запись каталога ZIP"
        );
        let n = u16_at(&header, 28);
        let rest = u16_at(&header, 30) + u16_at(&header, 32);
        ensure!(at + 46 + n + rest <= stop, "Обрезанная запись ZIP");
        let mut name = vec![0; n as usize];
        file.read_exact(&mut name)?;
        ensure!(names.insert(name), "Повторяющееся имя в каталоге ZIP");
        file.seek(SeekFrom::Current(rest as i64))?;
    }
    ensure!(file.stream_position()? == stop, "Неоднозначный каталог ZIP");
    Ok(())
}

/// Raw iteration lets us bound GNU/PAX extension bodies before tar allocates them.
/// Reject variants whose sparse/size overrides would give the two passes different framing.
pub(super) fn tar(reader: impl Read) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let mut metadata = 0u64;
    for (i, entry) in archive.entries()?.raw(true).enumerate() {
        ensure!(i < MAX_MEMBERS * 3, "Слишком много заголовков TAR");
        let mut entry = entry?;
        let ty = entry.header().entry_type();
        ensure!(
            !ty.is_gnu_sparse(),
            "Разреженные записи TAR пока не поддерживаются"
        );
        if ty.is_gnu_longname()
            || ty.is_gnu_longlink()
            || ty.is_pax_local_extensions()
            || ty.is_pax_global_extensions()
        {
            ensure!(
                entry.size() <= 64 * 1024,
                "Заголовок GNU/PAX превышает 64 КиБ"
            );
            metadata += entry.size();
            ensure!(
                metadata <= MAX_NAMES as u64,
                "Метаданные TAR превышают 128 МиБ"
            );
            if (ty.is_pax_local_extensions() || ty.is_pax_global_extensions())
                && let Some(fields) = entry.pax_extensions()?
            {
                for field in fields {
                    let field = field?;
                    let key = field.key_bytes();
                    ensure!(
                        key != b"size" && !key.starts_with(b"GNU.sparse"),
                        "Переопределение размера PAX пока не поддерживается"
                    );
                }
            }
        }
    }
    Ok(())
}

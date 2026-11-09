//! Bounded, read-only rename previews. Execution accepts exactly the checked plan.
use crate::{fs, journal::Identity, operations::Cancellation};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{collections::BTreeMap, path::PathBuf};

pub const MAX_ITEMS: usize = 10_000;
pub const DEFAULT_PATTERN: &str = "{name}";
pub const NUMBERED_PATTERN: &str = "{name}_{n:03}";

/// Plain user text is always literal. Only the explicit advanced mode parses tokens.
pub fn simple_pattern(text: &str, numbered: bool) -> String {
    let mut pattern = if text.is_empty() {
        "{name}".to_owned()
    } else {
        text.replace('{', "{{").replace('}', "}}")
    };
    if numbered {
        pattern.push_str("_{n:03}");
    }
    pattern.push_str("{ext}");
    pattern
}

/// Start advanced editing without a redundant extension token. An explicitly
/// dotted, unnumbered common name still needs to preserve its literal suffix.
pub fn advanced_pattern(text: &str, numbered: bool) -> String {
    let mut pattern = simple_pattern(text, numbered);
    if numbered || !text.rfind('.').is_some_and(|i| i > 0) {
        pattern.truncate(pattern.len() - "{ext}".len());
    }
    pattern
}

#[derive(Clone, Debug)]
pub struct Item {
    pub(crate) source: PathBuf,
    pub(crate) destination: PathBuf,
    identity: Identity,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub(crate) parent: PathBuf,
    parent_identity: Identity,
    pub(crate) items: Vec<Item>,
}
impl Plan {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub(crate) fn validate_item(&self, item: &Item, cancel: &Cancellation) -> Result<()> {
        fs::operation_checkpoint(cancel)?;
        let parent = Identity::read(&self.parent)?.context("Папка исчезла")?;
        ensure!(
            parent.directory()
                && parent.device == self.parent_identity.device
                && parent.inode == self.parent_identity.inode,
            "Папка изменилась; обновите предпросмотр"
        );
        ensure!(
            Identity::read(&item.source)?.as_ref() == Some(&item.identity),
            "Исходник изменился; обновите предпросмотр: {}",
            fs::clean(&item.source.to_string_lossy())
        );
        ensure!(
            Identity::read(&item.destination)?.is_none(),
            "Имя занято; обновите предпросмотр: {}",
            fs::clean(&item.destination.to_string_lossy())
        );
        fs::operation_checkpoint(cancel)
    }
    pub(crate) fn validate(&self, cancel: &Cancellation) -> Result<()> {
        for item in &self.items {
            self.validate_item(item, cancel)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct Row {
    pub original: String,
    pub proposed: String,
    pub error: Option<String>,
    pub changed: bool,
}
#[derive(Debug)]
pub struct Preview {
    pub rows: Vec<Row>,
    pub changed: usize,
    pub conflicts: usize,
    plan: Option<Plan>,
}
impl Preview {
    pub fn plan(&self) -> Option<Plan> {
        self.plan.clone()
    }
    pub fn ready(&self) -> bool {
        self.plan.is_some()
    }
}

enum Part {
    Literal(String),
    Name,
    Full,
    Extension,
    SetExtension(String),
    Number(usize),
}
fn unknown_variable(text: &str, start: usize, end: usize, token: &str) -> anyhow::Error {
    let position = crate::text_edit::boundaries(&text[..start]).len();
    let mut message = format!(
        "Неизвестная переменная: {{{}}}\nСимвол {position}. В скобках — только переменные.",
        fs::clean(token)
    );
    let extension = token.strip_prefix('.').unwrap_or(token);
    if matches!(
        extension.to_ascii_lowercase().as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "svg"
            | "bmp"
            | "tif"
            | "tiff"
            | "txt"
            | "md"
            | "pdf"
            | "doc"
            | "docx"
            | "csv"
            | "zip"
            | "mp3"
            | "mp4"
    ) {
        let corrected = |replacement: &str| {
            let mut value = text.to_owned();
            value.replace_range(start..end, replacement);
            fs::clean(&value)
        };
        message.push_str(&format!(
            "\nЗадать расширение .{extension}:\n{}\nСохранить расширения — убрать эту вставку:\n{}",
            corrected(&format!("{{ext:{extension}}}")),
            corrected("")
        ));
    } else {
        message.push_str("\n{name} — прежнее имя без расширения\n{n:03} — номер: 001, 002, 003\n{ext:jpg} — задать расширение .jpg\n{ext:} — убрать расширение\nБез ext: расширение сохранится само.\n{full} — прежнее имя целиком");
    }
    anyhow!(message)
}
fn pattern(text: &str) -> Result<Vec<Part>> {
    ensure!(
        text.len() <= 4096,
        "Слишком длинный шаблон\nСократите его до 4096 байт."
    );
    let mut rest = text;
    let mut parts = Vec::new();
    while !rest.is_empty() {
        if let Some(next) = rest.strip_prefix("{{") {
            parts.push(Part::Literal("{".into()));
            rest = next;
        } else if let Some(next) = rest.strip_prefix("}}") {
            parts.push(Part::Literal("}".into()));
            rest = next;
        } else if let Some(next) = rest.strip_prefix('{') {
            let start = text.len() - rest.len();
            let position = crate::text_edit::boundaries(&text[..start]).len();
            let end = next.find('}').with_context(|| format!("Не закрыта скобка {{\nСимвол {position}. Добавьте }} после переменной.\nПример номера: {{n:03}}"))?;
            parts.push(match &next[..end] {
                "name" => Part::Name,
                "full" => Part::Full,
                "ext" => Part::Extension,
                token if token.starts_with("ext:") => {
                    let extension = token[4..].strip_prefix('.').unwrap_or(&token[4..]);
                    ensure!(
                        start + end + 2 == text.len(),
                        "Расширение должно быть в конце: {{{}}}\nСимвол {position}. Перенесите эту вставку в конец.\nПример: Photo_{{n:03}}{{ext:jpg}}",
                        fs::clean(token)
                    );
                    ensure!(
                        token == "ext:" || (!extension.is_empty() && fs::validate_name(extension).is_ok()),
                        "Неверное расширение: {{{}}}\nСимвол {position}. Укажите расширение без пути.\n{{ext:jpg}} — .jpg; {{ext:}} — убрать расширение.",
                        fs::clean(token)
                    );
                    Part::SetExtension(extension.to_owned())
                }
                "n" => Part::Number(1),
                token if token.starts_with("n:") => {
                    let width = token[2..]
                        .parse::<usize>()
                        .map_err(|_| anyhow!("Неверный номер: {{{}}}\nСимвол {position}. После n: нужна ширина от 1 до 8.\n{{n}} — 1, 2, 3; {{n:03}} — 001, 002, 003.", fs::clean(token)))?;
                    ensure!((1..=8).contains(&width), "Неверная ширина: {{{}}}\nСимвол {position}. Допустима ширина от 1 до 8.\nНапример, {{n:03}} даёт 001, 002, 003.", fs::clean(token));
                    Part::Number(width)
                }
                token => return Err(unknown_variable(text, start, start + end + 2, token)),
            });
            rest = &next[end + 1..];
        } else if rest.starts_with('}') {
            let position = crate::text_edit::boundaries(&text[..text.len() - rest.len()]).len();
            bail!(
                "Лишняя скобка }}\nСимвол {position}. Уберите её.\nЧтобы оставить }} в имени, напишите }}}}."
            );
        } else {
            let end = rest.find(['{', '}']).unwrap_or(rest.len());
            parts.push(Part::Literal(rest[..end].into()));
            rest = &rest[end..];
        }
    }
    Ok(parts)
}
fn has_literal_extension(result: &str, literal_tail: usize) -> bool {
    let tail_start = result.len() - literal_tail;
    result[tail_start..]
        .rfind('.')
        .is_some_and(|i| tail_start + i > 0)
}

fn name(parts: &[Part], original: &str, directory: bool, number: usize) -> Result<String> {
    let split = (!directory)
        .then(|| original.rfind('.'))
        .flatten()
        .filter(|&i| i > 0 && i + 1 < original.len())
        .unwrap_or(original.len());
    let mut result = String::new();
    let mut explicit_extension = false;
    let mut literal_tail = 0;
    for part in parts {
        match part {
            Part::Literal(s) => result.push_str(s),
            Part::Name => result.push_str(&original[..split]),
            Part::Full => {
                result.push_str(original);
                explicit_extension |= split < original.len();
            }
            Part::Extension => {
                result.push_str(&original[split..]);
                explicit_extension |= split < original.len();
            }
            Part::SetExtension(extension) => {
                // {full} and legacy literal suffixes already carry an extension;
                // a new explicit choice replaces it. Dots inside a {name} stem
                // and a directory's full name remain part of that name.
                if (explicit_extension || has_literal_extension(&result, literal_tail))
                    && let Some(dot) = result.rfind('.').filter(|&i| i > 0)
                {
                    result.truncate(dot);
                }
                if !extension.is_empty() {
                    result.push('.');
                    result.push_str(extension);
                }
                explicit_extension = true;
            }
            Part::Number(width) => result.push_str(&format!("{number:0width$}")),
        }
        if let Part::Literal(s) = part {
            literal_tail += s.len();
        } else {
            literal_tail = 0;
        }
        ensure!(result.len() <= 255, "Имя длиннее 255 байт");
    }
    // Preserve existing literal full-name patterns such as Photo_{n:03}.jpeg.
    // Dots produced by {name} belong to the stem (e.g. archive.tar), so they
    // must not suppress the automatic original suffix. Escaped braces remain
    // part of the consecutive literal tail.
    let literal_extension = has_literal_extension(&result, literal_tail);
    fs::validate_name(&result)?;
    if !directory && !explicit_extension && !literal_extension {
        result.push_str(&original[split..]);
        ensure!(result.len() <= 255, "Имя длиннее 255 байт");
    }
    Ok(result)
}

pub fn preview(sources: &[PathBuf], text: &str, cancel: &Cancellation) -> Result<Preview> {
    ensure!(
        !sources.is_empty() && sources.len() <= MAX_ITEMS,
        "Выберите от 1 до {MAX_ITEMS} файлов"
    );
    ensure!(
        sources.iter().map(|p| p.as_os_str().len()).sum::<usize>() <= 8 * 1024 * 1024,
        "Слишком большой список путей"
    );
    let parts = pattern(text)?;
    let parent = sources[0]
        .parent()
        .context("Нельзя переименовать корень")?
        .to_path_buf();
    ensure!(
        parent.is_absolute() && sources.iter().all(|p| p.parent() == Some(parent.as_path())),
        "Выберите файлы одной папки"
    );
    fs::operation_checkpoint(cancel)?;
    let parent_identity = Identity::read(&parent)?.context("Папка исчезла")?;
    ensure!(
        parent_identity.directory(),
        "Родительская папка заменена ссылкой"
    );
    let mut rows = Vec::with_capacity(sources.len());
    let mut items = Vec::new();
    let mut destinations = BTreeMap::<PathBuf, Vec<usize>>::new();
    let mut originals = std::collections::BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        fs::operation_checkpoint(cancel)?;
        let mut row = Row {
            original: fs::clean(&source.file_name().unwrap_or_default().to_string_lossy()),
            proposed: String::new(),
            error: None,
            changed: false,
        };
        let checked = (|| -> Result<()> {
            ensure!(originals.insert(source), "Исходник выбран дважды");
            let original = source
                .file_name()
                .and_then(|n| n.to_str())
                .context("Имя не в UTF-8; автоматическое преобразование недоступно")?;
            let identity = Identity::read(source)?.context("Исходник исчез")?;
            row.proposed = name(&parts, original, identity.directory(), index + 1)?;
            row.changed = row.proposed != original;
            let destination = parent.join(&row.proposed);
            destinations
                .entry(destination.clone())
                .or_default()
                .push(index);
            if row.changed {
                ensure!(Identity::read(&destination)?.is_none(), "Имя уже занято");
                items.push(Item {
                    source: source.clone(),
                    destination,
                    identity,
                });
            }
            Ok(())
        })();
        if let Err(e) = checked {
            row.error = Some(format!("{e:#}"));
        }
        rows.push(row);
    }
    for indices in destinations.values().filter(|v| v.len() > 1) {
        for &index in indices {
            rows[index].error = Some("Повторяющееся имя в выбранной группе".into());
        }
    }
    let changed = rows.iter().filter(|r| r.changed).count();
    let conflicts = rows.iter().filter(|r| r.error.is_some()).count();
    let plan = (conflicts == 0 && !items.is_empty()).then_some(Plan {
        parent,
        parent_identity,
        items,
    });
    Ok(Preview {
        rows,
        changed,
        conflicts,
        plan,
    })
}

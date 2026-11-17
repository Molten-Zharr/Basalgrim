//! Visual groups are metadata, never synthetic filesystem entries.
use crate::{columns::Column, fs::Entry, view::ViewMode};
use chrono::{DateTime, Datelike, Local};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Range, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grouping {
    pub enabled: bool,
    pub field: Column,
    pub descending: bool,
}
impl Default for Grouping {
    fn default() -> Self {
        Self {
            enabled: false,
            field: Column::Created,
            descending: true,
        }
    }
}
impl Grouping {
    pub fn date(self) -> bool {
        matches!(
            self.field,
            Column::Created | Column::Modified | Column::Accessed
        )
    }
    pub fn order_label(self) -> &'static str {
        if self.date() {
            if self.descending {
                "Сначала новые"
            } else {
                "Сначала старые"
            }
        } else if self.field == Column::Size {
            if self.descending {
                "Сначала большие"
            } else {
                "Сначала маленькие"
            }
        } else if self.descending {
            "По убыванию"
        } else {
            "По возрастанию"
        }
    }
    fn key(self, e: &Entry) -> (Option<Key>, String) {
        let text = |s: String| (Some(Key::Text(s.to_lowercase())), s);
        if self.date() {
            let time = match self.field {
                Column::Created => e.details.created,
                Column::Accessed => e.details.accessed,
                _ => e.modified,
            };
            let Some(time) = time else {
                return (None, "Дата неизвестна".into());
            };
            let date: DateTime<Local> = time.into();
            const MONTHS: [&str; 12] = [
                "Январь",
                "Февраль",
                "Март",
                "Апрель",
                "Май",
                "Июнь",
                "Июль",
                "Август",
                "Сентябрь",
                "Октябрь",
                "Ноябрь",
                "Декабрь",
            ];
            return (
                Some(Key::Month(date.year(), date.month())),
                format!("{} {}", MONTHS[date.month0() as usize], date.year()),
            );
        }
        match self.field {
            Column::Name => {
                let c = e.name.chars().next().unwrap_or(' ');
                if c.is_ascii_digit() {
                    text("0–9".into())
                } else if c.is_alphabetic() {
                    text(c.to_uppercase().collect())
                } else {
                    text("Символы".into())
                }
            }
            Column::Size => {
                let (n, label) = if e.directory {
                    (0, "Папки")
                } else {
                    match e.size {
                        0 => (1, "Пустые файлы"),
                        1..=1_048_575 => (2, "Меньше 1 MiB"),
                        1_048_576..=16_777_215 => (3, "1–16 MiB"),
                        16_777_216..=134_217_727 => (4, "16–128 MiB"),
                        134_217_728..=1_073_741_823 => (5, "128 MiB–1 GiB"),
                        _ => (6, "1 GiB и больше"),
                    }
                };
                (Some(Key::Number(n)), label.into())
            }
            Column::Kind => text(self.field.text(e)),
            Column::Extension => {
                let ext = self.field.text(e);
                text(if e.directory {
                    "Папки".into()
                } else if ext.is_empty() {
                    "Без расширения".into()
                } else {
                    ext.to_lowercase()
                })
            }
            Column::Owner if e.details.owner.is_none() => (None, "Владелец неизвестен".into()),
            Column::Permissions => e
                .details
                .mode
                .map(|m| {
                    (
                        Some(Key::Number(u64::from(m & 0o7777))),
                        format!("{:04o}", m & 0o7777),
                    )
                })
                .unwrap_or((None, "Права неизвестны".into())),
            _ => text(self.field.text(e)),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Number(u64),
    Month(i32, u32),
    Text(String),
}
#[derive(Debug, Clone)]
pub struct Group {
    pub label: String,
    pub range: Range<usize>,
    pub count: usize,
}
/// Preserve the ordinary stable sort inside each bucket. A tree root travels
/// with its whole expanded subtree; children never detach from their parent.
pub fn project(
    entries: &[Entry],
    visible: &mut Vec<usize>,
    policy: Grouping,
    tree_root: Option<&Path>,
) -> Vec<Group> {
    if !policy.enabled {
        return vec![];
    }
    let mut buckets: BTreeMap<Option<Key>, (String, Vec<usize>, usize)> = BTreeMap::new();
    let mut current = None;
    for &index in visible.iter() {
        let e = &entries[index];
        let root = tree_root.is_none_or(|p| e.path.parent() == Some(p));
        if root || current.is_none() {
            current = Some(policy.key(e));
        }
        let (key, label) = current.as_ref().unwrap();
        let bucket = buckets
            .entry(key.clone())
            .or_insert_with(|| (label.clone(), vec![], 0));
        bucket.1.push(index);
        bucket.2 += usize::from(root);
    }
    let unknown = buckets.remove(&None);
    let mut ordered: Vec<_> = buckets.into_values().collect();
    if policy.descending {
        ordered.reverse();
    }
    ordered.extend(unknown);
    visible.clear();
    ordered
        .into_iter()
        .map(|(label, indices, count)| {
            let start = visible.len();
            visible.extend(indices);
            Group {
                label,
                range: start..visible.len(),
                count,
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Section {
    range: Range<usize>,
    start: usize,
    end: usize,
}
/// Vertical views measure offsets in terminal rows, Columns in whole columns.
/// Tab.offset remains an entry index for refresh/rename snapshot compatibility.
#[derive(Debug, Clone)]
pub struct Viewport {
    pub mode: ViewMode,
    pub width: u16,
    pub height: u16,
    pub columns: usize,
    pub rows: usize,
    pub offset: usize,
    pub total: usize,
    sections: Vec<Section>,
}
impl Viewport {
    pub fn new(groups: &[Group], mode: ViewMode, width: u16, height: u16, columns: usize) -> Self {
        let rows = usize::from(height.saturating_sub(1)).max(1);
        let mut total = 0;
        let sections = groups
            .iter()
            .map(|g| {
                let start = total;
                total += match mode {
                    ViewMode::Icons => 1 + g.range.len().div_ceil(columns) * 5,
                    ViewMode::Columns => g.range.len().div_ceil(rows),
                    _ => 1 + g.range.len(),
                };
                Section {
                    range: g.range.clone(),
                    start,
                    end: total,
                }
            })
            .collect();
        Self {
            mode,
            width,
            height,
            columns,
            rows,
            offset: 0,
            total,
            sections,
        }
    }
    fn extent(&self) -> usize {
        if self.mode == ViewMode::Columns {
            self.columns
        } else {
            usize::from(self.height).max(1)
        }
    }
    pub fn position(&self, index: usize) -> (usize, usize) {
        let n = self.sections.partition_point(|s| s.range.end <= index);
        let Some(s) = self.sections.get(n) else {
            return (0, 0);
        };
        let local = index - s.range.start;
        match self.mode {
            ViewMode::Icons => (local % self.columns, s.start + 1 + local / self.columns * 5),
            ViewMode::Columns => (s.start + local / self.rows, 1 + local % self.rows),
            _ => (0, s.start + 1 + local),
        }
    }
    fn unit(&self, index: usize) -> usize {
        let (x, y) = self.position(index);
        if self.mode == ViewMode::Columns { x } else { y }
    }
    fn snap(&self, offset: usize, up: bool) -> usize {
        if self.mode != ViewMode::Icons {
            return offset;
        }
        let n = self.sections.partition_point(|s| s.end <= offset);
        let Some(s) = self.sections.get(n) else {
            return self.sections.last().map_or(0, |s| s.end.saturating_sub(5));
        };
        if offset <= s.start {
            return s.start;
        }
        let local = offset - s.start - 1;
        if up {
            (s.start + 1 + local.div_ceil(5) * 5).min(s.end)
        } else {
            s.start + 1 + local / 5 * 5
        }
    }
    fn maximum(&self) -> usize {
        self.snap(self.total.saturating_sub(self.extent()), true)
    }
    pub fn scroll(&mut self, delta: isize) {
        if self.mode == ViewMode::Icons {
            // A title is one row, a tile band five: visit both instead of
            // jumping from a title straight past its first band of files.
            let mut current = 0;
            let mut bands = 0;
            for s in &self.sections {
                let count = 1 + (s.end - s.start - 1) / 5;
                if self.offset >= s.start {
                    current = bands
                        + if self.offset == s.start {
                            0
                        } else {
                            1 + (self.offset - s.start - 1) / 5
                        };
                }
                bands += count;
            }
            let mut wanted = current
                .saturating_add_signed(delta)
                .min(bands.saturating_sub(1));
            for s in &self.sections {
                let count = 1 + (s.end - s.start - 1) / 5;
                if wanted < count {
                    self.offset = s.start + if wanted == 0 { 0 } else { 1 + (wanted - 1) * 5 };
                    break;
                }
                wanted -= count;
            }
            self.offset = self.offset.min(self.maximum());
        } else {
            let step = if self.mode == ViewMode::Columns {
                delta
            } else {
                let limit = self.extent().saturating_sub(1).max(1) as isize;
                delta.clamp(-limit, limit)
            };
            self.offset = self.offset.saturating_add_signed(step).min(self.maximum());
        }
    }
    pub fn follow(&mut self, index: usize) {
        let unit = self.unit(index);
        let size = if self.mode == ViewMode::Icons {
            5.min(self.extent())
        } else {
            1
        };
        if unit < self.offset {
            self.offset = self.snap(unit, false);
        }
        if unit + size > self.offset + self.extent() {
            self.offset = self.snap(unit + size - self.extent(), true);
        }
        self.offset = self.offset.min(self.maximum());
        // Include a group's title whenever its first item comes into focus.
        if let Some(s) = self.sections.iter().find(|s| s.range.start == index)
            && unit + size <= s.start + self.extent()
        {
            self.offset = self.offset.min(s.start);
        }
    }
    pub fn anchor(&mut self, index: usize) {
        self.offset = self
            .sections
            .iter()
            .find(|s| s.range.start == index)
            .map_or_else(|| self.snap(self.unit(index), false), |s| s.start)
            .min(self.maximum());
    }
    pub fn first(&self) -> usize {
        let mut first = 0;
        for s in &self.sections {
            if s.end <= self.offset {
                first = s.range.end;
                continue;
            }
            let skip = match self.mode {
                ViewMode::Columns => self.offset.saturating_sub(s.start) * self.rows,
                ViewMode::Icons => {
                    self.offset.saturating_sub(s.start + 1).div_ceil(5) * self.columns
                }
                _ => self.offset.saturating_sub(s.start + 1),
            };
            return (s.range.start + skip).min(s.range.end.saturating_sub(1));
        }
        first.saturating_sub(1)
    }
    pub fn headers(&self) -> Vec<(usize, usize, usize)> {
        let mut result = vec![];
        for (i, s) in self.sections.iter().enumerate() {
            if self.mode == ViewMode::Columns {
                for col in s.start.max(self.offset)..s.end.min(self.offset + self.columns) {
                    result.push((i, col - self.offset, 0));
                }
            } else if (self.offset..self.offset + self.extent()).contains(&s.start) {
                result.push((i, 0, s.start - self.offset));
            }
        }
        result
    }
    pub fn items(&self) -> Vec<(usize, usize, usize)> {
        let mut result = vec![];
        for s in &self.sections {
            if s.end <= self.offset || s.start >= self.offset + self.extent() {
                continue;
            }
            // Only visit the visible slice, including a partially visible last tile.
            let (start, count) = match self.mode {
                ViewMode::Columns => (
                    s.range.start + self.offset.saturating_sub(s.start) * self.rows,
                    self.columns * self.rows,
                ),
                ViewMode::Icons => (
                    s.range.start
                        + self.offset.saturating_sub(s.start + 1).div_ceil(5) * self.columns,
                    (usize::from(self.height) / 5 + 2) * self.columns,
                ),
                _ => (
                    s.range.start + self.offset.saturating_sub(s.start + 1),
                    usize::from(self.height),
                ),
            };
            for i in start..(start + count).min(s.range.end) {
                let (x, y) = self.position(i);
                if self.mode == ViewMode::Columns {
                    if x >= self.offset
                        && x < self.offset + self.columns
                        && y < usize::from(self.height)
                    {
                        result.push((i, x - self.offset, y));
                    }
                } else if y >= self.offset && y < self.offset + usize::from(self.height) {
                    result.push((i, x, y - self.offset));
                }
            }
        }
        result
    }
    pub fn next(&self, index: usize, dx: isize, dy: isize, len: usize) -> usize {
        let (x, y) = self.position(index);
        (0..len)
            .filter(|&i| i != index)
            .filter_map(|i| {
                let (xx, yy) = self.position(i);
                let along = if dx != 0 {
                    xx as isize - x as isize
                } else {
                    yy as isize - y as isize
                };
                let direction = if dx != 0 { dx } else { dy };
                if along.signum() != direction.signum() {
                    return None;
                }
                let cross = if dx != 0 {
                    yy.abs_diff(y)
                } else {
                    xx.abs_diff(x)
                };
                Some(((along.unsigned_abs(), cross), i))
            })
            .min_by_key(|(distance, _)| *distance)
            .map_or(index, |(_, i)| i)
    }
}

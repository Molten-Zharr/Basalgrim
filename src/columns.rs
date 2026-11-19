//! Persisted detail columns. Widths are shares of the pane, independent of pixels/fonts.
use crate::fs::{self, Entry};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, time::SystemTime};

pub const TOTAL: u32 = 1_000_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Column {
    Name,
    Size,
    Kind,
    Extension,
    Created,
    Modified,
    Accessed,
    Permissions,
    Owner,
    Location,
}
impl Column {
    pub const ALL: [Self; 10] = [
        Self::Name,
        Self::Size,
        Self::Kind,
        Self::Extension,
        Self::Created,
        Self::Modified,
        Self::Accessed,
        Self::Permissions,
        Self::Owner,
        Self::Location,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Имя",
            Self::Size => "Размер",
            Self::Kind => "Тип",
            Self::Extension => "Расширение",
            Self::Created => "Дата создания",
            Self::Modified => "Дата изменения",
            Self::Accessed => "Дата доступа",
            Self::Permissions => "Права доступа",
            Self::Owner => "Владелец",
            Self::Location => "Расположение",
        }
    }
    pub fn text(self, e: &Entry) -> String {
        match self {
            Self::Name => e.name.clone(),
            Self::Size => {
                if e.directory {
                    "—".into()
                } else {
                    fs::size(e.size)
                }
            }
            Self::Kind => fs::kind(e).into(),
            Self::Extension => extension(e),
            Self::Created => fs::date(e.details.created),
            Self::Modified => fs::date(e.modified),
            Self::Accessed => fs::date(e.details.accessed),
            Self::Permissions => e
                .details
                .mode
                .map(|m| format!("{:04o}", m & 0o7777))
                .unwrap_or_else(|| "—".into()),
            Self::Owner => e.details.owner.clone().unwrap_or_else(|| "—".into()),
            Self::Location => fs::clean(&e.path.parent().unwrap_or(&e.path).to_string_lossy()),
        }
    }
    /// Lexicographic database ordering equivalent to the in-memory typed sort.
    pub fn order_key(self, e: &Entry) -> Option<String> {
        self.value(e).map(|v| match v {
            Value::Text(s) => s,
            Value::Number(n) => format!("{n:020}"),
            Value::Time(t) => {
                let signed = match t.duration_since(std::time::UNIX_EPOCH) {
                    Ok(d) => d.as_nanos() as i128,
                    Err(e) => -(e.duration().as_nanos() as i128),
                };
                format!("{:039}", (signed + (i128::MAX / 2)) as u128)
            }
        })
    }
    fn value(self, e: &Entry) -> Option<Value> {
        match self {
            Self::Size => (!e.directory).then_some(Value::Number(e.size)),
            Self::Created => e.details.created.map(Value::Time),
            Self::Modified => e.modified.map(Value::Time),
            Self::Accessed => e.details.accessed.map(Value::Time),
            Self::Permissions => e.details.mode.map(|m| Value::Number(u64::from(m & 0o7777))),
            Self::Owner => e
                .details
                .owner
                .as_ref()
                .map(|v| Value::Text(v.to_lowercase())),
            _ => Some(Value::Text(self.text(e).to_lowercase())),
        }
    }
}
fn extension(e: &Entry) -> String {
    if e.directory {
        return String::new();
    }
    std::path::Path::new(&e.name)
        .extension()
        .filter(|s| !s.is_empty())
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default()
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Details {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<crate::folder_style::FolderId>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    Number(u64),
    Text(String),
    Time(SystemTime),
}
#[derive(Eq, PartialEq)]
struct Ordered {
    value: Option<Value>,
    descending: bool,
}
impl Ord for Ordered {
    fn cmp(&self, other: &Self) -> Ordering {
        match (&self.value, &other.value) {
            (Some(a), Some(b)) => {
                if self.descending {
                    b.cmp(a)
                } else {
                    a.cmp(b)
                }
            }
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (None, None) => Ordering::Equal,
        }
    }
}
impl PartialOrd for Ordered {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
pub fn sort(entries: &mut [Entry], column: Column, descending: bool) {
    entries.sort_by_cached_key(|e| {
        (
            !e.directory,
            Ordered {
                value: column.value(e),
                descending,
            },
            e.name.to_lowercase(),
            e.path.clone(),
        )
    });
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub column: Column,
    pub share: u32,
    #[serde(default)]
    pub manual: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub columns: Vec<Spec>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            columns: vec![Spec {
                column: Column::Name,
                share: TOTAL,
                manual: false,
            }],
        }
    }
}
impl Layout {
    pub fn normalize(&mut self) {
        let mut seen = std::collections::BTreeSet::new();
        self.columns.retain(|s| seen.insert(s.column));
        if let Some(i) = self.columns.iter().position(|s| s.column == Column::Name) {
            let name = self.columns.remove(i);
            self.columns.insert(0, name);
        } else {
            self.columns.insert(
                0,
                Spec {
                    column: Column::Name,
                    share: 0,
                    manual: false,
                },
            );
        }
        if self.columns.iter().any(|s| s.share == 0 || s.share > TOTAL)
            || self.columns.iter().map(|s| u64::from(s.share)).sum::<u64>() != u64::from(TOTAL)
        {
            for s in &mut self.columns {
                s.manual = false;
            }
            self.equalize();
        }
    }
    /// Preserve manually sized shares and divide the remaining space equally.
    pub fn equalize(&mut self) {
        let fixed: u32 = self
            .columns
            .iter()
            .filter(|s| s.manual)
            .map(|s| s.share)
            .sum();
        let count = self.columns.iter().filter(|s| !s.manual).count() as u32;
        if count == 0 {
            return;
        }
        let space = TOTAL.saturating_sub(fixed);
        let mut extra = space % count;
        for s in self.columns.iter_mut().filter(|s| !s.manual) {
            s.share = space / count + u32::from(extra > 0);
            extra = extra.saturating_sub(1);
        }
    }
    pub fn reset(&mut self, column: Column) {
        if let Some(s) = self.columns.iter_mut().find(|s| s.column == column) {
            s.manual = false;
            self.equalize();
        }
    }
    pub fn toggle(&mut self, column: Column) {
        if column == Column::Name {
            return;
        }
        if let Some(i) = self.columns.iter().position(|s| s.column == column) {
            self.columns.remove(i);
        } else {
            // Leave every previous custom share intact while room remains. If all
            // columns are custom, make room proportionally; never create zero width.
            if self.columns.iter().all(|s| s.manual) {
                let n = self.columns.len() as u32;
                for s in &mut self.columns {
                    s.share = s.share * n / (n + 1);
                }
            }
            self.columns.push(Spec {
                column,
                share: 0,
                manual: false,
            });
        }
        if self.columns.iter().all(|s| s.manual) {
            let sum: u32 = self.columns.iter().map(|s| s.share).sum();
            let mut left = TOTAL;
            let last = self.columns.len() - 1;
            for (i, s) in self.columns.iter_mut().enumerate() {
                s.share = if i == last {
                    left
                } else {
                    (u64::from(s.share) * u64::from(TOTAL) / u64::from(sum.max(1))) as u32
                };
                left -= s.share;
            }
        } else {
            self.equalize();
        }
    }
    /// Convert normalized shares to exact integer cells; every column stays reachable.
    pub fn widths(&self, width: u16) -> Vec<u16> {
        let n = self.columns.len();
        if n == 0 {
            return vec![];
        }
        let floor = (width / n as u16).min(3);
        let mut widths: Vec<u16> = self
            .columns
            .iter()
            .map(|s| ((u64::from(width) * u64::from(s.share) / u64::from(TOTAL)) as u16).max(floor))
            .collect();
        let mut sum: u32 = widths.iter().map(|w| u32::from(*w)).sum();
        while sum > u32::from(width) {
            let Some(i) = (0..n).filter(|&i| widths[i] > floor).max_by_key(|&i| {
                i64::from(widths[i]) * i64::from(TOTAL)
                    - i64::from(width) * i64::from(self.columns[i].share)
            }) else {
                break;
            };
            widths[i] -= 1;
            sum -= 1;
        }
        while sum < u32::from(width) {
            let i = (0..n)
                .max_by_key(|&i| {
                    i64::from(width) * i64::from(self.columns[i].share)
                        - i64::from(widths[i]) * i64::from(TOTAL)
                })
                .unwrap();
            widths[i] += 1;
            sum += 1;
        }
        widths
    }
    /// A boundary changes two neighbours. Only the column whose right edge was
    /// grabbed becomes manual; its neighbour remains in the automatic pool.
    pub fn resize(&mut self, index: usize, width: u16, delta: i32) {
        if self.columns.len() < 2 || index >= self.columns.len() || width == 0 {
            return;
        }
        let other = if index + 1 < self.columns.len() {
            index + 1
        } else {
            index - 1
        };
        let delta = i64::from(delta) * i64::from(TOTAL) / i64::from(width);
        let a = i64::from(self.columns[index].share);
        let b = i64::from(self.columns[other].share);
        let min = (3 * u64::from(TOTAL) / u64::from(width)) as i64;
        if a + b < min * 2 {
            return;
        }
        let change = delta.clamp(min - a, b - min);
        if change == 0 {
            return;
        }
        self.columns[index].share = (a + change) as u32;
        self.columns[other].share = (b - change) as u32;
        self.columns[index].manual = true;
    }
}

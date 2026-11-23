//! Shared file-view policy. Column mode fills the viewport vertically, then scrolls right.
use serde::{Deserialize, Serialize};

/// Ratatui cell geometry for one file pane. Zero is the original layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "i8")]
pub struct FileZoom(i8);
impl From<i8> for FileZoom {
    fn from(level: i8) -> Self {
        Self(level.clamp(-1, 3))
    }
}
impl FileZoom {
    pub fn step(self, mode: ViewMode, direction: i8) -> Self {
        let min = if mode == ViewMode::Icons { -1 } else { 0 };
        let current = self.0.max(min);
        let next = (current + direction.signum()).clamp(min, 3);
        if next == current { self } else { Self(next) }
    }
    pub fn tile_width(self) -> usize {
        (18 + 6 * self.0) as usize
    }
    pub fn tile_height(self) -> usize {
        (5 + self.0) as usize
    }
    pub fn row_height(self) -> usize {
        1 + self.0.max(0) as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewMode {
    Icons,
    List,
    Columns,
    Tree,
}
impl ViewMode {
    pub const ALL: [Self; 4] = [Self::Icons, Self::List, Self::Columns, Self::Tree];
    pub fn name(self) -> &'static str {
        match self {
            Self::Icons => "Значки",
            Self::List => "Список",
            Self::Columns => "Столбцы",
            Self::Tree => "Таблица с деревом",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Icons => "Значки слева направо, затем следующая строка.",
            Self::List => "Один столбец имён. Прокрутка сверху вниз.",
            Self::Columns => {
                "Сверху вниз на высоту окна, затем вправо. Колесо прокручивает столбцы."
            }
            Self::Tree => {
                "Таблица: раскрывайте папки стрелкой рядом с именем. Двойной щелчок входит в папку."
            }
        }
    }
}

//! Shared file-view policy. Column mode fills the viewport vertically, then scrolls right.
use serde::{Deserialize, Serialize};
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

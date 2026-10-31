use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Delete,
    Insert,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPress {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyPress {
    pub fn normalized(mut self) -> Self {
        if let Key::Char(c) = self.key {
            let c = c.to_lowercase().next().unwrap_or(c);
            let mapped = "йцукенгшщзхъфывапролджэячсмитьбю"
                .chars()
                .position(|v| v == c)
                .and_then(|i| "qwertyuiop[]asdfghjkl;'zxcvbnm,.".chars().nth(i))
                .unwrap_or(c);
            self.key = Key::Char(mapped);
            if matches!(mapped, '+' | '-' | '/') {
                self.shift = false;
            }
        }
        self
    }
    pub fn label(self) -> String {
        let key = match self.key {
            Key::Char(' ') => "Space".into(),
            Key::Char(c) => c.to_uppercase().to_string(),
            Key::F(n) => format!("F{n}"),
            Key::Escape => "Esc".into(),
            Key::PageUp => "PgUp".into(),
            Key::PageDown => "PgDn".into(),
            Key::Up => "↑".into(),
            Key::Down => "↓".into(),
            Key::Left => "←".into(),
            Key::Right => "→".into(),
            other => format!("{other:?}"),
        };
        format!(
            "{}{}{}{key}",
            if self.ctrl { "Ctrl+" } else { "" },
            if self.alt { "Alt+" } else { "" },
            if self.shift { "Shift+" } else { "" }
        )
    }
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub fn ctrl(c: char) -> Self {
        Self {
            ctrl: true,
            ..Self::plain(Key::Char(c))
        }
    }
}

pub enum Input {
    /// Hover position only; never changes keyboard focus or selection. None clears it.
    Pointer(Option<ratatui::layout::Position>),
    Key(KeyPress),
    Text(String),
    PointerDown {
        x: u16,
        y: u16,
        double: bool,
        toggle: bool,
    },
    Drag {
        x: u16,
        y: u16,
    },
    PointerUp {
        x: u16,
        y: u16,
    },
    CancelDrag,
    SecondaryClick {
        x: u16,
        y: u16,
    },
    Click {
        x: u16,
        y: u16,
        double: bool,
        toggle: bool,
    },
    Scroll {
        x: u16,
        y: u16,
        delta: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Commands,
    Operations,
    CancelOperation,
    Back,
    Forward,
    Parent,
    Home,
    Refresh,
    Search,
    Path,
    Grid,
    Dual,
    Preview,
    Hidden,
    Sort,
    Copy,
    Cut,
    Paste,
    CopyToOtherPane,
    MoveToOtherPane,
    Duplicate,
    Rename,
    NewFolder,
    NewFile,
    Trash,
    SelectAll,
    NewTab,
    CloseTab,
    NextTab,
    Help,
    Quit,
    Settings,
    Open,
    Mark,
    NextPane,
    FontLarger,
    FontSmaller,
}

impl Action {
    pub const ALL: &'static [Self] = &[
        Self::Commands,
        Self::Back,
        Self::Forward,
        Self::Parent,
        Self::Home,
        Self::Refresh,
        Self::Search,
        Self::Path,
        Self::Grid,
        Self::Dual,
        Self::Preview,
        Self::Hidden,
        Self::Sort,
        Self::Copy,
        Self::Cut,
        Self::Paste,
        Self::CopyToOtherPane,
        Self::MoveToOtherPane,
        Self::Duplicate,
        Self::Rename,
        Self::NewFolder,
        Self::NewFile,
        Self::Trash,
        Self::SelectAll,
        Self::NewTab,
        Self::CloseTab,
        Self::NextTab,
        Self::Help,
        Self::Quit,
        Self::Settings,
        Self::Open,
        Self::Mark,
        Self::NextPane,
        Self::FontLarger,
        Self::FontSmaller,
        Self::Operations,
        Self::CancelOperation,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Commands => "Меню действий",
            Self::Operations => "Операции",
            Self::CancelOperation => "Отменить текущую операцию",
            Self::Back => "История: назад",
            Self::Forward => "История: вперёд",
            Self::Parent => "Родительская папка",
            Self::Home => "Домашняя папка",
            Self::Refresh => "Обновить",
            Self::Search => "Фильтр папки",
            Self::Path => "Перейти по пути",
            Self::Grid => "Сетка / список",
            Self::Dual => "Две панели",
            Self::Preview => "Предпросмотр",
            Self::Hidden => "Скрытые файлы",
            Self::Sort => "Сортировка",
            Self::Copy => "Копировать",
            Self::Cut => "Вырезать",
            Self::Paste => "Вставить",
            Self::CopyToOtherPane => "Копировать в соседнюю панель",
            Self::MoveToOtherPane => "Переместить в соседнюю панель",
            Self::Duplicate => "Дублировать",
            Self::Rename => "Переименовать",
            Self::NewFolder => "Новая папка",
            Self::NewFile => "Новый файл",
            Self::Trash => "В корзину",
            Self::SelectAll => "Выбрать все",
            Self::NewTab => "Новая вкладка",
            Self::CloseTab => "Закрыть вкладку",
            Self::NextTab => "Следующая вкладка",
            Self::Help => "Справка",
            Self::Quit => "Выход",
            Self::Settings => "Настройки",
            Self::Open => "Открыть",
            Self::Mark => "Отметить",
            Self::NextPane => "Сменить панель",
            Self::FontLarger => "Увеличить шрифт GUI",
            Self::FontSmaller => "Уменьшить шрифт GUI",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub action: Action,
    pub key: KeyPress,
}
pub fn default_bindings() -> Vec<Binding> {
    use Action::*;
    let mut result: Vec<_> = [
        (Help, Key::F(1)),
        (Rename, Key::F(2)),
        (Preview, Key::F(3)),
        (Refresh, Key::F(5)),
        (NewFolder, Key::F(7)),
        (Trash, Key::F(8)),
        (Sort, Key::F(9)),
        (Settings, Key::F(10)),
        (Search, Key::Char('/')),
        (Parent, Key::Backspace),
        (Trash, Key::Delete),
        (Open, Key::Enter),
        (Mark, Key::Char(' ')),
        (NextPane, Key::Tab),
    ]
    .into_iter()
    .map(|(action, key)| Binding {
        action,
        key: KeyPress::plain(key),
    })
    .collect();
    result.push(Binding {
        action: Commands,
        key: KeyPress {
            shift: true,
            ..KeyPress::plain(Key::F(10))
        },
    });
    result.push(Binding {
        action: Operations,
        key: KeyPress::plain(Key::F(6)),
    });
    result.push(Binding {
        action: CancelOperation,
        key: KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::F(6))
        },
    });
    for (action, key, ctrl, shift) in [
        (CopyToOtherPane, Key::F(5), false, true),
        (MoveToOtherPane, Key::F(6), false, true),
        (Duplicate, Key::F(5), true, false),
    ] {
        result.push(Binding {
            action,
            key: KeyPress {
                key,
                ctrl,
                alt: false,
                shift,
            },
        });
    }
    for (action, c) in [
        (Quit, 'q'),
        (Copy, 'c'),
        (Cut, 'x'),
        (Paste, 'v'),
        (SelectAll, 'a'),
        (Path, 'l'),
        (Search, 'f'),
        (Grid, 'g'),
        (Hidden, 'h'),
        (Dual, 'd'),
        (NewTab, 't'),
        (CloseTab, 'w'),
        (Refresh, 'r'),
        (NewFile, 'n'),
        (Settings, ','),
        (FontLarger, '+'),
        (FontLarger, '='),
        (FontSmaller, '-'),
    ] {
        result.push(Binding {
            action,
            key: KeyPress::ctrl(c),
        });
    }
    for (action, key) in [
        (Back, Key::Left),
        (Forward, Key::Right),
        (Parent, Key::Up),
        (Home, Key::Home),
    ] {
        result.push(Binding {
            action,
            key: KeyPress {
                alt: true,
                ..KeyPress::plain(key)
            },
        });
    }
    result.push(Binding {
        action: NewFolder,
        key: KeyPress {
            shift: true,
            ..KeyPress::ctrl('n')
        },
    });
    result.push(Binding {
        action: NextTab,
        key: KeyPress {
            ctrl: true,
            ..KeyPress::plain(Key::Tab)
        },
    });
    result
}
pub fn validate_binding(action: Action, key: KeyPress) -> Result<()> {
    let key = key.normalized();
    ensure!(key.key != Key::Escape, "Esc зарезервирована для отмены");
    ensure!(
        !matches!(key.key,Key::F(n) if !(1..=12).contains(&n)),
        "Допустимы F1–F12"
    );
    ensure!(
        !matches!(key.key,Key::Char(c) if !c.is_ascii() || c.is_control()),
        "Используйте букву, цифру, знак или F1–F12"
    );
    if !key.ctrl && !key.alt {
        ensure!(
            !matches!(
                key.key,
                Key::Up
                    | Key::Down
                    | Key::Left
                    | Key::Right
                    | Key::Home
                    | Key::End
                    | Key::PageUp
                    | Key::PageDown
            ),
            "Клавиши навигации зарезервированы"
        );
        ensure!(
            match key.key {
                Key::Enter => action == Action::Open,
                Key::Tab => action == Action::NextPane,
                Key::Char(' ') => action == Action::Mark,
                Key::Backspace => action == Action::Parent,
                _ => true,
            },
            "Эта клавиша зарезервирована для навигации"
        );
    }
    ensure!(
        key != KeyPress::ctrl('u'),
        "Ctrl+U зарезервировано для очистки поля ввода"
    );
    Ok(())
}
pub fn shortcut_choices() -> Vec<Key> {
    "abcdefghijklmnopqrstuvwxyz0123456789+-=,./;[]\\'`"
        .chars()
        .map(Key::Char)
        .chain((1..=12).map(Key::F))
        .chain([
            Key::Char(' '),
            Key::Enter,
            Key::Tab,
            Key::Backspace,
            Key::Delete,
            Key::Insert,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ])
        .collect()
}

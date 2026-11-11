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
    /// Shift state for pointer selection in file views and text fields.
    SelectionModifier(bool),
    /// Hover position only; never changes keyboard focus or selection. None clears it.
    Pointer(Option<ratatui::layout::Position>),
    Key(KeyPress),
    Text(String),
    Paste(String),
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
    DragModifiers {
        copy: bool,
    },
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
    OpenWith,
    Properties,
    Network,
    Devices,
    Places,
    Bookmark,
    Favorites,
    Recent,
    Favorite,
    Unfavorite,
    ArchiveCreate,
    ArchiveExtract,
    Commands,
    Operations,
    HistoryStorage,
    CancelOperation,
    Undo,
    Redo,
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
    FolderSize,
    Terminal,
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
    CopyPath,
    TrashView,
    RestoreTrash,
    RestoreTrashHere,
    DeleteTrash,
    EmptyTrash,
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
        Self::FolderSize,
        Self::Terminal,
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
        Self::HistoryStorage,
        Self::CancelOperation,
        Self::Undo,
        Self::Redo,
        Self::CopyPath,
        Self::TrashView,
        Self::RestoreTrash,
        Self::RestoreTrashHere,
        Self::DeleteTrash,
        Self::EmptyTrash,
        Self::ArchiveCreate,
        Self::ArchiveExtract,
        Self::Places,
        Self::Bookmark,
        Self::Favorites,
        Self::Recent,
        Self::Favorite,
        Self::Unfavorite,
        Self::Devices,
        Self::Network,
        Self::Properties,
        Self::OpenWith,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Devices => "Накопители",
            Self::Properties => "Свойства",
            Self::OpenWith => "Открыть с помощью…",
            Self::Network => "Сеть / подключиться к серверу",
            Self::Places => "Места / закладки",
            Self::Bookmark => "Добавить папку в закладки",
            Self::Favorites => "Избранное",
            Self::Recent => "Недавние файлы",
            Self::Favorite => "В избранное",
            Self::Unfavorite => "Убрать из избранного",
            Self::ArchiveCreate => "Создать архив…",
            Self::ArchiveExtract => "Распаковать…",
            Self::Commands => "Меню действий",
            Self::Operations => "Операции",
            Self::HistoryStorage => "Хранилище undo/redo",
            Self::CancelOperation => "Отменить текущую операцию",
            Self::Undo => "Отменить завершённую операцию",
            Self::Redo => "Повторить отменённую операцию",
            Self::CopyPath => "Копировать путь папки",
            Self::TrashView => "Открыть корзину",
            Self::RestoreTrash => "Восстановить из корзины",
            Self::RestoreTrashHere => "Восстановить в текущую папку",
            Self::DeleteTrash => "Удалить из корзины",
            Self::EmptyTrash => "Очистить корзину",
            Self::Back => "История: назад",
            Self::Forward => "История: вперёд",
            Self::Parent => "Родительская папка",
            Self::Home => "Домашняя папка",
            Self::Refresh => "Обновить",
            Self::Search => "Фильтр папки",
            Self::Path => "Перейти по пути",
            Self::Grid => "Сетка / список",
            Self::Dual => "Вторая панель",
            Self::Preview => "Предпросмотр",
            Self::FolderSize => "Размер папки",
            Self::Terminal => "Терминал",
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
        (Terminal, Key::F(4)),
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
    for (action, key, alt, shift) in [
        (Places, 'b', false, false),
        (Devices, 'm', true, false),
        (Network, 'k', true, false),
        (Bookmark, 'b', true, false),
        (Favorites, 's', true, false),
        (Recent, 'n', true, false),
        (Favorite, 'f', true, false),
        (Unfavorite, 'f', true, true),
    ] {
        result.push(Binding {
            action,
            key: KeyPress {
                key: Key::Char(key),
                ctrl: true,
                alt,
                shift,
            },
        });
    }
    for (action, key) in [(ArchiveCreate, 'a'), (ArchiveExtract, 'e')] {
        result.push(Binding {
            action,
            key: KeyPress {
                alt: true,
                ..KeyPress::ctrl(key)
            },
        });
    }
    result.push(Binding {
        action: Properties,
        key: KeyPress {
            alt: true,
            ..KeyPress::plain(Key::Enter)
        },
    });
    result.push(Binding {
        action: OpenWith,
        key: KeyPress {
            ctrl: true,
            alt: false,
            shift: false,
            key: Key::Enter,
        },
    });
    result.push(Binding {
        action: FolderSize,
        key: KeyPress {
            shift: true,
            ..KeyPress::plain(Key::F(4))
        },
    });
    result.push(Binding {
        action: Commands,
        key: KeyPress {
            shift: true,
            ..KeyPress::plain(Key::F(10))
        },
    });
    result.push(Binding {
        action: CopyPath,
        key: KeyPress {
            shift: true,
            ..KeyPress::ctrl('c')
        },
    });
    for (action, key) in [
        (
            TrashView,
            KeyPress {
                shift: true,
                ..KeyPress::ctrl('b')
            },
        ),
        (
            RestoreTrash,
            KeyPress {
                shift: true,
                ..KeyPress::ctrl('r')
            },
        ),
        (
            RestoreTrashHere,
            KeyPress {
                alt: true,
                ..KeyPress::ctrl('r')
            },
        ),
    ] {
        result.push(Binding { action, key });
    }
    result.push(Binding {
        action: DeleteTrash,
        key: KeyPress {
            shift: true,
            ..KeyPress::plain(Key::Delete)
        },
    });
    result.push(Binding {
        action: EmptyTrash,
        key: KeyPress {
            shift: true,
            ..KeyPress::ctrl('e')
        },
    });
    result.push(Binding {
        action: Operations,
        key: KeyPress::plain(Key::F(6)),
    });
    result.push(Binding {
        action: HistoryStorage,
        key: KeyPress {
            ctrl: true,
            shift: true,
            ..KeyPress::plain(Key::F(6))
        },
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
        (Undo, 'z'),
        (Redo, 'y'),
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

use super::*;
use crate::input::{Key, KeyPress};

#[derive(Clone, Copy)]
pub enum PaletteControl {
    Row(usize),
    Run,
    Clear,
    Move(isize),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteRow {
    pub action: Action,
    pub shortcut: String,
    pub reason: Option<&'static str>,
}
pub struct PaletteView {
    pub token: u64,
    pub revision: u64,
    pub owner: PathOwner,
    pub request: u64,
    pub query: String,
    pub rows: Vec<PaletteRow>,
    pub selected: usize,
    pub offset: usize,
    pub capacity: usize,
    pub area: Rect,
    pub error: Option<&'static str>,
    selection: Vec<PathBuf>,
    trash: bool,
    opposite: Option<(PathOwner, u64)>,
}
fn normalized(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}
fn aliases(action: Action) -> &'static str {
    use Action::*;
    match action {
        Extensions => "extensions plugins плагины установка расширения",
        ExtensionCommands => "extensions plugins команды расширений",
        Copy => "copy копия буфер",
        Cut => "cut буфер",
        Paste => "paste буфер",
        CopyToOtherPane => "copy panel копия",
        MoveToOtherPane => "move panel перенос",
        Duplicate => "duplicate копия",
        Rename => "rename имя массовое переименование",
        NewFile => "new create file создать файл",
        NewFolder => "new create folder mkdir создать папку каталог директория",
        Trash => "trash delete удалить",
        TrashView => "trash bin",
        RestoreTrash | RestoreTrashHere => "restore trash восстановление",
        DeleteTrash => "delete remove trash удалить навсегда",
        EmptyTrash => "empty trash очистка",
        Open => "open открыть",
        OpenWith => "open with application приложение программа",
        Properties => "properties chmod permissions права доступ свойства",
        Back => "back назад",
        Forward => "forward вперед",
        Parent => "parent up вверх",
        Home => "home домой",
        Path => "path location адрес папка",
        CopyPath => "copy path address адрес",
        Search => "search find поиск найти фильтр",
        Refresh => "refresh reload",
        Grid => "grid list вид",
        Dual => "dual panes split панели",
        NextPane => "next pane переключить",
        Preview => "preview просмотр",
        FolderSize => "folder size размер подсчет",
        Terminal => "terminal shell консоль",
        Hidden => "hidden скрытые",
        Sort => "sort порядок",
        NewTab => "new tab добавить вкладка",
        CloseTab => "close tab закрыть вкладка",
        NextTab => "next tab переключить вкладка",
        Help => "help помощь",
        Settings => "settings preferences тема цвет шрифт клавиши",
        Quit => "quit exit закрыть выйти",
        SelectAll => "select all выделить все",
        Mark => "mark select выбрать выделить",
        FontLarger => "font zoom in масштаб",
        FontSmaller => "font zoom out масштаб",
        Operations => "operations progress history журнал история прогресс",
        CancelOperation => "cancel stop остановить",
        HistoryStorage => "storage cleanup очистка журнал история",
        Undo => "undo отмена",
        Redo => "redo повтор",
        ArchiveCreate => "create archive zip tar архив",
        ArchiveExtract => "extract unpack archive zip tar извлечь архив",
        Places | Bookmark => "places bookmarks закладки",
        Favorites | Favorite | Unfavorite => "favorites избранное",
        Recent => "recent недавние",
        Devices => "devices drives mount eject диски накопители подключить извлечь",
        Network => "network connect server smb sftp ftp webdav сеть сервер подключить",
        Commands | Palette => "",
    }
}
impl App {
    pub fn palette_dialog(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::CommandFilter,
                ..
            })
        )
    }
    pub(super) fn open_palette(&mut self) {
        let token = self.token();
        self.finish_marquee(true);
        self.cancel_file_drag();
        self.palette_view = Some(PaletteView {
            token,
            revision: 0,
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            query: String::new(),
            rows: vec![],
            selected: 0,
            offset: 0,
            capacity: 1,
            area: Rect::default(),
            error: None,
            selection: self.tab().targets(),
            trash: self.in_trash(),
            opposite: self.settings.dual.then(|| {
                (
                    PathOwner::current(self, 1 - self.active),
                    self.file_tab(1 - self.active).request,
                )
            }),
        });
        self.begin_edit(EditKind::CommandFilter, String::new(), 0..0);
        self.update_palette();
    }
    pub(super) fn close_palette(&mut self) {
        self.palette_view = None;
        if self.palette_dialog() {
            self.dialog = None;
            self.editor_menu = None;
            self.editor_drag = None;
            self.text_paste = None;
            self.text_paste_fallback = None;
        }
    }
    fn palette_reason(&self, action: Action) -> Option<&'static str> {
        use Action::*;
        if action == Quit && self.desktop.working {
            return Some("Дождитесь регистрации приложения");
        }
        let mutation = matches!(
            action,
            Paste
                | Rename
                | Trash
                | NewFile
                | NewFolder
                | Duplicate
                | CopyToOtherPane
                | MoveToOtherPane
                | ArchiveCreate
                | ArchiveExtract
                | RestoreTrash
                | RestoreTrashHere
                | DeleteTrash
                | EmptyTrash
                | Undo
                | Redo
        );
        if (mutation || action == Quit)
            && (self.busy
                || self.extensions.working.is_some()
                || self.devices.working.is_some()
                || self.network.working.is_some())
        {
            return Some("Дождитесь завершения текущей операции");
        }
        if mutation && (self.clipboard_reads_pending() || self.external_drop_reading()) {
            return Some("Дождитесь чтения буфера или отмените операцию");
        }
        if self.in_archive() && Self::archive_writable_action(action) {
            return Some("Архив открыт только для чтения; сначала распакуйте файлы");
        }
        if self.in_trash()
            && matches!(
                action,
                Copy | Cut
                    | Paste
                    | Rename
                    | Duplicate
                    | CopyToOtherPane
                    | MoveToOtherPane
                    | NewFile
                    | NewFolder
                    | Forward
                    | Open
                    | Trash
                    | TrashView
            )
        {
            return Some("В корзине используйте восстановление или удаление из корзины");
        }
        if action == Terminal && !self.terminal.exists() && (self.in_trash() || self.in_archive()) {
            return Some("Откройте обычную папку для запуска терминала");
        }
        if !self.in_trash()
            && matches!(
                action,
                RestoreTrash | RestoreTrashHere | DeleteTrash | EmptyTrash
            )
        {
            return Some("Сначала откройте корзину");
        }
        if self.tab().loading
            && matches!(
                action,
                Open | Copy
                    | Cut
                    | Rename
                    | Trash
                    | NewFile
                    | NewFolder
                    | Paste
                    | Duplicate
                    | CopyToOtherPane
                    | MoveToOtherPane
                    | Mark
                    | SelectAll
                    | Terminal
            )
        {
            return Some("Дождитесь чтения папки");
        }
        if action == Open && self.tab().selection_count() != 1 {
            return Some("Выберите один файл или папку");
        }
        if matches!(action, CopyToOtherPane | MoveToOtherPane) && self.other_destination().is_none()
        {
            return Some("Откройте обычную папку во второй панели");
        }
        if self.command_enabled(action, MenuContext::All) {
            return None;
        }
        Some(match action {
            OpenWith => "Выберите один обычный файл",
            Properties => "Выберите один файл или папку вне корзины",
            Open | Copy | Cut | Rename | Trash | Duplicate | Mark | CopyToOtherPane
            | MoveToOtherPane => "Сначала выберите файлы",
            Paste => "Буфер файлов пуст или вставка здесь недоступна",
            CloseTab | NextTab => "В этой панели открыта только одна вкладка",
            NextPane => "Сначала включите вторую панель",
            Back => "В истории нет предыдущей папки",
            Forward => "В истории нет следующей папки",
            Parent => "Вы уже в корневой папке",
            Undo => "Нет доступной операции для отмены",
            Redo => "Нет доступной операции для повтора",
            CancelOperation => "Нет выполняющейся файловой операции",
            HistoryStorage => "Журнал операций недоступен",
            ArchiveExtract => "Выберите поддерживаемый архив или его содержимое",
            ArchiveCreate => "Выберите файлы в обычной папке",
            FolderSize => "Откройте обычную папку",
            SelectAll => "В списке нет файлов",
            RestoreTrash | RestoreTrashHere | DeleteTrash => "Выберите доступные элементы корзины",
            EmptyTrash => "Корзина пуста или ещё читается",
            Bookmark => "Папка уже в быстром доступе или хранилище закладок недоступно",
            Favorite | Unfavorite => "Проверьте выбор файлов и их наличие в избранном",
            _ => "Команда сейчас недоступна",
        })
    }
    pub(crate) fn update_palette(&mut self) {
        let Some(view) = &self.palette_view else {
            return;
        };
        if !self.palette_dialog() {
            self.palette_view = None;
            return;
        }
        if !view.owner.valid(self)
            || view.owner.pane != self.active
            || view.request != self.tab().request
            || view.trash != self.in_trash()
            || view.selection != self.tab().targets()
        {
            self.close_palette();
            self.message(
                "Папка или выбор изменились; откройте поиск команд снова",
                false,
            );
            return;
        }
        let Some(Dialog::Edit { text, .. }) = &self.dialog else {
            return;
        };
        let query = text.clone();
        let error =
            (query.chars().count() > 256).then_some("Запрос не должен превышать 256 символов");
        let folded = normalized(query.trim());
        let words = folded.split_whitespace().collect::<Vec<_>>();
        let mut ranked = Vec::new();
        if error.is_none() {
            for &action in Action::ALL {
                if matches!(action, Action::Commands | Action::Palette) {
                    continue;
                }
                let shortcut = self.settings.shortcuts(action);
                let label = normalized(action.name());
                let searchable = format!("{} {} {}", label, normalized(&shortcut), aliases(action));
                if !words.iter().all(|word| searchable.contains(word)) {
                    continue;
                }
                let exact_alias = aliases(action)
                    .split_whitespace()
                    .any(|word| word == folded);
                let exact_key = self.settings.bindings.iter().any(|binding| {
                    binding.action == action && normalized(&binding.key.label()) == folded
                });
                let rank = if folded.is_empty() || label == folded || exact_alias || exact_key {
                    0
                } else if label.starts_with(&folded) {
                    1
                } else if words.iter().all(|w| label.contains(w)) {
                    2
                } else {
                    3
                };
                ranked.push((
                    rank,
                    PaletteRow {
                        action,
                        shortcut,
                        reason: self.palette_reason(action),
                    },
                ));
            }
        }
        ranked.sort_by_key(|(rank, _)| *rank);
        let rows = ranked.into_iter().map(|(_, row)| row).collect::<Vec<_>>();
        let view = self.palette_view.as_mut().unwrap();
        if view.query != query || view.rows != rows {
            let previous = view.rows.get(view.selected).map(|r| r.action);
            let selected = (view.query == query)
                .then(|| rows.iter().position(|r| Some(r.action) == previous))
                .flatten();
            // Never substitute an available but different action for a disabled match.
            view.selected = selected.unwrap_or(0);
            if view.query != query {
                view.offset = 0;
            }
            view.query = query;
            view.rows = rows;
            view.error = error;
            view.revision += 1;
        }
    }
    pub(super) fn palette_control(
        &mut self,
        token: u64,
        revision: u64,
        control: PaletteControl,
        double: bool,
    ) {
        self.update_palette();
        let Some(view) = &mut self.palette_view else {
            return;
        };
        if view.token != token || view.revision != revision {
            return;
        }
        match control {
            PaletteControl::Row(index) => {
                if index >= view.rows.len() {
                    return;
                }
                view.selected = index;
                // Selection doesn't change row identities: two presses in a single
                // frontend frame can still form a valid double click.
                if double {
                    self.confirm_palette();
                }
            }
            PaletteControl::Run => self.confirm_palette(),
            PaletteControl::Clear => {
                self.begin_edit(EditKind::CommandFilter, String::new(), 0..0);
                self.update_palette();
            }
            PaletteControl::Move(delta) => self.palette_move(delta),
        }
    }
    pub(super) fn palette_move(&mut self, delta: isize) {
        self.update_palette();
        if let Some(view) = &mut self.palette_view {
            view.selected = view
                .selected
                .saturating_add_signed(delta)
                .min(view.rows.len().saturating_sub(1));
        }
    }
    pub(super) fn confirm_palette(&mut self) {
        self.update_palette();
        let Some(view) = &self.palette_view else {
            return;
        };
        let Some(row) = view.rows.get(view.selected) else {
            return;
        };
        if row.reason.is_some() {
            return;
        }
        let action = row.action;
        if matches!(action, Action::CopyToOtherPane | Action::MoveToOtherPane)
            && view.opposite.as_ref().is_none_or(|(owner, request)| {
                !self.settings.dual
                    || !owner.valid(self)
                    || *request != self.file_tab(owner.pane).request
            })
        {
            self.close_palette();
            self.message(
                "Вторая панель изменилась; откройте поиск команд снова",
                false,
            );
            return;
        }
        self.close_palette();
        self.action(action);
    }
    pub(super) fn palette_key(&mut self, press: KeyPress) -> bool {
        if !self.palette_dialog() || self.editor_menu.is_some() {
            return false;
        }
        if press.key == Key::Escape {
            self.close_palette();
            return true;
        }
        if press.ctrl || press.alt {
            return false;
        }
        match press.key {
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                let page = self.palette_view.as_ref().map_or(1, |v| v.capacity) as isize;
                self.palette_move(match press.key {
                    Key::Up => -1,
                    Key::Down => 1,
                    Key::PageUp => -page,
                    _ => page,
                });
                true
            }
            Key::Enter => {
                self.confirm_palette();
                true
            }
            _ => false,
        }
    }
}

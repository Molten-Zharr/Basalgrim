use super::*;
use crate::input::{Key, KeyPress};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuContext {
    All,
    Entries,
    Directory,
    Trash,
    TrashBackground,
}

pub struct CommandMenu {
    pub context: MenuContext,
    pub anchor: Position,
    pub selected: usize,
    pub offset: usize,
    pub capacity: usize,
}

pub struct CommandItem {
    pub action: Action,
    pub enabled: bool,
}

impl App {
    pub fn command_items(&self) -> Vec<CommandItem> {
        use Action::*;
        let context = match &self.dialog {
            Some(Dialog::Commands(menu)) => menu.context,
            _ => MenuContext::All,
        };
        let actions: &[Action] = match context {
            MenuContext::Entries if self.in_archive() => {
                &[Open, OpenWith, Properties, ArchiveExtract]
            }
            MenuContext::Directory if self.in_archive() => &[
                ArchiveExtract,
                Properties,
                Sort,
                Grid,
                Hidden,
                Refresh,
                Search,
                SelectAll,
                CopyPath,
            ],
            MenuContext::Trash => &[RestoreTrash, RestoreTrashHere, DeleteTrash],
            MenuContext::TrashBackground => &[EmptyTrash, Sort, Grid, Refresh],
            MenuContext::All if self.in_trash() => &[
                Palette,
                RestoreTrash,
                RestoreTrashHere,
                DeleteTrash,
                EmptyTrash,
                SelectAll,
                Search,
                Sort,
                Refresh,
                Operations,
                HistoryStorage,
            ],
            MenuContext::Entries => &[
                ExtensionCommands,
                Open,
                OpenWith,
                Properties,
                Copy,
                Cut,
                Rename,
                Duplicate,
                CopyToOtherPane,
                MoveToOtherPane,
                FolderSize,
                Trash,
                ArchiveCreate,
                ArchiveExtract,
                Favorite,
                Unfavorite,
            ],
            MenuContext::Directory => &[
                ExtensionCommands,
                NewFile,
                NewFolder,
                Paste,
                Properties,
                FolderSize,
                Sort,
                Grid,
                Hidden,
                Refresh,
                Search,
                SelectAll,
                CopyPath,
                Bookmark,
                Places,
            ],
            MenuContext::All => &[
                Extensions,
                ExtensionCommands,
                Palette,
                Open,
                OpenWith,
                Properties,
                ArchiveCreate,
                ArchiveExtract,
                Mark,
                SelectAll,
                Copy,
                Cut,
                Paste,
                Rename,
                Trash,
                TrashView,
                NewFolder,
                NewFile,
                CopyToOtherPane,
                MoveToOtherPane,
                Duplicate,
                Operations,
                HistoryStorage,
                Undo,
                Redo,
                CancelOperation,
                NewTab,
                CloseTab,
                NextTab,
                NextPane,
                Path,
                CopyPath,
                Search,
                Refresh,
                Grid,
                Dual,
                Preview,
                Terminal,
                FolderSize,
                Hidden,
                Sort,
                Back,
                Forward,
                Parent,
                Home,
                FontLarger,
                FontSmaller,
                Settings,
                Help,
                Quit,
                Places,
                Bookmark,
                Favorites,
                Recent,
                Favorite,
                Unfavorite,
                Devices,
                Network,
            ],
        };
        actions
            .iter()
            .copied()
            .map(|action| CommandItem {
                action,
                enabled: self.command_enabled(action, context),
            })
            .filter(|item| (context == MenuContext::All && !self.in_trash()) || item.enabled)
            .collect()
    }

    pub(super) fn command_enabled(&self, action: Action, context: MenuContext) -> bool {
        use Action::*;
        let count = self.tab().selection_count();
        let selection = count > 0;
        (match action {
            Extensions => true,
            ExtensionCommands => self.has_extension_commands(context == MenuContext::Directory),
            OpenWith => self.can_open_with(),
            Properties => self
                .detail_source(context == MenuContext::Directory)
                .is_some(),
            Bookmark => self.can_bookmark(),
            Favorite => self.can_favorite(true),
            Unfavorite => self.can_favorite(false),
            Places if context != MenuContext::All => self.library.ready,
            ArchiveCreate => {
                selection
                    && !self.busy
                    && !self.in_archive()
                    && !self.in_trash()
                    && !self.tab().loading
            }
            ArchiveExtract => {
                self.can_extract()
                    || (context == MenuContext::Directory
                        && self.in_archive()
                        && !self.busy
                        && !self.tab().loading)
            }
            FolderSize => {
                self.folder_size_target(context == MenuContext::Directory)
                    .is_some()
                    && (context != MenuContext::Entries || self.single_folder().is_some())
            }
            DeleteTrash => {
                !self.busy && self.in_trash() && self.trash.as_ref().is_some_and(|t| t.can_delete())
            }
            EmptyTrash => {
                !self.busy
                    && self.in_trash()
                    && self.trash.as_ref().is_some_and(|t| {
                        !t.loading && t.error.is_none() && !t.catalog.deletions.is_empty()
                    })
            }
            RestoreTrash | RestoreTrashHere => {
                !self.busy && self.trash.as_ref().is_some_and(|t| t.can_restore())
            }
            SelectAll if self.in_trash() => !self.trash.as_ref().unwrap().visible.is_empty(),
            Mark if self.in_trash() => self.trash.as_ref().unwrap().entry().is_some(),
            Open if context == MenuContext::Entries && count != 1 => false,
            Rename if context == MenuContext::Entries && count == 1 => self
                .tab()
                .entry()
                .is_some_and(|entry| entry.path.file_name().and_then(|n| n.to_str()).is_some()),
            SelectAll => !self.tab().visible.is_empty(),
            Open | Mark | Copy | Cut | Rename | Trash => selection,
            Paste => self.can_paste(),
            NewFolder | NewFile => !self.busy,
            CopyToOtherPane | MoveToOtherPane => {
                selection && !self.busy && self.other_destination().is_some()
            }
            Duplicate => selection && !self.busy,
            CancelOperation => {
                self.busy
                    || self.extensions.working.is_some()
                    || self.clipboard_reads_pending()
                    || self.external_drop_reading()
            }
            Undo => self.replay_candidate(false).is_some(),
            Redo => self.replay_candidate(true).is_some(),
            HistoryStorage => self.journal_enabled,
            CloseTab | NextTab => self.panes[self.active].tabs.len() > 1,
            NextPane => self.settings.dual,
            Back => !self.tab().back.is_empty(),
            Forward => !self.tab().forward.is_empty(),
            Parent => self.tab().cwd.parent().is_some(),
            Quit => !self.busy && !self.desktop.working,
            _ => true,
        }) && (!self.busy || !matches!(action, Rename | Trash))
            && !(self.extensions.working.is_some()
                && matches!(
                    action,
                    Quit | Paste
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
                ))
            && !(self.in_archive() && Self::archive_writable_action(action))
    }

    pub(crate) fn command_title(&self) -> String {
        match &self.dialog {
            Some(Dialog::Commands(menu)) if menu.context == MenuContext::Trash => "КОРЗИНА".into(),
            Some(Dialog::Commands(menu)) if menu.context == MenuContext::Directory => {
                "ТЕКУЩАЯ ПАПКА".into()
            }
            Some(Dialog::Commands(menu)) if menu.context == MenuContext::Entries => {
                let count = self.tab().selection_count();
                if count > 1 {
                    format!("ВЫБРАНО: {count}")
                } else if self.tab().entry().is_some_and(|e| e.directory) {
                    "ПАПКА".into()
                } else {
                    "ФАЙЛ".into()
                }
            }
            _ => "ДЕЙСТВИЯ".into(),
        }
    }

    pub(super) fn open_commands(&mut self, anchor: Position, context: MenuContext) {
        self.dialog = Some(Dialog::Commands(CommandMenu {
            context,
            anchor,
            selected: 0,
            offset: 0,
            capacity: 1,
        }));
    }

    pub(super) fn choose_command(&mut self, index: usize) {
        if !matches!(self.dialog, Some(Dialog::Commands(_))) {
            return;
        }
        if let Some(item) = self.command_items().get(index)
            && item.enabled
        {
            let action = item.action;
            let archive_background = matches!(&self.dialog, Some(Dialog::Commands(menu)) if menu.context == MenuContext::Directory);
            let folder = (action == Action::FolderSize).then(|| self.folder_size_target(matches!(&self.dialog, Some(Dialog::Commands(menu)) if menu.context == MenuContext::Directory))).flatten();
            self.dialog = None;
            if action == Action::ExtensionCommands {
                self.open_extensions(true, archive_background);
                return;
            }
            if action == Action::Properties {
                self.open_details(archive_background, false);
                return;
            }
            if action == Action::ArchiveExtract {
                self.begin_extract(archive_background);
                return;
            }
            if let Some(path) = folder {
                self.open_folder_size(path);
                return;
            }
            self.action(action);
        }
    }

    pub(super) fn scroll_commands(&mut self, delta: isize) {
        let count = self.command_items().len();
        if let Some(Dialog::Commands(menu)) = &mut self.dialog {
            menu.offset = menu
                .offset
                .saturating_add_signed(delta)
                .min(count.saturating_sub(menu.capacity));
            menu.selected = menu.selected.clamp(
                menu.offset,
                (menu.offset + menu.capacity).min(count).saturating_sub(1),
            );
        }
    }

    pub(super) fn commands_key(&mut self, press: KeyPress) {
        if press.key == Key::Escape {
            self.dialog = None;
            return;
        }
        let items = self.command_items();
        let Some(Dialog::Commands(menu)) = &mut self.dialog else {
            return;
        };
        if press.key == Key::Enter {
            let index = menu.selected;
            self.choose_command(index);
            return;
        }
        if press.ctrl || press.alt {
            return;
        }
        let last = items.len().saturating_sub(1);
        menu.selected = match press.key {
            Key::Up => menu.selected.saturating_sub(1),
            Key::Down => (menu.selected + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::PageUp => menu.selected.saturating_sub(menu.capacity),
            Key::PageDown => (menu.selected + menu.capacity).min(last),
            _ => return,
        };
        if menu.selected < menu.offset {
            menu.offset = menu.selected;
        }
        if menu.selected >= menu.offset + menu.capacity {
            menu.offset = menu.selected + 1 - menu.capacity;
        }
    }
}

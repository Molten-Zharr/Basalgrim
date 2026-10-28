use super::*;
use crate::input::{Key, KeyPress};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuContext {
    All,
    Entries,
    Directory,
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
        let count = self.tab().selection_count();
        let selection = count > 0;
        let actions: &[Action] = match context {
            MenuContext::Entries => &[Open, Copy, Cut, Rename, Trash],
            MenuContext::Directory => &[
                NewFile, NewFolder, Paste, Sort, Grid, Hidden, Refresh, Search, SelectAll,
            ],
            MenuContext::All => &[
                Open,
                Mark,
                SelectAll,
                Copy,
                Cut,
                Paste,
                Rename,
                Trash,
                NewFolder,
                NewFile,
                Operations,
                CancelOperation,
                NewTab,
                CloseTab,
                NextTab,
                NextPane,
                Path,
                Search,
                Refresh,
                Grid,
                Dual,
                Preview,
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
            ],
        };
        actions
            .iter()
            .copied()
            .map(|action| {
                let enabled = match action {
                    Open | Rename if context == MenuContext::Entries && count != 1 => false,
                    Rename if context == MenuContext::Entries => {
                        self.tab().entry().is_some_and(|entry| {
                            entry.path.file_name().and_then(|n| n.to_str()).is_some()
                        })
                    }
                    SelectAll => !self.tab().visible.is_empty(),
                    Open | Mark | Copy | Cut | Rename | Trash => selection,
                    Paste => {
                        !self.busy && self.clipboard.as_ref().is_some_and(|c| !c.paths.is_empty())
                    }
                    NewFolder | NewFile => !self.busy,
                    CancelOperation => self.busy,
                    CloseTab | NextTab => self.panes[self.active].tabs.len() > 1,
                    NextPane => self.settings.dual,
                    Back => !self.tab().back.is_empty(),
                    Forward => !self.tab().forward.is_empty(),
                    Parent => self.tab().cwd.parent().is_some(),
                    Quit => !self.busy,
                    _ => true,
                } && (!self.busy || !matches!(action, Rename | Trash));
                CommandItem { action, enabled }
            })
            .filter(|item| context == MenuContext::All || item.enabled)
            .collect()
    }

    pub(crate) fn command_title(&self) -> String {
        match &self.dialog {
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
            self.dialog = None;
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

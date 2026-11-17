use super::*;
use crate::{
    columns::Column,
    grouping::Grouping,
    input::{Input, Key},
};
pub struct GroupMenu {
    pub token: u64,
    pub owner: PathOwner,
    pub request: u64,
    pub anchor: Position,
    pub selected: usize,
}
impl App {
    pub fn group_checkbox(&mut self, owner: PathOwner, request: u64, enabled: bool) {
        if !matches!(self.dialog, Some(Dialog::Commands(_)))
            || !owner.valid(self)
            || self.file_tab(owner.pane).request != request
            || self.settings.grouping.enabled != enabled
        {
            return;
        }
        let Some(index) = self
            .command_items()
            .iter()
            .position(|i| i.action == Action::Grouping && i.enabled)
        else {
            return;
        };
        if !enabled {
            self.choose_command(index);
            return;
        }
        self.dialog = None;
        let mut policy = self.settings.grouping;
        policy.enabled = false;
        self.set_grouping(policy);
    }
    pub fn set_grouping(&mut self, policy: Grouping) {
        self.finish_column_drag(true);
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.typeahead = None;
        self.settings.grouping = policy;
        for tab in self.panes.iter_mut().flat_map(|p| &mut p.tabs) {
            tab.grouping = policy;
            tab.rebuild(self.settings.hidden);
            tab.offset = 0;
            tab.follow_selection = false;
        }
        if let Some(trash) = &mut self.trash {
            trash.files.grouping = policy;
            trash.rebuild();
            trash.files.offset = 0;
            trash.files.follow_selection = false;
        }
        self.hits.clear();
        self.drop_regions.clear();
        self.preferences_changed();
    }
    pub fn open_grouping(&mut self, anchor: Position) {
        if self.dialog.is_some() {
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.dialog = Some(Dialog::Grouping(GroupMenu {
            token: self.token(),
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            anchor,
            selected: 0,
        }));
    }
    fn group_valid(&self, m: &GroupMenu) -> bool {
        m.owner.valid(self) && self.file_tab(m.owner.pane).request == m.request
    }
    pub fn group_control(&mut self, token: u64, row: usize) {
        let Some(Dialog::Grouping(m)) = &self.dialog else {
            return;
        };
        if m.token != token || !self.group_valid(m) {
            return;
        }
        let mut policy = self.settings.grouping;
        if row == 0 {
            policy.enabled = !policy.enabled;
        } else if !policy.enabled {
            return;
        } else if row == 1 {
            policy.descending = !policy.descending;
        } else if let Some(field) = Column::ALL.get(row - 2) {
            policy.field = *field;
            policy.descending = policy.date();
        } else {
            return;
        }
        self.set_grouping(policy);
        let token = self.token();
        if let Some(Dialog::Grouping(m)) = &mut self.dialog {
            m.token = token;
            m.selected = row;
        }
    }
    pub(super) fn grouping_input(&mut self, input: &Input) -> bool {
        if let Some(Dialog::Grouping(m)) = &self.dialog
            && !self.group_valid(m)
        {
            self.dialog = None;
            self.hits.clear();
            return true;
        }
        let Some(Dialog::Grouping(m)) = &mut self.dialog else {
            return false;
        };
        if let Input::Key(k) = input {
            let token = m.token;
            match k.key {
                Key::Escape => self.cancel_dialog(),
                Key::Up => m.selected = m.selected.saturating_sub(1),
                Key::Down => {
                    m.selected = (m.selected + 1).min(if self.settings.grouping.enabled {
                        11
                    } else {
                        0
                    })
                }
                Key::Enter | Key::Char(' ') => {
                    let row = m.selected;
                    self.group_control(token, row);
                }
                _ => {}
            }
            return true;
        }
        false
    }
}

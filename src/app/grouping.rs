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
        self.hits.clear();
        self.dialog = Some(Dialog::Grouping(GroupMenu {
            token: self.token(),
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            anchor,
            selected: self.settings.grouping.selected_row(),
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
            policy.enabled = false;
        } else if let Some(&field) = Column::ALL.get(row - 1) {
            if field != policy.field {
                policy.field = field;
                policy.descending = policy.date();
            }
            policy.enabled = true;
        } else if row == Grouping::ORDER_ROW && policy.enabled {
            policy.descending = !policy.descending;
        } else {
            return;
        }
        self.dialog = None;
        self.hits.clear();
        if policy != self.settings.grouping {
            self.set_grouping(policy);
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
            let last = Column::ALL.len() + usize::from(self.settings.grouping.enabled);
            match k.key {
                Key::Escape => self.cancel_dialog(),
                Key::Home => m.selected = 0,
                Key::End => m.selected = last,
                Key::Up => m.selected = m.selected.saturating_sub(1),
                Key::Down => m.selected = (m.selected + 1).min(last),
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

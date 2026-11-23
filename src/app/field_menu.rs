use super::*;
use crate::{
    columns::Column,
    grouping::Grouping,
    input::{Input, Key},
};
#[derive(Clone, Copy)]
enum Choice {
    Group(Grouping),
    Sort(fs::Sort),
}

pub struct FieldMenuRow {
    pub label: String,
    pub checked: Option<bool>,
    choice: Option<Choice>,
}

impl FieldMenuRow {
    fn option(label: impl Into<String>, active: bool, choice: Choice) -> Self {
        Self {
            label: label.into(),
            checked: Some(active),
            choice: Some(choice),
        }
    }

    pub fn enabled(&self) -> bool {
        self.choice.is_some()
    }
}

pub struct FieldMenu {
    pub token: u64,
    pub owner: PathOwner,
    pub request: u64,
    pub anchor: Position,
    pub selected: usize,
    pub title: &'static str,
    pub rows: Vec<FieldMenuRow>,
}

fn group_rows(policy: Grouping) -> Vec<FieldMenuRow> {
    let mut rows = vec![FieldMenuRow::option(
        "Без группировки",
        !policy.enabled,
        Choice::Group(Grouping {
            enabled: false,
            ..policy
        }),
    )];
    for field in Column::ALL {
        let mut next = Grouping {
            enabled: true,
            field,
            ..policy
        };
        if field != policy.field {
            next.descending = next.date();
        }
        let label = if next.date() {
            format!("{} · месяц", field.label())
        } else {
            field.label().into()
        };
        rows.push(FieldMenuRow::option(
            label,
            next == policy,
            Choice::Group(next),
        ));
    }
    rows.push(FieldMenuRow {
        label: format!("Порядок: {}", policy.order_label()),
        checked: None,
        choice: policy.enabled.then_some(Choice::Group(Grouping {
            descending: !policy.descending,
            ..policy
        })),
    });
    rows
}

fn sort_rows(sort: fs::Sort, trash: bool) -> Vec<FieldMenuRow> {
    let (current, descending) = sort.field();
    let fields: &[Column] = if trash {
        &[Column::Name, Column::Modified]
    } else {
        &Column::ALL
    };
    let mut rows: Vec<_> = fields
        .iter()
        .map(|&field| {
            let label = field.sort_label(trash);
            FieldMenuRow::option(
                label,
                field == current,
                Choice::Sort(fs::Sort::Field(field, descending)),
            )
        })
        .collect();
    for (reverse, label) in [(false, "По возрастанию ↑"), (true, "По убыванию ↓")]
    {
        rows.push(FieldMenuRow::option(
            label,
            reverse == descending,
            Choice::Sort(fs::Sort::Field(current, reverse)),
        ));
    }
    rows
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
    pub(super) fn open_field_menu(&mut self, action: Action, anchor: Position) {
        if self.dialog.is_some() {
            return;
        }
        let rows = match action {
            Action::Grouping => group_rows(self.settings.grouping),
            Action::Sort => sort_rows(self.tab().sort, self.in_trash()),
            _ => return,
        };
        let selected = rows
            .iter()
            .position(|row| row.checked == Some(true))
            .unwrap_or(0);
        self.finish_column_drag(true);
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.hits.clear();
        self.dialog = Some(Dialog::FieldMenu(FieldMenu {
            token: self.token(),
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            anchor,
            selected,
            title: action.name(),
            rows,
        }));
    }
    fn field_menu_valid(&self, m: &FieldMenu) -> bool {
        m.owner.pane == self.active && m.owner.valid(self) && self.tab().request == m.request
    }
    pub fn choose_field_row(&mut self, token: u64, row: usize) {
        let Some(Dialog::FieldMenu(m)) = &self.dialog else {
            return;
        };
        if m.token != token || !self.field_menu_valid(m) {
            return;
        }
        let Some(choice) = m.rows.get(row).and_then(|r| r.choice) else {
            return;
        };
        self.dialog = None;
        self.hits.clear();
        match choice {
            Choice::Group(policy) if policy != self.settings.grouping => self.set_grouping(policy),
            Choice::Sort(sort) if sort.field() != self.tab().sort.field() => self.apply_sort(sort),
            _ => {}
        }
    }
    pub(super) fn field_menu_input(&mut self, input: &Input) -> bool {
        if let Some(Dialog::FieldMenu(m)) = &self.dialog
            && !self.field_menu_valid(m)
        {
            self.dialog = None;
            self.hits.clear();
            return true;
        }
        let Some(Dialog::FieldMenu(m)) = &mut self.dialog else {
            return false;
        };
        if let Input::Key(k) = input {
            let token = m.token;
            let last = m.rows.iter().rposition(FieldMenuRow::enabled).unwrap_or(0);
            match k.key {
                Key::Escape => self.cancel_dialog(),
                Key::Home => m.selected = 0,
                Key::End => m.selected = last,
                Key::Up => m.selected = m.selected.saturating_sub(1),
                Key::Down => m.selected = (m.selected + 1).min(last),
                Key::Enter | Key::Char(' ') => {
                    let row = m.selected;
                    self.choose_field_row(token, row);
                }
                _ => {}
            }
            return true;
        }
        false
    }
}

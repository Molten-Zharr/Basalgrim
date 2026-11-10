use super::*;
use crate::{
    input::{Key, KeyPress},
    places::{Command, Config, Item, Section},
};

#[derive(Clone, Copy)]
pub enum PlaceControl {
    Open,
    Reveal,
    Remove,
    Rename,
    Add,
    Filter,
    Refresh,
    Up,
    Down,
}
pub struct PlacesView {
    pub section: Section,
    pub rows: Vec<Item>,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    pub filter: String,
    pub revision: u64,
    source_revision: u64,
}
impl Default for PlacesView {
    fn default() -> Self {
        Self {
            section: Section::Bookmarks,
            rows: vec![],
            selected: 0,
            offset: 0,
            page: 1,
            filter: String::new(),
            revision: 0,
            source_revision: u64::MAX,
        }
    }
}
pub(super) struct PendingOpen {
    id: u64,
    owner: PathOwner,
    request: u64,
    path: PathBuf,
    reveal: bool,
    from_dialog: bool,
}
impl App {
    pub fn configure_places(&mut self, config: Config) {
        self.library.attach(config);
    }
    pub fn places_editing(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::PlaceFilter | EditKind::PlaceLabel { .. },
                ..
            })
        )
    }
    pub(super) fn rebuild_places(&mut self) {
        let v = &self.places_view;
        let selected = v.rows.get(v.selected).map(|i| i.path.clone());
        let filter = v.filter.to_lowercase();
        let rows = self
            .library
            .snapshot
            .items(v.section)
            .iter()
            .filter(|i| {
                v.section != Section::Bookmarks || !i.desktop || !self.in_quick_access(&i.path)
            })
            .filter(|i| {
                filter.is_empty()
                    || i.label.to_lowercase().contains(&filter)
                    || i.path.to_string_lossy().to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        let v = &mut self.places_view;
        v.rows = rows;
        v.selected = selected
            .and_then(|p| v.rows.iter().position(|i| i.path == p))
            .unwrap_or(v.selected)
            .min(v.rows.len().saturating_sub(1));
        v.offset = v.offset.min(v.rows.len().saturating_sub(1));
        v.source_revision = self.library.revision;
        v.revision += 1;
    }
    pub fn open_places(&mut self, section: Section) {
        self.place_open = None;
        self.cancel_file_drag();
        self.finish_marquee(true);
        if self.places_view.section != section {
            self.places_view = PlacesView {
                section,
                revision: self.places_view.revision + 1,
                ..Default::default()
            };
        }
        self.rebuild_places();
        self.dialog = Some(Dialog::Places);
        if self.library.pending == 0 {
            self.library.send(Command::Refresh);
        }
    }
    pub(crate) fn in_quick_access(&self, path: &Path) -> bool {
        self.places.iter().any(|p| p.path == path)
    }
    pub(crate) fn can_bookmark(&self) -> bool {
        !self.busy
            && self.library.writable()
            && !self.in_trash()
            && !self.in_archive()
            && !self.tab().loading
            && self.tab().error.is_none()
            && !self.in_quick_access(&self.tab().cwd)
    }
    pub(super) fn can_favorite(&self, add: bool) -> bool {
        if self.busy
            || !self.library.writable()
            || self.in_trash()
            || self.in_archive()
            || self.tab().loading
        {
            return false;
        }
        let paths = self.tab().targets();
        !paths.is_empty()
            && (paths
                .iter()
                .all(|p| self.library.snapshot.favorites.iter().any(|i| &i.path == p))
                != add)
    }
    pub(super) fn bookmark_current(&mut self) {
        if self.in_quick_access(&self.tab().cwd) {
            self.message("Папка уже в быстром доступе", false);
            return;
        }
        if self.can_bookmark() {
            self.library.send(Command::Bookmark(self.tab().cwd.clone()));
        }
    }
    pub(super) fn favorite_selection(&mut self, add: bool) {
        if self.can_favorite(add) {
            self.library.send(Command::Favorite {
                paths: self.tab().targets(),
                add,
            });
        }
    }
    pub fn place_open(&mut self, path: PathBuf, reveal: bool) {
        let id = self.token();
        self.place_open = Some(PendingOpen {
            id,
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            path: path.clone(),
            reveal,
            from_dialog: matches!(self.dialog, Some(Dialog::Places)),
        });
        if !self.library.send(Command::Resolve { id, path }) {
            self.place_open = None;
        }
    }
    pub(super) fn poll_places(&mut self) {
        let old_error = self.library.error.clone();
        let results = self.library.poll();
        if self.library.error != old_error
            && let Some(error) = &self.library.error
        {
            self.message(format!("Места: {error}"), true);
        }
        if self.places_view.source_revision != self.library.revision {
            self.rebuild_places();
        }
        for (id, result) in results {
            if self.place_open.as_ref().is_none_or(|p| p.id != id) {
                continue;
            }
            let open = self.place_open.take().unwrap();
            if !open.owner.valid(self)
                || open.owner.pane != self.active
                || open.request != self.tab().request
                || if open.from_dialog {
                    !matches!(self.dialog, Some(Dialog::Places))
                } else {
                    self.dialog.is_some()
                }
            {
                continue;
            }
            match result {
                Err(e) => self.message(e, true),
                Ok(directory) => {
                    self.dialog = None;
                    if open.reveal {
                        if let Some(parent) = open.path.parent() {
                            self.navigate(parent.into(), true);
                            self.tab_mut().filter.clear();
                            self.search_focus = Some((self.tab().request, open.path));
                        }
                    } else if directory || crate::archive::Format::of(&open.path).is_some() {
                        self.navigate(open.path, true);
                    } else {
                        self.worker.open(open.path);
                    }
                }
            }
        }
    }
    pub(super) fn place_select(&mut self, revision: u64, index: usize, open: bool) {
        if !matches!(self.dialog, Some(Dialog::Places))
            || revision != self.places_view.revision
            || index >= self.places_view.rows.len()
        {
            return;
        }
        self.places_view.selected = index;
        self.places_view.revision += 1;
        if open {
            self.place_control(self.places_view.revision, PlaceControl::Open);
        }
    }
    pub(super) fn place_move(&mut self, delta: isize) {
        self.places_view.selected = self
            .places_view
            .selected
            .saturating_add_signed(delta)
            .min(self.places_view.rows.len().saturating_sub(1));
        self.places_view.revision += 1;
    }
    pub(super) fn place_control(&mut self, revision: u64, control: PlaceControl) {
        if !matches!(self.dialog, Some(Dialog::Places)) || self.places_view.revision != revision {
            return;
        }
        let item = self
            .places_view
            .rows
            .get(self.places_view.selected)
            .cloned();
        let section = self.places_view.section;
        match control {
            PlaceControl::Up => self.place_move(-1),
            PlaceControl::Down => self.place_move(1),
            PlaceControl::Refresh => {
                self.library.send(Command::Refresh);
            }
            PlaceControl::Add if section == Section::Bookmarks => self.bookmark_current(),
            PlaceControl::Filter => {
                let text = self.places_view.filter.clone();
                let end = text.len();
                self.begin_edit(EditKind::PlaceFilter, text, 0..end);
            }
            PlaceControl::Open | PlaceControl::Reveal => {
                if let Some(item) = item {
                    self.place_open(item.path, matches!(control, PlaceControl::Reveal));
                }
            }
            PlaceControl::Remove if self.library.writable() => {
                if let Some(item) = item {
                    self.library.send(Command::Remove {
                        section,
                        path: item.path,
                    });
                }
            }
            PlaceControl::Rename if self.library.writable() => {
                if let Some(item) = item.filter(|i| !i.desktop && section != Section::Recent) {
                    let end = item.label.len();
                    self.begin_edit(
                        EditKind::PlaceLabel {
                            section,
                            path: item.path,
                        },
                        item.label,
                        0..end,
                    );
                }
            }
            _ => {}
        }
    }
    pub(super) fn submit_place_edit(&mut self, kind: EditKind, text: String) {
        match &kind {
            EditKind::PlaceFilter => {
                self.places_view.filter = text;
                self.rebuild_places();
            }
            EditKind::PlaceLabel { section, path } => {
                if let Err(error) = crate::places::valid_label(&text) {
                    self.dialog = Some(Dialog::Edit {
                        kind,
                        text,
                        error: Some(error.to_string()),
                    });
                    return;
                }
                self.library.send(Command::Rename {
                    section: *section,
                    path: path.clone(),
                    label: text,
                });
            }
            _ => unreachable!(),
        }
        self.dialog = Some(Dialog::Places);
    }
    pub(super) fn places_key(&mut self, press: KeyPress) {
        let v = &self.places_view;
        if press.ctrl || press.alt || press.shift {
            return;
        }
        let control = match press.key {
            Key::Escape => {
                self.cancel_dialog();
                return;
            }
            Key::Up => PlaceControl::Up,
            Key::Down => PlaceControl::Down,
            Key::PageUp => {
                self.place_move(-(v.page as isize));
                return;
            }
            Key::PageDown => {
                self.place_move(v.page as isize);
                return;
            }
            Key::Home => {
                self.place_move(isize::MIN);
                return;
            }
            Key::End => {
                self.place_move(isize::MAX);
                return;
            }
            Key::Tab | Key::Right | Key::Left => {
                let index = Section::ALL.iter().position(|s| *s == v.section).unwrap();
                self.open_places(
                    Section::ALL[(index + if press.key == Key::Left { 2 } else { 1 }) % 3],
                );
                return;
            }
            Key::Enter => PlaceControl::Open,
            Key::Delete => PlaceControl::Remove,
            Key::F(2) => PlaceControl::Rename,
            Key::F(5) => PlaceControl::Refresh,
            Key::Char('/') => PlaceControl::Filter,
            _ => return,
        };
        self.place_control(self.places_view.revision, control);
    }
}

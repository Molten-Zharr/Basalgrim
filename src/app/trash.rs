use super::*;
use crate::{
    operations::Cancellation,
    trash::{Catalog, Entry, Root},
};
use std::time::{Duration, Instant};

pub struct TrashView {
    pub catalog: Catalog,
    pub owner: PathOwner,
    pub files: Tab,
    by_path: BTreeMap<PathBuf, usize>,
    pub sort_name: bool,
    pub generation: u64,
    pub destination: PathBuf,
    roots: Option<Vec<Root>>,
    request: u64,
    cancellation: Cancellation,
    pending: Option<Result<Catalog, String>>,
    next_refresh: Instant,
    pending_empty: Option<u64>,
}
impl Drop for TrashView {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
impl std::ops::Deref for TrashView {
    type Target = Tab;
    fn deref(&self) -> &Tab {
        &self.files
    }
}
impl std::ops::DerefMut for TrashView {
    fn deref_mut(&mut self) -> &mut Tab {
        &mut self.files
    }
}
impl TrashView {
    pub fn entry(&self) -> Option<&Entry> {
        let path = &self.files.entry()?.path;
        self.by_path.get(path).map(|&i| &self.catalog.entries[i])
    }
    pub fn targets(&self) -> Vec<Entry> {
        self.selected_entries().cloned().collect()
    }
    pub fn selected_entries(&self) -> impl Iterator<Item = &Entry> {
        let single = self.entry().map(|e| e.data.as_path());
        let group = self.explicit_selection || !self.marked.is_empty();
        self.catalog.entries.iter().filter(move |e| {
            if group {
                self.marked.contains(&e.data)
            } else {
                single == Some(e.data.as_path())
            }
        })
    }
    pub fn deletion_targets(&self) -> Vec<crate::trash::Deletion> {
        let paths: BTreeSet<_> = self.files.targets().into_iter().collect();
        self.catalog
            .deletions
            .iter()
            .filter(|target| paths.contains(target.path()))
            .cloned()
            .collect()
    }
    pub fn can_delete(&self) -> bool {
        !self.loading && self.error.is_none() && self.files.selection_count() > 0
    }
    pub fn can_restore(&self) -> bool {
        let mut selected = self.selected_entries();
        !self.loading
            && self.error.is_none()
            && selected.next().is_some_and(|e| e.issue.is_none())
            && selected.all(|e| e.issue.is_none())
    }
    pub fn rebuild(&mut self) {
        // Keep the same file model and renderer as an ordinary directory.
        // Sorting entries never changes the association with guarded trash metadata.
        let focus = self.files.entry().map(|e| e.path.clone());
        let query = self.filter.to_lowercase();
        self.files.sort = if self.sort_name {
            fs::Sort::Name
        } else {
            fs::Sort::Modified
        };
        self.files.sort.sort(&mut self.files.entries);
        self.files.visible = self
            .files
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.name.to_lowercase().contains(&query)
                    || self
                        .by_path
                        .get(&e.path)
                        .and_then(|&i| self.catalog.entries[i].original.as_ref())
                        .is_some_and(|p| p.to_string_lossy().to_lowercase().contains(&query))
            })
            .map(|(i, _)| i)
            .collect();
        self.files.selected = focus
            .and_then(|path| {
                self.files
                    .visible
                    .iter()
                    .position(|&i| self.files.entries[i].path == path)
            })
            .unwrap_or(self.files.selected)
            .min(self.files.visible.len().saturating_sub(1));
        self.files.offset = self
            .files
            .offset
            .min(self.files.visible.len().saturating_sub(1));
    }
}
impl App {
    pub fn open_trash(&mut self, roots: Option<Vec<Root>>) {
        if self.dialog.is_some() {
            return;
        }
        self.finish_marquee(true);
        if let Some(view) = &self.trash {
            self.active = view.owner.pane;
            self.panes[self.active].active = view.owner.tab;
            return;
        }
        let owner = PathOwner::current(self, self.active);
        let mut files = Tab::new(owner.cwd.clone());
        files.loading = false;
        let roots = roots.or_else(|| {
            self.trash_home_only
                .then(|| dirs::data_dir().map(Root::home).into_iter().collect())
        });
        self.trash = Some(TrashView {
            catalog: Default::default(),
            by_path: BTreeMap::new(),
            owner,
            files,
            sort_name: false,
            generation: 0,
            destination: self.tab().cwd.clone(),
            roots,
            request: 0,
            cancellation: Default::default(),
            pending: None,
            next_refresh: Instant::now(),
            pending_empty: None,
        });
        self.refresh_trash();
        self.hits.clear();
    }
    pub fn close_trash(&mut self) {
        self.finish_marquee(true);
        self.trash = None;
        self.hits.clear();
    }
    pub fn trash_in(&self, pane: usize) -> bool {
        self.trash
            .as_ref()
            .is_some_and(|view| view.owner.pane == pane && view.owner.valid(self))
    }
    pub fn in_trash(&self) -> bool {
        self.trash_in(self.active)
    }
    pub fn refresh_trash(&mut self) {
        let id = self.token();
        let Some(view) = &mut self.trash else { return };
        view.cancellation.cancel();
        view.cancellation = Default::default();
        view.pending_empty = None;
        view.request = id;
        view.pending = None;
        view.files.refreshing = !view.files.entries.is_empty();
        view.loading = true;
        view.next_refresh = Instant::now() + Duration::from_secs(3);
        if !self
            .worker
            .read_trash(id, view.roots.clone(), view.cancellation.clone())
        {
            view.loading = false;
            view.error = Some("Чтение корзины недоступно".into());
        }
    }
    pub(super) fn tick_trash(&mut self) {
        if self.trash.as_ref().is_some_and(|view| {
            self.panes[view.owner.pane]
                .tabs
                .get(view.owner.tab)
                .is_none_or(|tab| tab.cwd != view.owner.cwd)
        }) {
            self.close_trash();
        }
        if self.dialog.is_none()
            && !self.busy
            && self
                .trash
                .as_ref()
                .is_some_and(|t| !t.loading && Instant::now() >= t.next_refresh)
        {
            self.refresh_trash();
        }
    }
    pub(super) fn accept_trash(&mut self, token: u64, result: Result<Catalog, String>) {
        if let Some(view) = &mut self.trash
            && view.request == token
        {
            view.pending = Some(result);
        }
    }
    pub(super) fn apply_trash(&mut self) {
        if self.dialog.is_some() || self.marquee.is_some() || self.file_drag.is_some() || self.busy
        {
            return;
        }
        let Some(view) = &mut self.trash else { return };
        let Some(result) = view.pending.take() else {
            return;
        };
        self.hits.clear();
        view.loading = false;
        view.files.refreshing = false;
        view.generation = view.request;
        view.next_refresh = Instant::now() + Duration::from_secs(3);
        match result {
            Err(error) => view.error = Some(error),
            Ok(catalog) => {
                let focus = view.entry().map(|e| e.data.clone());
                let top = view
                    .visible
                    .get(view.offset)
                    .map(|i| view.files.entries[*i].path.clone());
                view.files.entries = catalog
                    .entries
                    .iter()
                    .map(|e| fs::Entry {
                        path: e.data.clone(),
                        name: fs::clean(&e.name),
                        directory: e.directory,
                        symlink: e.symlink,
                        hidden: false,
                        size: e.size,
                        modified: chrono::NaiveDateTime::parse_from_str(
                            &e.deleted,
                            "%Y-%m-%d %H:%M:%S",
                        )
                        .ok()
                        .and_then(|time| time.and_local_timezone(chrono::Local).single())
                        .map(std::time::SystemTime::from),
                    })
                    .collect();
                view.files.visible.clear();
                view.files.request = view.generation;
                view.by_path = catalog
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (e.data.clone(), i))
                    .collect();
                view.catalog = catalog;
                view.error = None;
                view.files.marked.retain(|p| view.by_path.contains_key(p));
                view.rebuild();
                if let Some(i) = view
                    .visible
                    .iter()
                    .position(|i| Some(&view.files.entries[*i].path) == focus.as_ref())
                {
                    view.selected = i;
                }
                if !view.follow_selection
                    && let Some(i) = view
                        .visible
                        .iter()
                        .position(|i| Some(&view.files.entries[*i].path) == top.as_ref())
                {
                    view.offset = i.min(view.visible.len().saturating_sub(view.page_size));
                }
            }
        }
        let empty = view.pending_empty.take() == Some(view.generation) && view.error.is_none();
        if empty && self.in_trash() {
            self.confirm_delete_trash(true);
        }
    }
    pub(super) fn confirm_restore_trash(&mut self, here: bool) {
        if self.busy {
            return;
        }
        let Some(view) = &self.trash else { return };
        if !view.can_restore() {
            return;
        }
        self.dialog = Some(Dialog::RestoreTrash {
            entries: view.targets(),
            destination: here.then(|| view.destination.clone()),
        });
        self.hits.clear();
    }
    pub(super) fn confirm_delete_trash(&mut self, empty: bool) {
        if self.busy || !self.in_trash() {
            return;
        }
        let view = self.trash.as_mut().unwrap();
        view.pending_empty = None;
        if view.loading || view.error.is_some() {
            return;
        }
        let targets = if empty {
            view.catalog.deletions.clone()
        } else {
            view.deletion_targets()
        };
        if targets.is_empty() {
            return;
        }
        self.dialog = Some(Dialog::DeleteTrash { targets, empty });
        self.hits.clear();
    }
    pub(super) fn trash_action(&mut self, action: Action) -> bool {
        if action == Action::TrashView {
            return true;
        }
        match action {
            Action::RestoreTrash | Action::Open => self.confirm_restore_trash(false),
            Action::RestoreTrashHere => self.confirm_restore_trash(true),
            Action::Trash | Action::DeleteTrash => self.confirm_delete_trash(false),
            Action::EmptyTrash => {
                if self.busy {
                    return true;
                }
                self.refresh_trash();
                let view = self.trash.as_mut().unwrap();
                view.pending_empty = Some(view.request);
                self.message("Чтение корзины перед очисткой…", false);
            }
            Action::CopyPath => self.request_text_copy("trash:///".into()),
            Action::Refresh => self.refresh_trash(),
            Action::Sort => {
                let view = self.trash.as_mut().unwrap();
                view.sort_name = !view.sort_name;
                view.follow_selection = true;
                view.rebuild();
            }
            Action::Search => {
                let original = self.tab().filter.clone();
                self.dialog = Some(Dialog::Edit {
                    kind: EditKind::TrashSearch {
                        original: original.clone(),
                    },
                    text: original,
                    error: None,
                });
            }
            Action::Back | Action::Parent => self.close_trash(),
            Action::Copy
            | Action::Cut
            | Action::Paste
            | Action::Rename
            | Action::Duplicate
            | Action::CopyToOtherPane
            | Action::MoveToOtherPane
            | Action::NewFile
            | Action::NewFolder
            | Action::Forward => {}
            // Navigation, presentation and application controls remain shared.
            _ => return false,
        }
        true
    }
}

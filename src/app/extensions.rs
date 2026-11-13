use super::*;
use crate::{
    extensions::{self, Catalog, Context, Job, Selection, Service, Store},
    input::{Key, KeyPress},
    operations::Cancellation,
};

#[derive(Clone, Copy)]
pub enum ExtensionControl {
    Commands,
    Installed,
    Row(usize),
    Run,
    Toggle,
    Remove,
    Install,
    Refresh,
    Stop,
    Move(isize),
    Detail(isize),
}
#[derive(Default)]
pub struct ExtensionsState {
    service: Option<Service>,
    pub catalog: Catalog,
    pub working: Option<(u64, Cancellation)>,
    pub running_command: bool,
    pub status: String,
    pub revision: u64,
    location: Option<PathOwner>,
}
pub struct ExtensionRow {
    pub plugin: usize,
    pub command: Option<usize>,
    pub label: String,
}
pub struct ExtensionsView {
    pub token: u64,
    pub revision: u64,
    pub commands: bool,
    pub rows: Vec<ExtensionRow>,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    pub detail_offset: usize,
    pub detail_area: Rect,
    owner: PathOwner,
    request: u64,
    context: Option<Context>,
    from_settings: bool,
}
impl App {
    pub fn configure_extensions(&mut self, store: Store) {
        self.extensions.service = Some(Service::new(store));
        self.extension_job(Job::Reload);
    }
    pub fn extension_context(&self, background: bool) -> Option<Context> {
        if self.in_archive()
            || self.in_trash()
            || self.tab().loading
            || self.tab().error.is_some()
            || (!background && self.tab().selection_count() > 256)
        {
            return None;
        }
        let targets = if background {
            vec![]
        } else {
            self.tab().targets()
        };
        if targets.len() > 256 {
            return None;
        }
        let targets: BTreeSet<_> = targets.into_iter().collect();
        let mut selected: Vec<_> = self
            .tab()
            .entries
            .iter()
            .filter(|e| targets.contains(&e.path))
            .map(Selection::from)
            .collect();
        selected.sort_by(|a, b| a.path.cmp(&b.path));
        Some(Context {
            cwd: crate::journal::bytes(&self.tab().cwd),
            selected,
        })
    }
    pub fn has_extension_commands(&self, background: bool) -> bool {
        // Availability runs while drawing menus/palette. Do not scan the entire
        // directory (or clone a large marked group) on every frame.
        if !self
            .extensions
            .catalog
            .iter()
            .any(|p| p.enabled && !p.manifest.commands.is_empty())
            || self.in_archive()
            || self.in_trash()
            || self.tab().loading
            || self.tab().error.is_some()
            || (!background && self.tab().selection_count() > 256)
        {
            return false;
        }
        let paths = if background {
            vec![]
        } else {
            self.tab().targets()
        };
        let single = if paths.len() == 1 {
            self.tab()
                .entry()
                .filter(|e| e.path == paths[0])
                .or_else(|| self.tab().entries.iter().find(|e| e.path == paths[0]))
        } else {
            None
        };
        let c = Context {
            cwd: vec![],
            selected: paths
                .iter()
                .map(|p| Selection {
                    path: crate::journal::bytes(p),
                    directory: single.is_some_and(|e| e.directory),
                    symlink: false,
                    size: 0,
                    modified: None,
                })
                .collect(),
        };
        {
            self.extensions
                .catalog
                .iter()
                .any(|p| p.enabled && p.manifest.commands.iter().any(|cmd| c.matches(cmd)))
        }
    }
    pub fn open_extensions(&mut self, commands: bool, background: bool) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        let from_settings = matches!(self.dialog, Some(Dialog::Settings));
        let token = self.token();
        self.extension_view = Some(ExtensionsView {
            token,
            revision: 0,
            commands,
            rows: vec![],
            selected: 0,
            offset: 0,
            page: 1,
            detail_offset: 0,
            detail_area: Rect::default(),
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            context: self.extension_context(background),
            from_settings,
        });
        self.dialog = Some(Dialog::Extensions);
        self.rebuild_extensions();
    }
    fn rebuild_extensions(&mut self) {
        if let Some(v) = &mut self.extension_view {
            v.rows.clear();
            for (pi, p) in self.extensions.catalog.iter().enumerate() {
                if v.commands {
                    if !p.enabled {
                        continue;
                    }
                    for (ci, c) in p.manifest.commands.iter().enumerate() {
                        if v.context.as_ref().is_some_and(|context| context.matches(c)) {
                            v.rows.push(ExtensionRow {
                                plugin: pi,
                                command: Some(ci),
                                label: format!("{} · {}", c.name, p.manifest.name),
                            });
                        }
                    }
                } else {
                    v.rows.push(ExtensionRow {
                        plugin: pi,
                        command: None,
                        label: format!("{} {}", if p.enabled { "●" } else { "○" }, p.manifest.name),
                    });
                }
            }
            v.selected = v.selected.min(v.rows.len().saturating_sub(1));
            v.revision += 1;
        }
    }
    pub(super) fn close_extensions(&mut self) {
        if matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::ExtensionInstall,
                ..
            })
        ) {
            self.dialog = Some(Dialog::Extensions);
            self.editor_menu = None;
            self.editor_drag = None;
            self.text_paste = None;
            self.text_paste_fallback = None;
            return;
        }
        let settings = self.extension_view.take().is_some_and(|v| v.from_settings);
        self.dialog = settings.then_some(Dialog::Settings);
    }
    pub(super) fn extension_job(&mut self, job: Job) {
        if self.extensions.working.is_some() {
            return;
        }
        let id = self.token();
        let cancel = Cancellation::default();
        let running_command = matches!(job, Job::Run { .. });
        if self
            .extensions
            .service
            .as_ref()
            .is_some_and(|s| s.send(id, job, cancel.clone()))
        {
            self.extensions.working = Some((id, cancel));
            self.extensions.running_command = running_command;
            self.extensions.status = "Выполняется…".into();
        } else {
            self.extensions.status = "Служба расширений недоступна".into();
        }
        if let Some(v) = &mut self.extension_view {
            v.revision += 1;
            v.detail_offset = 0;
        }
    }
    pub fn extension_control(
        &mut self,
        token: u64,
        revision: u64,
        control: ExtensionControl,
        double: bool,
    ) {
        if !matches!(self.dialog, Some(Dialog::Extensions))
            || !self
                .extension_view
                .as_ref()
                .is_some_and(|v| v.token == token && v.revision == revision)
        {
            return;
        }
        let v = self.extension_view.as_mut().unwrap();
        match control {
            ExtensionControl::Row(index) if index < v.rows.len() => {
                v.selected = index;
                v.detail_offset = 0;
                if !double || !v.commands {
                    return;
                }
            }
            ExtensionControl::Row(_) => return,
            ExtensionControl::Move(delta) => {
                v.selected = v
                    .selected
                    .saturating_add_signed(delta)
                    .min(v.rows.len().saturating_sub(1));
                v.detail_offset = 0;
                return;
            }
            ExtensionControl::Detail(delta) => {
                v.detail_offset = v.detail_offset.saturating_add_signed(delta).min(1000);
                return;
            }
            ExtensionControl::Commands | ExtensionControl::Installed => {
                v.commands = matches!(control, ExtensionControl::Commands);
                v.selected = 0;
                v.offset = 0;
                v.detail_offset = 0;
                self.extensions.status.clear();
                self.rebuild_extensions();
                return;
            }
            ExtensionControl::Stop => {
                if let Some((_, cancel)) = &self.extensions.working {
                    cancel.cancel();
                }
                return;
            }
            _ => {}
        }
        if self.extensions.working.is_some() {
            return;
        }
        match control {
            ExtensionControl::Refresh => {
                self.extension_job(Job::Reload);
                return;
            }
            ExtensionControl::Install => {
                let initial = self
                    .single_folder()
                    .unwrap_or_else(|| self.tab().cwd.clone())
                    .to_string_lossy()
                    .into_owned();
                self.begin_edit(
                    EditKind::ExtensionInstall,
                    initial.clone(),
                    0..initial.len(),
                );
                return;
            }
            _ => {}
        }
        let v = self.extension_view.as_ref().unwrap();
        let Some(row) = v.rows.get(v.selected) else {
            return;
        };
        let Some(plugin) = self.extensions.catalog.get(row.plugin).cloned() else {
            return;
        };
        let job = match control {
            ExtensionControl::Toggle if !v.commands => Job::Toggle(plugin),
            ExtensionControl::Remove if !v.commands => Job::Remove(plugin),
            ExtensionControl::Run | ExtensionControl::Row(_) if v.commands => {
                if self.busy
                    || self.devices.working.is_some()
                    || self.network.working.is_some()
                    || !v.owner.valid(self)
                    || v.owner.pane != self.active
                    || v.request != self.tab().request
                {
                    self.extensions.status =
                        "Папка изменилась или занята. Откройте список команд заново".into();
                    return;
                }
                let Some(context) = v.context.clone() else {
                    return;
                };
                if !context.selected.is_empty()
                    && self.tab().targets()
                        != context
                            .selected
                            .iter()
                            .map(|s| crate::journal::path(&s.path))
                            .collect::<Vec<_>>()
                {
                    self.extensions.status =
                        "Выбор изменился. Откройте список команд заново".into();
                    return;
                }
                let Some(index) = row.command else {
                    return;
                };
                let command = plugin.manifest.commands[index].id.clone();
                Job::Run {
                    plugin,
                    command,
                    context,
                }
            }
            _ => return,
        };
        self.extension_job(job);
    }
    pub(super) fn extension_key(&mut self, press: KeyPress) -> bool {
        if !matches!(self.dialog, Some(Dialog::Extensions)) {
            return false;
        }
        if press.key == Key::Escape {
            self.close_extensions();
            return true;
        }
        let Some(v) = &self.extension_view else {
            return true;
        };
        let control = match press.key {
            Key::Tab => {
                if v.commands {
                    ExtensionControl::Installed
                } else {
                    ExtensionControl::Commands
                }
            }
            Key::Up => ExtensionControl::Move(-1),
            Key::Down => ExtensionControl::Move(1),
            Key::PageUp => ExtensionControl::Detail(-3),
            Key::PageDown => ExtensionControl::Detail(3),
            Key::Enter => {
                if v.commands {
                    ExtensionControl::Run
                } else {
                    ExtensionControl::Toggle
                }
            }
            Key::F(5) => ExtensionControl::Refresh,
            Key::Insert => ExtensionControl::Install,
            Key::Delete => ExtensionControl::Remove,
            Key::F(4) => ExtensionControl::Stop,
            _ => return true,
        };
        self.extension_control(v.token, v.revision, control, false);
        true
    }
    pub(super) fn submit_extension_install(&mut self, text: String) {
        let source = PathBuf::from(&text);
        if !source.is_absolute() {
            self.dialog = Some(Dialog::Edit {
                kind: EditKind::ExtensionInstall,
                text,
                error: Some("Укажите полный путь к папке пакета".into()),
            });
            return;
        }
        self.dialog = Some(Dialog::Extensions);
        self.extension_job(Job::Install(source));
    }
    pub(super) fn poll_extensions(&mut self) {
        let events: Vec<_> = self
            .extensions
            .service
            .as_ref()
            .map(|s| s.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                extensions::Event::Catalog(catalog) => {
                    for p in self.extensions.catalog.iter() {
                        p.lease.cancel();
                    }
                    self.extensions.catalog = catalog.clone();
                    self.extensions.revision += 1;
                    self.worker.set_extensions(catalog);
                    self.preview_path = None;
                    self.rebuild_extensions();
                }
                extensions::Event::Done { id, result }
                    if self
                        .extensions
                        .working
                        .as_ref()
                        .is_some_and(|(expected, _)| *expected == id) =>
                {
                    let cancelled = self.extensions.working.take().unwrap().1.is_cancelled();
                    let command = std::mem::take(&mut self.extensions.running_command);
                    let failed = result.is_err();
                    self.extensions.status = if cancelled {
                        "Остановлено. Действия расширения не отменяются автоматически".into()
                    } else {
                        fs::clean(&result.unwrap_or_else(|e| format!("Ошибка: {e}")))
                    };
                    if let Some(v) = &mut self.extension_view {
                        v.revision += 1;
                        v.detail_offset = 0;
                    }
                    if command && !matches!(self.dialog, Some(Dialog::Extensions)) {
                        self.message(self.extensions.status.clone(), failed && !cancelled);
                    }
                }
                extensions::Event::HookError(error) if self.extensions.working.is_none() => {
                    self.extensions.status = fs::clean(&error);
                }
                _ => {}
            }
        }
        let location = PathOwner::current(self, self.active);
        if self.extensions.location.as_ref() != Some(&location)
            && !self.tab().loading
            && self.tab().error.is_none()
        {
            self.extensions.location = Some(location);
            self.extension_event("directory_changed", None);
        }
    }
    pub(super) fn extension_event(&self, hook: &'static str, outcome: Option<String>) {
        if let (Some(service), Some(context)) =
            (&self.extensions.service, self.extension_context(true))
        {
            service.notify(hook, context, outcome);
        }
    }
}

use super::*;
use crate::{
    devices::{Backend, Device, Kind, Plan},
    input::{Key, KeyPress},
};

#[derive(Clone, Copy)]
pub enum DeviceControl {
    Open,
    Mount,
    Unmount,
    Eject,
    Confirm,
    Back,
    Refresh,
    Up,
    Down,
}
#[derive(Default)]
pub struct DevicesView {
    pub rows: Vec<Device>,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    pub revision: u64,
    pub confirm: Option<Plan>,
    source_revision: u64,
}
pub(super) struct PendingOpen {
    id: u64,
    owner: PathOwner,
    request: u64,
    from_dialog: bool,
}
impl App {
    pub fn configure_devices(&mut self, backend: Box<dyn Backend>) {
        self.devices.attach(backend);
    }
    fn rebuild_devices(&mut self) {
        let v = &mut self.devices_view;
        let selected = v.rows.get(v.selected).map(|d| d.key.clone());
        v.rows = self.devices.snapshot.devices();
        v.selected = selected
            .and_then(|k| v.rows.iter().position(|d| d.key == k))
            .unwrap_or(v.selected)
            .min(v.rows.len().saturating_sub(1));
        v.offset = v.offset.min(v.rows.len().saturating_sub(1));
        v.source_revision = self.devices.revision;
        v.revision += 1;
    }
    pub fn open_devices(&mut self) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.device_open = None;
        self.devices_view.confirm = None;
        self.rebuild_devices();
        self.dialog = Some(Dialog::Devices);
        self.devices.refresh();
    }
    pub(super) fn disconnect_tabs(&mut self, roots: &[PathBuf]) {
        if roots.is_empty() {
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        for p in 0..self.panes.len() {
            for i in 0..self.panes[p].tabs.len() {
                let tab = &self.panes[p].tabs[i];
                if let Some(root) = roots
                    .iter()
                    .filter(|r| tab.cwd.starts_with(r))
                    .min_by_key(|r| r.components().count())
                {
                    let mut outside = root.parent().unwrap_or(Path::new("/")).to_path_buf();
                    while roots.iter().any(|r| outside.starts_with(r)) && outside != Path::new("/")
                    {
                        outside = outside.parent().unwrap_or(Path::new("/")).into();
                    }
                    self.navigate_tab(p, i, outside, true);
                }
            }
        }
        self.update_preview();
    }
    pub(super) fn poll_devices(&mut self) {
        let events = self.devices.poll();
        let departed = std::mem::take(&mut self.devices.departed);
        self.disconnect_tabs(&departed);
        if self.devices_view.source_revision != self.devices.revision {
            self.rebuild_devices();
        }
        for event in events {
            self.disconnect_tabs(&event.outcome.disconnected);
            if let Some(error) = event.outcome.error {
                let partial = if event.outcome.disconnected.is_empty() {
                    ""
                } else {
                    "Часть разделов уже отключена. "
                };
                self.message(format!("{partial}{error}"), true);
            } else {
                self.message(
                    match event.kind {
                        Kind::Open | Kind::Mount => "Накопитель подключён",
                        Kind::Unmount => "Раздел отключён",
                        Kind::Eject => "Накопитель извлечён",
                    },
                    false,
                );
            }
            if self.device_open.as_ref().is_some_and(|o| o.id == event.id) {
                let open = self.device_open.take().unwrap();
                let dialog_matches = if open.from_dialog {
                    matches!(self.dialog, Some(Dialog::Devices))
                } else {
                    self.dialog.is_none()
                };
                if open.owner.valid(self)
                    && open.owner.pane == self.active
                    && open.request == self.tab().request
                    && dialog_matches
                    && let Some(path) = event.outcome.opened
                {
                    self.dialog = None;
                    self.navigate(path, true);
                }
            }
            self.devices_view.revision += 1;
        }
    }
    fn run_device(&mut self, plan: Plan) {
        if self.busy
            || self.extensions.working.is_some()
            || self.network.working.is_some()
            || self.devices.working.is_some()
            || !self.devices.ready
        {
            self.message("Дождитесь завершения текущей операции", true);
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.cancel_clipboard_read();
        let id = self.token();
        let opening = (plan.kind == Kind::Open).then(|| PendingOpen {
            id,
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            from_dialog: matches!(self.dialog, Some(Dialog::Devices)),
        });
        if self.devices.run(id, plan) {
            self.device_open = opening;
        } else {
            self.message("Накопители заняты; повторите действие", true);
        }
    }
    pub(super) fn open_device(&mut self, revision: u64, key: String) {
        if revision != self.devices.revision || self.dialog.is_some() {
            return;
        }
        match self.devices.snapshot.plan(&key, Kind::Open) {
            Ok(plan) => self.run_device(plan),
            Err(_) => {
                self.open_devices();
                if let Some(index) = self.devices_view.rows.iter().position(|d| d.key == key) {
                    self.devices_view.selected = index;
                }
            }
        }
    }
    pub(super) fn device_select(&mut self, revision: u64, index: usize, open: bool) {
        if !matches!(self.dialog, Some(Dialog::Devices))
            || self.devices_view.confirm.is_some()
            || revision != self.devices_view.revision
            || index >= self.devices_view.rows.len()
        {
            return;
        }
        self.devices_view.selected = index;
        self.devices_view.revision += 1;
        if open {
            self.device_control(self.devices_view.revision, DeviceControl::Open);
        }
    }
    pub(super) fn device_move(&mut self, delta: isize) {
        if self.devices_view.confirm.is_some() {
            return;
        }
        self.devices_view.selected = self
            .devices_view
            .selected
            .saturating_add_signed(delta)
            .min(self.devices_view.rows.len().saturating_sub(1));
        self.devices_view.revision += 1;
    }
    pub(super) fn device_control(&mut self, revision: u64, control: DeviceControl) {
        if !matches!(self.dialog, Some(Dialog::Devices)) || revision != self.devices_view.revision {
            return;
        }
        if self.devices_view.confirm.is_some() {
            match control {
                DeviceControl::Confirm => {
                    let plan = self.devices_view.confirm.take().unwrap();
                    self.devices_view.revision += 1;
                    self.run_device(plan);
                }
                DeviceControl::Back => {
                    self.devices_view.confirm = None;
                    self.devices_view.revision += 1;
                }
                _ => {}
            }
            return;
        }
        match control {
            DeviceControl::Refresh => self.devices.refresh(),
            DeviceControl::Up => self.device_move(-1),
            DeviceControl::Down => self.device_move(1),
            DeviceControl::Open
            | DeviceControl::Mount
            | DeviceControl::Unmount
            | DeviceControl::Eject => {
                let Some(device) = self.devices_view.rows.get(self.devices_view.selected) else {
                    return;
                };
                if self.busy
                    || self.network.working.is_some()
                    || self.devices.working.is_some()
                    || !self.devices.ready
                {
                    return;
                }
                let kind = match control {
                    DeviceControl::Open => Kind::Open,
                    DeviceControl::Mount => Kind::Mount,
                    DeviceControl::Unmount => Kind::Unmount,
                    _ => Kind::Eject,
                };
                match self.devices.snapshot.plan(&device.key, kind) {
                    Ok(plan) if kind == Kind::Eject => {
                        self.devices_view.confirm = Some(plan);
                        self.devices_view.revision += 1;
                    }
                    Ok(plan) => self.run_device(plan),
                    Err(e) => self.message(e.to_string(), true),
                }
            }
            _ => {}
        }
    }
    pub(super) fn devices_key(&mut self, press: KeyPress) {
        match self.settings.action_for(press) {
            Some(Action::Quit) => {
                self.action(Action::Quit);
                return;
            }
            Some(Action::Refresh) => {
                self.device_control(self.devices_view.revision, DeviceControl::Refresh);
                return;
            }
            _ => {}
        }
        if press.ctrl || press.alt || press.shift {
            return;
        }
        let control = match press.key {
            Key::Escape => {
                self.cancel_dialog();
                return;
            }
            Key::Enter if self.devices_view.confirm.is_some() => DeviceControl::Confirm,
            Key::Enter => DeviceControl::Open,
            Key::Up => DeviceControl::Up,
            Key::Down => DeviceControl::Down,
            Key::PageUp => {
                self.device_move(-(self.devices_view.page as isize));
                return;
            }
            Key::PageDown => {
                self.device_move(self.devices_view.page as isize);
                return;
            }
            Key::Home => {
                self.device_move(isize::MIN);
                return;
            }
            Key::End => {
                self.device_move(isize::MAX);
                return;
            }
            _ => return,
        };
        self.device_control(self.devices_view.revision, control);
    }
}

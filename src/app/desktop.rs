use super::*;
use crate::desktop::{Location, Runtime, service::Event};
use std::collections::VecDeque;
#[derive(Default)]
pub struct DesktopState {
    pub runtime: Option<Runtime>,
    pub status: String,
    pub working: bool,
    pub focus: bool,
    queued: VecDeque<Location>,
    pending: Option<(PathOwner, u64, Location)>,
}
impl App {
    pub fn attach_desktop(&mut self, runtime: Runtime) {
        self.desktop.status = runtime.status.clone();
        self.desktop.runtime = Some(runtime);
    }
    pub(super) fn desktop_install(&mut self, action: crate::desktop::InstallAction) {
        if self.desktop.working {
            return;
        }
        let result = crate::desktop::Config::current().and_then(|config| {
            anyhow::ensure!(
                self.desktop
                    .runtime
                    .as_ref()
                    .is_some_and(|r| r.install(config, action)),
                "Служба рабочего стола недоступна"
            );
            Ok(())
        });
        match result {
            Ok(()) => {
                self.desktop.working = true;
                self.desktop.status = "Применение…".into();
            }
            Err(e) => self.desktop.status = e.to_string(),
        }
    }
    pub(super) fn poll_desktop(&mut self) {
        if let Some(result) = self
            .desktop
            .runtime
            .as_ref()
            .and_then(|r| r.integrations.try_recv().ok())
        {
            self.desktop.working = false;
            self.desktop.status = match result {
                Ok(s) => s,
                Err(e) => format!("Ошибка: {e}"),
            };
        }
        if self.desktop.queued.is_empty()
            && self.desktop.pending.is_none()
            && let Some(event) = self
                .desktop
                .runtime
                .as_ref()
                .and_then(|r| r.events.try_recv().ok())
        {
            match event {
                Event::Batch(Ok(batch)) => {
                    self.desktop.queued = batch.locations.into();
                    self.desktop.focus = true;
                }
                Event::Batch(Err(e)) => self.message(e, true),
            }
        }
        if self.dialog.is_some()
            || self.busy
            || self.dragging_files()
            || self.external_drag_active()
            || self.marquee.is_some()
            || self.devices.working.is_some()
            || self.network.working.is_some()
            || self.terminal.focused
        {
            return;
        }
        if let Some((owner, request, location)) = self.desktop.pending.take() {
            if !owner.valid(self)
                || owner.pane != self.active
                || self.tab().request != request
                || !self.tab().filter.is_empty()
                || self.tab().explicit_selection
                || !self.tab().marked.is_empty()
                || self.tab().selected != 0
            {
                self.message("Внешний переход отменён: вкладка изменилась", false);
                return;
            }
            if self.tab().loading {
                self.desktop.pending = Some((owner, request, location));
                return;
            }
            if self.tab().error.is_some() {
                return;
            }
            if !location.selected.is_empty() {
                // Make requested hidden entries visible only in this fresh tab, without
                // changing the user's global hidden-file preference.
                let hidden = self.settings.hidden;
                let tab = self.tab_mut();
                tab.revealed.extend(location.selected.iter().cloned());
                tab.rebuild(hidden);
                tab.marked = location
                    .selected
                    .iter()
                    .filter(|p| tab.entries.iter().any(|e| &e.path == *p))
                    .cloned()
                    .collect();
                if tab.marked.len() != location.selected.len() {
                    self.message("Некоторые запрошенные файлы больше не существуют", true);
                    return;
                }
                let tab = self.tab_mut();
                tab.explicit_selection = true;
                tab.selected = tab
                    .visible
                    .iter()
                    .position(|&i| tab.marked.contains(&tab.entries[i].path))
                    .unwrap_or(0);
                tab.follow_selection = true;
                tab.reset_range();
            }
            if location.properties {
                self.open_details(location.selected.is_empty(), false);
            }
            self.update_preview();
            return;
        }
        if let Some(location) = self.desktop.queued.pop_front() {
            let pane = &mut self.panes[self.active];
            pane.tabs.push(Tab::new(location.directory.clone()));
            pane.active = pane.tabs.len() - 1;
            self.refresh();
            self.desktop.pending = Some((
                PathOwner::current(self, self.active),
                self.tab().request,
                location,
            ));
            self.desktop.focus = true;
        }
    }
}

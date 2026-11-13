use super::*;
use crate::{
    input::{Key, KeyPress},
    network::{Answer, Backend, Event, Location, Prompt, Request},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkField {
    Address,
    User,
    Password,
    Domain,
}
impl NetworkField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Address => "Адрес сервера",
            Self::User => "Пользователь",
            Self::Password => "Пароль",
            Self::Domain => "Домен",
        }
    }
}
#[derive(Clone, Copy)]
pub enum NetworkControl {
    New,
    Open,
    Disconnect,
    ConfirmDisconnect,
    Field(NetworkField),
    Submit,
    Anonymous,
    Choice(usize),
    MoreChoices,
    Scroll(isize),
    Row(usize),
    Scheme(usize),
}
#[derive(Default)]
pub struct NetworkView {
    pub uri: String,
    pub user: String,
    pub password: Zeroizing<String>,
    pub domain: String,
    pub prompt: Option<(u64, Prompt)>,
    pub confirm: Option<Location>,
    pub selected: usize,
    pub offset: usize,
    pub page: usize,
    pub scroll: usize,
    pub choices: usize,
    pub revision: u64,
    pub error: Option<String>,
    pending: Option<Pending>,
}
struct Pending {
    id: u64,
    owner: PathOwner,
    request: u64,
    disconnect: Option<PathBuf>,
}
impl App {
    pub(super) fn network_scroll(&mut self, delta: isize) {
        if self.network_view.prompt.is_some()
            || self.network_view.error.is_some()
            || self.network.error.is_some()
        {
            self.network_view.scroll = self
                .network_view
                .scroll
                .saturating_add_signed(delta)
                .min(16384);
        } else {
            self.network_view.selected = self
                .network_view
                .selected
                .saturating_add_signed(delta)
                .min(self.network.locations.len().saturating_sub(1));
        }
        self.network_view.revision += 1;
    }
    pub fn network_disconnecting(&self) -> bool {
        self.network_view
            .pending
            .as_ref()
            .is_some_and(|p| p.disconnect.is_some())
    }
    pub fn configure_network(
        &mut self,
        factory: impl FnOnce() -> Box<dyn Backend> + Send + 'static,
    ) {
        self.network.attach(factory);
    }
    pub fn network_dialog(&self) -> bool {
        matches!(
            self.dialog,
            Some(
                Dialog::Network
                    | Dialog::Edit {
                        kind: EditKind::Network(_),
                        ..
                    }
            )
        )
    }
    pub fn secret_text(&self) -> bool {
        matches!(
            self.dialog,
            Some(Dialog::Edit {
                kind: EditKind::Network(NetworkField::Password),
                ..
            })
        )
    }
    fn store_network_field(&mut self) {
        if let Some(Dialog::Edit {
            kind: EditKind::Network(field),
            text,
            ..
        }) = &mut self.dialog
        {
            let value = std::mem::take(text);
            match field {
                NetworkField::Address => self.network_view.uri = value,
                NetworkField::User => self.network_view.user = value,
                NetworkField::Domain => self.network_view.domain = value,
                NetworkField::Password => self.network_view.password = Zeroizing::new(value),
            }
        }
    }
    fn network_field(&mut self, field: NetworkField) {
        self.store_network_field();
        let text = match field {
            NetworkField::Address => self.network_view.uri.clone(),
            NetworkField::User => self.network_view.user.clone(),
            NetworkField::Password => std::mem::take(&mut *self.network_view.password),
            NetworkField::Domain => self.network_view.domain.clone(),
        };
        let len = text.len();
        self.begin_edit(EditKind::Network(field), text, 0..len);
        self.network_view.revision += 1;
    }
    pub fn open_network(&mut self) {
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.network_view.confirm = None;
        self.network_view.error = None;
        self.network_view.revision += 1;
        self.dialog = Some(Dialog::Network);
    }
    pub(super) fn cancel_network(&mut self) {
        self.store_network_field();
        self.network.cancel();
        self.network_view.password.zeroize();
        self.network_view.user.clear();
        self.network_view.domain.clear();
        self.network_view.prompt = None;
        self.network_view.pending = None;
        self.network_view.confirm = None;
        self.network_view.revision += 1;
        self.dialog = None;
        self.editor_menu = None;
        self.text_paste = None;
    }
    fn start_network(&mut self, request: Request) {
        if self.busy
            || self.extensions.working.is_some()
            || self.devices.working.is_some()
            || self.network.working.is_some()
        {
            self.network_view.error = Some("Дождитесь завершения текущей операции".into());
            return;
        }
        self.cancel_file_drag();
        self.finish_marquee(true);
        self.cancel_clipboard_read();
        let id = self.token();
        let disconnect = match &request {
            Request::Disconnect(l) => Some(l.root.clone()),
            _ => None,
        };
        let pending = Pending {
            id,
            owner: PathOwner::current(self, self.active),
            request: self.tab().request,
            disconnect,
        };
        match self.network.start(id, request) {
            Ok(()) => {
                self.network_view.pending = Some(pending);
                self.network_view.error = None;
                self.dialog = Some(Dialog::Network);
                self.network_view.revision += 1;
            }
            Err(e) => self.network_view.error = Some(e.to_string()),
        }
    }
    pub(super) fn connect_address(&mut self, text: String) {
        self.network_view.scroll = 0;
        self.network_view.uri = text;
        match crate::network::address(&self.network_view.uri) {
            Ok(uri) => {
                self.network_view.uri = uri.clone();
                self.start_network(Request::Connect(uri));
            }
            Err(e) => {
                self.network_field(NetworkField::Address);
                self.network_view.error = Some(e.to_string());
            }
        }
    }
    pub(super) fn submit_network(&mut self, anonymous: bool) {
        if self.network_view.prompt.is_some() {
            let valid = |s: &str| s.len() <= 4096 && !s.contains('\0');
            let current_valid = match &self.dialog {
                Some(Dialog::Edit { text, .. }) => valid(text),
                _ => true,
            };
            if !current_valid
                || !valid(&self.network_view.user)
                || !valid(&self.network_view.domain)
                || !valid(&self.network_view.password)
            {
                self.network_view.error =
                    Some("Поле входа превышает 4096 байт или содержит нулевой символ".into());
                return;
            }
        }
        self.store_network_field();
        if self.network_view.prompt.is_none() {
            self.connect_address(self.network_view.uri.clone());
            return;
        }
        let Some((prompt, Prompt::Password { .. })) = &self.network_view.prompt else {
            return;
        };
        let Some(pending) = &self.network_view.pending else {
            return;
        };
        self.network.reply(
            pending.id,
            *prompt,
            Answer::Credentials {
                user: self.network_view.user.clone(),
                password: std::mem::take(&mut self.network_view.password),
                domain: self.network_view.domain.clone(),
                anonymous,
            },
        );
        self.network_view.prompt = None;
        self.network_view.revision += 1;
        self.dialog = Some(Dialog::Network);
        self.text_paste = None;
    }
    pub(super) fn network_control(&mut self, revision: u64, control: NetworkControl) {
        if !self.network_dialog() || revision != self.network_view.revision {
            return;
        }
        if self.network_view.pending.as_ref().is_some_and(|p| {
            !p.owner.valid(self) || p.owner.pane != self.active || p.request != self.tab().request
        }) {
            self.cancel_network();
            return;
        }
        use NetworkControl::*;
        match control {
            Scroll(delta) => {
                self.network_view.scroll = self
                    .network_view
                    .scroll
                    .saturating_add_signed(delta)
                    .min(16384)
            }
            MoreChoices => {
                if let Some((_, Prompt::Question { choices, .. })) = &self.network_view.prompt {
                    self.network_view.choices =
                        (self.network_view.choices + 3) % choices.len().max(1);
                }
            }
            Choice(choice) => {
                if let Some((prompt, Prompt::Question { choices, .. })) = &self.network_view.prompt
                    && choice < choices.len()
                    && let Some(pending) = &self.network_view.pending
                {
                    self.network
                        .reply(pending.id, *prompt, Answer::Choice(choice));
                    self.network_view.prompt = None;
                }
            }
            Field(field) => self.network_field(field),
            Submit => self.submit_network(false),
            Anonymous => {
                if matches!(
                    self.network_view.prompt,
                    Some((
                        _,
                        Prompt::Password {
                            anonymous: true,
                            ..
                        }
                    ))
                ) {
                    self.submit_network(true);
                }
            }
            Scheme(index) => {
                if self.network.working.is_none()
                    && let Some(scheme) = ["sftp", "smb", "davs", "ftp"].get(index)
                {
                    self.network_view.uri = format!("{scheme}://");
                    self.network_view.error = None;
                    self.dialog = Some(Dialog::Network);
                    self.network_field(NetworkField::Address);
                    // A protocol button supplies a prefix: typing must append the host.
                    let text = &self.network_view.uri;
                    self.editor.reset(text, text.len()..text.len());
                }
            }
            New if self.network.working.is_none() => {
                if self.network_view.uri.is_empty() {
                    self.network_view.uri = "sftp://".into();
                }
                self.network_view.prompt = None;
                self.network_view.confirm = None;
                self.network_view.error = None;
                self.network_field(NetworkField::Address);
            }
            Row(index) if index < self.network.locations.len() => {
                self.network_view.selected = index
            }
            Open if self.network.working.is_none() => {
                if let Some(location) = self.network.locations.get(self.network_view.selected) {
                    self.connect_address(location.uri.clone());
                }
            }
            Disconnect if self.network.working.is_none() && !self.busy => {
                self.network_view.confirm = self
                    .network
                    .locations
                    .get(self.network_view.selected)
                    .cloned();
            }
            ConfirmDisconnect => {
                if let Some(location) = self.network_view.confirm.take() {
                    self.start_network(Request::Disconnect(location));
                }
            }
            _ => {}
        }
        self.network_view.revision += 1;
    }
    pub(super) fn poll_network(&mut self) {
        let old = self.network.locations.clone();
        let revision = self.network.revision;
        let events = self.network.poll();
        if revision != self.network.revision {
            let roots: Vec<_> = old
                .iter()
                .filter(|l| !self.network.locations.iter().any(|new| new.root == l.root))
                .map(|l| l.root.clone())
                .collect();
            self.disconnect_tabs(&roots);
            self.network_view.revision += 1;
            self.network_view.selected = self
                .network_view
                .selected
                .min(self.network.locations.len().saturating_sub(1));
        }
        for event in events {
            if matches!(event, Event::Prompt { .. })
                && self.network_view.pending.as_ref().is_some_and(|p| {
                    !p.owner.valid(self)
                        || p.owner.pane != self.active
                        || p.request != self.tab().request
                })
            {
                self.cancel_network();
                continue;
            }
            match event {
                Event::Prompt {
                    operation,
                    prompt,
                    value,
                } if self.network_dialog()
                    && self
                        .network_view
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.id == operation) =>
                {
                    self.network_view.scroll = 0;
                    self.network_view.choices = 0;
                    self.network_view.password.zeroize();
                    let field = match &value {
                        Prompt::Password {
                            user,
                            domain,
                            need_user,
                            need_domain,
                            ..
                        } => {
                            self.network_view.user = user.clone();
                            self.network_view.domain = domain.clone();
                            Some(if *need_user {
                                NetworkField::User
                            } else if *need_domain {
                                NetworkField::Domain
                            } else {
                                NetworkField::Password
                            })
                        }
                        _ => None,
                    };
                    self.network_view.prompt = Some((prompt, value));
                    self.network_view.revision += 1;
                    if let Some(field) = field {
                        self.dialog = Some(Dialog::Network);
                        self.network_field(field);
                    } else {
                        self.dialog = Some(Dialog::Network);
                    }
                }
                Event::Withdraw { operation, prompt }
                    if self
                        .network_view
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.id == operation)
                        && self
                            .network_view
                            .prompt
                            .as_ref()
                            .is_some_and(|(p, _)| *p == prompt) =>
                {
                    self.store_network_field();
                    self.network_view.password.zeroize();
                    self.network_view.prompt = None;
                    self.dialog = Some(Dialog::Network);
                    self.network_view.revision += 1;
                }
                Event::Done { operation, result } => {
                    let Some(pending) = self.network_view.pending.take() else {
                        continue;
                    };
                    if pending.id != operation {
                        self.network_view.pending = Some(pending);
                        continue;
                    }
                    let visible = self.network_dialog();
                    self.store_network_field();
                    self.network_view.password.zeroize();
                    self.network_view.prompt = None;
                    self.network_view.revision += 1;
                    if visible {
                        self.dialog = Some(Dialog::Network);
                    }
                    match result {
                        Ok(Some(location))
                            if visible
                                && pending.owner.valid(self)
                                && pending.owner.pane == self.active
                                && pending.request == self.tab().request =>
                        {
                            self.dialog = None;
                            self.navigate(location.path, true);
                            self.message("Сетевая папка подключена", false);
                        }
                        Ok(None) => {
                            if let Some(root) = pending.disconnect {
                                self.disconnect_tabs(&[root]);
                            }
                            self.message("Сетевая папка отключена", false);
                        }
                        Err(error) => {
                            self.network_view.error = Some(error.clone());
                            self.message(error, true);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    pub(super) fn network_key(&mut self, press: KeyPress) -> bool {
        if !self.network_dialog() {
            return false;
        }
        if press.key == Key::Tab && !press.ctrl && !press.alt && self.text_active() {
            let mut fields = vec![];
            if let Some((
                _,
                Prompt::Password {
                    need_user,
                    need_domain,
                    need_password,
                    ..
                },
            )) = &self.network_view.prompt
            {
                if *need_user {
                    fields.push(NetworkField::User);
                }
                if *need_domain {
                    fields.push(NetworkField::Domain);
                }
                if *need_password {
                    fields.push(NetworkField::Password);
                }
            }
            if let Some(Dialog::Edit {
                kind: EditKind::Network(field),
                ..
            }) = &self.dialog
                && !fields.is_empty()
            {
                let index = fields.iter().position(|f| f == field).unwrap_or(0);
                self.network_field(
                    fields[(index + if press.shift { fields.len() - 1 } else { 1 }) % fields.len()],
                );
            }
            return true;
        }
        if self.text_active() {
            return false;
        }
        if self.settings.action_for(press) == Some(Action::Quit) {
            self.action(Action::Quit);
            return true;
        }
        if press.ctrl || press.alt || press.shift {
            return true;
        }
        match press.key {
            Key::Escape => self.cancel_network(),
            Key::Enter if self.network_view.prompt.is_none() && self.network.working.is_none() => {
                self.network_control(
                    self.network_view.revision,
                    if self.network_view.confirm.is_some() {
                        NetworkControl::ConfirmDisconnect
                    } else {
                        NetworkControl::Open
                    },
                )
            }
            Key::Up | Key::Down => {
                let delta = if press.key == Key::Up { -1 } else { 1 };
                if self.network_view.prompt.is_some() {
                    self.network_view.scroll =
                        self.network_view.scroll.saturating_add_signed(delta);
                } else {
                    self.network_view.selected = self
                        .network_view
                        .selected
                        .saturating_add_signed(delta)
                        .min(self.network.locations.len().saturating_sub(1));
                }
                self.network_view.revision += 1;
            }
            _ => {}
        }
        true
    }
}

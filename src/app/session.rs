use super::*;
use crate::{
    options::Options,
    session::{Runtime, State},
};

impl App {
    pub fn startup(options: Options, settings: Settings, warning: Option<String>) -> Self {
        Self::startup_with_session(options, settings, warning, State::path())
    }
    /// Explicit storage injection keeps tests and previews independent of the user's session.
    pub fn startup_with_session(
        options: Options,
        settings: Settings,
        warning: Option<String>,
        path: Option<PathBuf>,
    ) -> Self {
        let mut app = Self::new(options.directory, settings);
        let mut warnings: Vec<_> = warning.into_iter().collect();
        if !options.no_session
            && let Some(path) = path
        {
            match State::load(&path) {
                Ok(state) => {
                    if app.settings.restore_session
                        && !options.explicit_directory
                        && let Some(state) = state
                    {
                        state.restore(&mut app);
                        app.message("Сессия восстановлена", false);
                    }
                    app.session = Some(Runtime::new(path, app.settings.restore_session));
                }
                Err(error) => {
                    let message = format!(
                        "Сессия не загружена; исходный файл сохранён, автосохранение отключено: {error:#}"
                    );
                    app.session_load_error = Some(message.clone());
                    warnings.push(message);
                }
            }
        }
        if !warnings.is_empty() {
            app.message(warnings.join(" · "), true);
        }
        app
    }
    pub fn persist_session(&mut self) {
        if self.session.is_none() {
            return;
        }
        let state = State::capture(self);
        if let Some(result) = self
            .session
            .as_mut()
            .unwrap()
            .update(state, self.settings.restore_session)
        {
            match result {
                Ok(()) => {
                    if let Some(error) = self.session_save_error.take()
                        && self.notice == fs::clean(&error)
                    {
                        self.message("Сессия сохранена", false);
                    }
                }
                Err(error) => {
                    let message = format!("Не удалось сохранить сессию: {error}");
                    self.session_save_error = Some(message.clone());
                    self.message(message, true);
                }
            }
        }
    }
    pub fn retry_session_save(&mut self) {
        if let Some(session) = &mut self.session {
            session.retry();
        }
        self.persist_session();
    }
}
impl Drop for App {
    fn drop(&mut self) {
        if self.settings.restore_session
            && let Some(session) = &mut self.session
        {
            session.retry();
        }
        self.persist_session();
    }
}

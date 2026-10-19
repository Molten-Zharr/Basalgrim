use super::*;

impl App {
    pub fn action(&mut self, action: Action) {
        if matches!(action, Action::Quit) {
            if self.busy {
                self.message("Перед выходом дождитесь завершения файловой операции", true);
            } else {
                self.quit = true;
            }
            return;
        }
        // Modal commands always act on the captured sources, never on a changing selection.
        if self.dialog.is_some() {
            return;
        }
        match action {
            Action::Back => {
                if let Some(path) = self.tab_mut().back.pop() {
                    let current = self.tab().cwd.clone();
                    self.tab_mut().forward.push(current);
                    self.navigate(path, false);
                }
            }
            Action::Forward => {
                if let Some(path) = self.tab_mut().forward.pop() {
                    let current = self.tab().cwd.clone();
                    self.tab_mut().back.push(current);
                    self.navigate(path, false);
                }
            }
            Action::Parent => {
                if let Some(path) = self.tab().cwd.parent() {
                    self.navigate(path.to_path_buf(), true);
                }
            }
            Action::Home => self.navigate(self.home().to_path_buf(), true),
            Action::Refresh => {
                self.preview_path = None;
                self.refresh();
            }
            Action::Search => {
                self.dialog = Some(Dialog::Edit {
                    kind: EditKind::Search {
                        original: self.tab().filter.clone(),
                    },
                    text: self.tab().filter.clone(),
                    error: None,
                })
            }
            Action::Path => {
                self.dialog = Some(Dialog::Edit {
                    kind: EditKind::Path,
                    text: self.tab().cwd.to_string_lossy().into(),
                    error: None,
                })
            }
            Action::Grid => {
                self.settings.grid = !self.settings.grid;
                self.tab_mut().offset = 0;
                self.preferences_changed();
            }
            Action::Dual => {
                self.settings.dual = !self.settings.dual;
                if !self.settings.dual {
                    self.active = 0;
                }
                self.preferences_changed();
            }
            Action::Preview => {
                self.settings.preview = !self.settings.preview;
                self.preferences_changed();
            }
            Action::Hidden => {
                self.settings.hidden = !self.settings.hidden;
                for tab in self.panes.iter_mut().flat_map(|p| &mut p.tabs) {
                    tab.rebuild(self.settings.hidden);
                }
                self.preferences_changed();
            }
            Action::Sort => {
                let hidden = self.settings.hidden;
                let tab = self.tab_mut();
                tab.sort = tab.sort.next();
                tab.rebuild(hidden);
            }
            Action::Copy | Action::Cut => {
                let paths = self.tab().targets();
                if !paths.is_empty() {
                    self.message(
                        format!(
                            "В буфере: {} · {}",
                            paths.len(),
                            if action == Action::Cut {
                                "перемещение"
                            } else {
                                "копирование"
                            }
                        ),
                        false,
                    );
                    self.clipboard = Some(Clipboard {
                        paths,
                        cut: action == Action::Cut,
                    });
                }
            }
            Action::Paste => {
                if let Some(clip) = &self.clipboard
                    && !clip.paths.is_empty()
                {
                    self.start_operation(Operation::Transfer {
                        sources: clip.paths.clone(),
                        destination: self.tab().cwd.clone(),
                        cut: clip.cut,
                    });
                }
            }
            Action::Rename => {
                if let Some(entry) = self.tab().entry() {
                    // A lossy display name must never silently replace a non-UTF-8 filename.
                    if let Some(name) = entry.path.file_name().and_then(|s| s.to_str()) {
                        self.dialog = Some(Dialog::Edit {
                            kind: EditKind::Rename(entry.path.clone()),
                            text: name.into(),
                            error: None,
                        });
                    } else {
                        self.message("Переименование не-UTF-8 имён пока недоступно", true);
                    }
                }
            }
            Action::NewFolder | Action::NewFile => {
                self.dialog = Some(Dialog::Edit {
                    kind: if action == Action::NewFolder {
                        EditKind::Folder
                    } else {
                        EditKind::File
                    },
                    text: String::new(),
                    error: None,
                })
            }
            Action::Trash => {
                let paths = self.tab().targets();
                if !paths.is_empty() {
                    self.dialog = Some(Dialog::Trash(paths));
                }
            }
            Action::SelectAll => {
                let tab = self.tab_mut();
                tab.marked = tab
                    .visible
                    .iter()
                    .map(|&i| tab.entries[i].path.clone())
                    .collect();
            }
            Action::NewTab => {
                let cwd = self.tab().cwd.clone();
                let pane = &mut self.panes[self.active];
                pane.tabs.push(Tab::new(cwd));
                pane.active = pane.tabs.len() - 1;
                self.refresh();
            }
            Action::CloseTab => {
                let pane = &mut self.panes[self.active];
                if pane.tabs.len() > 1 {
                    pane.tabs.remove(pane.active);
                    pane.active = pane.active.min(pane.tabs.len() - 1);
                }
            }
            Action::NextTab => {
                let pane = &mut self.panes[self.active];
                pane.active = (pane.active + 1) % pane.tabs.len();
            }
            Action::Help => {
                self.help_offset = 0;
                self.dialog = Some(Dialog::Help);
            }
            Action::Settings => self.dialog = Some(Dialog::Settings),
            Action::Open => self.open_selected(),
            Action::Mark => self.toggle_mark(),
            Action::NextPane => {
                if self.settings.dual {
                    self.active = 1 - self.active;
                }
            }
            Action::FontLarger | Action::FontSmaller => {
                use crate::preferences::{MAX_FONT_SIZE, MIN_FONT_SIZE};
                self.settings.appearance.font_size = self
                    .settings
                    .appearance
                    .font_size
                    .saturating_add_signed(if action == Action::FontLarger { 1 } else { -1 })
                    .clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                self.preferences_changed();
            }
            Action::Quit => unreachable!(),
        }
        self.update_preview();
    }
    pub fn cancel_dialog(&mut self) {
        if matches!(
            self.dialog,
            Some(Dialog::CaptureBinding { .. })
                | Some(Dialog::Edit {
                    kind: EditKind::Setting(_),
                    ..
                })
        ) {
            self.dialog = Some(Dialog::Settings);
            return;
        }
        if let Some(Dialog::Edit {
            kind: EditKind::Search { original },
            ..
        }) = self.dialog.take()
        {
            self.tab_mut().filter = original;
            let hidden = self.settings.hidden;
            self.tab_mut().rebuild(hidden);
            self.update_preview();
        }
    }
    pub fn confirm_dialog(&mut self) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        match dialog {
            Dialog::Settings | Dialog::CaptureBinding { .. } => {
                self.dialog = Some(Dialog::Settings)
            }
            Dialog::Help => {}
            Dialog::Trash(sources) => self.start_operation(Operation::Trash { sources }),
            Dialog::Edit { kind, text, .. } => {
                if let EditKind::Setting(edit) = kind {
                    match self.submit_setting(edit.clone(), &text) {
                        Ok(()) => self.dialog = Some(Dialog::Settings),
                        Err(error) => {
                            self.dialog = Some(Dialog::Edit {
                                kind: EditKind::Setting(edit),
                                text,
                                error: Some(error.to_string()),
                            })
                        }
                    }
                    return;
                }
                if !matches!(kind, EditKind::Search { .. } | EditKind::Path)
                    && let Err(e) = fs::validate_name(&text)
                {
                    self.dialog = Some(Dialog::Edit {
                        kind,
                        text,
                        error: Some(e.to_string()),
                    });
                    return;
                }
                match kind {
                    EditKind::Setting(_) => unreachable!(),
                    EditKind::Search { .. } => {}
                    EditKind::Path => {
                        let path = if text == "~" {
                            self.home().to_path_buf()
                        } else if let Some(rest) = text.strip_prefix("~/") {
                            self.home().join(rest)
                        } else {
                            let path = PathBuf::from(text);
                            if path.is_absolute() {
                                path
                            } else {
                                self.tab().cwd.join(path)
                            }
                        };
                        self.navigate(path, true);
                    }
                    EditKind::Folder | EditKind::File => self.start_operation(Operation::Create {
                        parent: self.tab().cwd.clone(),
                        name: text,
                        directory: matches!(kind, EditKind::Folder),
                    }),
                    EditKind::Rename(source) => {
                        self.start_operation(Operation::Rename { source, name: text })
                    }
                }
            }
        }
    }
}

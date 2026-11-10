use super::*;
use crate::archive::{Extraction, Format};

impl App {
    pub fn in_archive(&self) -> bool {
        self.tab().archive.is_some()
    }
    pub(super) fn archive_writable_action(action: Action) -> bool {
        matches!(
            action,
            Action::Copy
                | Action::Cut
                | Action::Paste
                | Action::Rename
                | Action::NewFile
                | Action::NewFolder
                | Action::Trash
                | Action::Duplicate
                | Action::CopyToOtherPane
                | Action::MoveToOtherPane
                | Action::ArchiveCreate
                | Action::FolderSize
        )
    }
    pub(super) fn can_extract(&self) -> bool {
        if self.busy || self.in_trash() || self.tab().loading || self.tab().error.is_some() {
            return false;
        }
        if self.in_archive() {
            return self.tab().selection_count() > 0;
        }
        let paths = self.tab().targets();
        paths.len() == 1
            && self.tab().entries.iter().any(|e| {
                e.path == paths[0] && !e.directory && !e.symlink && Format::of(&e.path).is_some()
            })
    }
    pub(super) fn begin_archive(&mut self) {
        if self.busy || self.in_archive() || self.in_trash() || self.tab().loading {
            return;
        }
        let sources = self.tab().targets();
        if sources.is_empty() {
            return;
        }
        let base = if sources.len() == 1 {
            sources[0].file_name().unwrap().to_os_string()
        } else {
            "Архив".into()
        };
        let length = base.to_string_lossy().len();
        let mut name = base;
        name.push(".zip");
        let destination = self.tab().cwd.join(name);
        let text = destination.to_string_lossy().into_owned();
        let start = text.len() - length - 4;
        self.begin_edit(
            EditKind::ArchiveCreate {
                sources,
                destination,
            },
            text,
            start..start + length,
        );
    }
    pub(super) fn begin_extract(&mut self, background: bool) {
        if self.busy || self.in_trash() || self.tab().loading {
            return;
        }
        let request = if let Some(catalog) = &self.tab().archive {
            let selected: Vec<_> = if background {
                vec![]
            } else {
                self.tab()
                    .targets()
                    .into_iter()
                    .filter_map(|p| p.strip_prefix(&catalog.source).ok().map(Path::to_path_buf))
                    .collect()
            };
            if !background && selected.is_empty() {
                return;
            }
            Extraction {
                source: catalog.source.clone(),
                stamp: Some(catalog.stamp.clone()),
                selected,
                base: self.tab().cwd.strip_prefix(&catalog.source).unwrap().into(),
            }
        } else {
            if !self.can_extract() {
                return;
            }
            Extraction::all(self.tab().targets().remove(0))
        };
        let destination = crate::archive::unpacked_path(&request.source);
        let text = destination.to_string_lossy().into_owned();
        let end = text.len();
        self.begin_edit(
            EditKind::ArchiveExtract {
                request,
                destination,
            },
            text,
            0..end,
        );
    }
    pub(super) fn archive_format(&mut self, format: Format) {
        let Some(Dialog::Edit {
            kind:
                EditKind::ArchiveCreate {
                    sources,
                    destination,
                },
            text,
            ..
        }) = &self.dialog
        else {
            return;
        };
        let sources = sources.clone();
        let destination = if text == &destination.to_string_lossy() {
            let path = crate::archive::unpacked_path(destination);
            let mut name = path.file_name().unwrap().to_os_string();
            name.push(format.suffix());
            path.with_file_name(name)
        } else {
            destination.clone()
        };
        let mut text = text.clone();
        let lower = text.to_ascii_lowercase();
        for suffix in [".tar.gz", ".tgz", ".tar", ".zip"] {
            if lower.ends_with(suffix) {
                text.truncate(text.len() - suffix.len());
                break;
            }
        }
        text.push_str(format.suffix());
        let end = text.len();
        self.begin_edit(
            EditKind::ArchiveCreate {
                sources,
                destination,
            },
            text,
            end..end,
        );
    }
    pub(super) fn submit_archive(&mut self, kind: EditKind, text: String) {
        let result = (|| -> anyhow::Result<Operation> {
            let original = match &kind {
                EditKind::ArchiveCreate { destination, .. }
                | EditKind::ArchiveExtract { destination, .. } => destination,
                _ => unreachable!(),
            };
            let destination = if text == original.to_string_lossy() {
                original.clone()
            } else {
                let p = if let Some(rest) = text.strip_prefix("~/") {
                    self.home().join(rest)
                } else {
                    PathBuf::from(&text)
                };
                if p.is_absolute() {
                    p
                } else {
                    original.parent().unwrap().join(p)
                }
            };
            anyhow::ensure!(
                !text.is_empty() && destination.file_name().is_some(),
                "Укажите имя результата и существующую папку"
            );
            match &kind {
                EditKind::ArchiveCreate { sources, .. } => {
                    let format = Format::of(&destination).ok_or_else(|| {
                        anyhow::anyhow!("Выберите ZIP, TAR или TAR.GZ кнопкой ниже")
                    })?;
                    Ok(Operation::ArchiveCreate {
                        sources: sources.clone(),
                        destination,
                        format,
                    })
                }
                EditKind::ArchiveExtract { request, .. } => Ok(Operation::ArchiveExtract {
                    request: request.clone(),
                    destination,
                }),
                _ => unreachable!(),
            }
        })();
        match result {
            Ok(operation) => self.start_operation(operation),
            Err(e) => {
                self.dialog = Some(Dialog::Edit {
                    kind,
                    text,
                    error: Some(e.to_string()),
                })
            }
        }
    }
}

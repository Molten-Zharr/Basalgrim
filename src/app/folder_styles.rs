use super::*;
use crate::folder_style::{COLORS, Change, Decoration, FolderId, FolderStyle, SYMBOLS};
use crate::input::{Key, KeyPress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleField {
    Icon,
    Color,
    Tag,
}
impl StyleField {
    pub const ALL: [Self; 3] = [Self::Icon, Self::Color, Self::Tag];
    pub fn index(self) -> usize {
        self as usize
    }
}
#[derive(Debug, Clone, Copy)]
pub enum StyleControl {
    Open,
    Field(StyleField),
    Symbol(usize),
    Color(usize),
    Reset,
    Save,
}

pub struct StyleEditor {
    pub fields: [String; 3],
    pub saving: bool,
    pub error: Option<String>,
    folder: FolderId,
    expected: Option<i64>,
}

impl App {
    pub(crate) fn folder_decoration(&self, entry: &crate::fs::Entry) -> Option<&Decoration> {
        self.library
            .snapshot
            .styles
            .get(entry.details.folder.as_ref()?)
    }
    fn style_field(&mut self, field: StyleField, preset: Option<String>) {
        let v = self.detail_view.as_mut().unwrap();
        let draft = v.style.as_mut().unwrap();
        if let Some(Dialog::Edit {
            kind: EditKind::FolderStyle(current),
            text,
            ..
        }) = &mut self.dialog
        {
            draft.fields[current.index()] = std::mem::take(text);
        }
        if let Some(value) = preset {
            draft.fields[field.index()] = value;
        }
        let text = draft.fields[field.index()].clone();
        let len = text.len();
        v.revision += 1;
        self.begin_edit(EditKind::FolderStyle(field), text, 0..len);
    }
    pub(super) fn style_control(&mut self, control: StyleControl) {
        use StyleControl::*;
        let Some(v) = self.detail_view.as_mut() else {
            return;
        };
        if matches!(control, Open) {
            let Some(folder) = v.properties.as_ref().and_then(|p| p.folder.clone()) else {
                return;
            };
            if v.loading || v.chooser || v.style.is_some() || !self.library.ready {
                return;
            }
            let old = self.library.snapshot.styles.get(&folder);
            let style = old.map(|d| d.style.clone()).unwrap_or_default();
            v.style = Some(StyleEditor {
                fields: [
                    style.icon,
                    style
                        .color
                        .map(crate::preferences::color_hex)
                        .unwrap_or_default(),
                    style.tag,
                ],
                saving: false,
                error: None,
                folder,
                expected: old.map(|d| d.revision),
            });
            self.style_field(StyleField::Icon, None);
            return;
        }
        let Some(draft) = &mut v.style else { return };
        if draft.saving {
            return;
        }
        match control {
            Open => {}
            Field(f) => self.style_field(f, None),
            Symbol(i) | Color(i) => {
                let (field, value) = if matches!(control, Symbol(_)) {
                    let Some(symbol) = SYMBOLS.get(i) else { return };
                    (StyleField::Icon, (*symbol).to_string())
                } else {
                    let Some(color) = COLORS.get(i) else { return };
                    (StyleField::Color, crate::preferences::color_hex(*color))
                };
                self.style_field(field, Some(value));
            }
            Reset => {
                draft.fields = Default::default();
                draft.error = None;
                self.dialog = Some(Dialog::Details);
                self.style_field(StyleField::Icon, None);
            }
            Save => {
                if let Some(Dialog::Edit {
                    kind: EditKind::FolderStyle(field),
                    text,
                    ..
                }) = &self.dialog
                {
                    draft.fields[field.index()] = text.clone();
                }
                let [icon, color, tag] = &draft.fields;
                let result = (|| -> anyhow::Result<_> {
                    let style = FolderStyle {
                        icon: icon.trim().into(),
                        color: (!color.trim().is_empty())
                            .then(|| crate::preferences::parse_color(color.trim()))
                            .transpose()?,
                        tag: tag.trim().into(),
                    };
                    style.validate()?;
                    Ok(style)
                })();
                let style = match result {
                    Ok(s) => s,
                    Err(e) => {
                        draft.error = Some(e.to_string());
                        return;
                    }
                };
                let change = Change {
                    path: v.source.path.clone(),
                    folder: draft.folder.clone(),
                    expected: draft.expected,
                    style,
                };
                if self.library.send(crate::places::Command::SetStyle {
                    token: v.token,
                    change,
                }) {
                    draft.saving = true;
                    draft.error = None;
                    v.revision += 1;
                    self.dialog = Some(Dialog::Details);
                    self.text_paste = None;
                    self.editor_menu = None;
                } else {
                    draft.error = self.library.error.clone();
                }
            }
        }
    }
    pub(super) fn finish_folder_style(&mut self, token: u64, result: Result<(), String>) {
        let Some(v) = self.detail_view.as_mut().filter(|v| v.token == token) else {
            if let Err(error) = result {
                self.message(error, true);
            }
            return;
        };
        let Some(draft) = &mut v.style else { return };
        match result {
            Ok(()) => {
                self.close_details();
                self.message("Оформление папки сохранено", false);
            }
            Err(error) => {
                draft.saving = false;
                draft.error = Some(error);
                self.style_field(StyleField::Icon, None);
            }
        }
    }
    pub(super) fn style_key(&mut self, press: KeyPress) -> bool {
        if self.editor_menu.is_some() {
            return false;
        }
        let v = self.detail_view.as_ref().unwrap();
        if press.key == Key::Escape {
            self.close_details();
            return true;
        }
        let control = match press.key {
            Key::Tab if !press.ctrl && !press.alt => {
                let Some(Dialog::Edit {
                    kind: EditKind::FolderStyle(f),
                    ..
                }) = &self.dialog
                else {
                    return true;
                };
                StyleControl::Field(
                    StyleField::ALL[(f.index() + if press.shift { 2 } else { 1 }) % 3],
                )
            }
            Key::Enter if !press.alt && !press.ctrl => StyleControl::Save,
            Key::Char('s') if press.ctrl && !press.alt => StyleControl::Save,
            _ => return v.style.as_ref().is_some_and(|s| s.saving),
        };
        self.detail_control(v.revision, DetailControl::Style(control));
        true
    }
}

//! Backend capabilities and icon geometry shared by both frontends.
use crate::fs::Entry;
use ratatui::layout::Rect;

#[cfg(any(feature = "guitui", feature = "tui"))]
pub mod images;
#[cfg(feature = "tui")]
pub mod terminal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconBackend {
    #[default]
    Symbols,
    Gui,
    Kitty,
    Sixel,
    Iterm2,
}
impl IconBackend {
    pub fn supports_images(self) -> bool {
        self != Self::Symbols
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconKind {
    Folder,
    File,
    Document,
    Code,
    Archive,
    Image,
    Audio,
    Video,
    Pdf,
    Application,
    Font,
    Link,
}
impl IconKind {
    pub const ALL: [Self; 12] = [
        Self::Folder,
        Self::File,
        Self::Document,
        Self::Code,
        Self::Archive,
        Self::Image,
        Self::Audio,
        Self::Video,
        Self::Pdf,
        Self::Application,
        Self::Font,
        Self::Link,
    ];

    pub fn for_entry(entry: &Entry) -> Self {
        if entry.directory {
            return Self::Folder;
        }
        if entry.symlink {
            return Self::Link;
        }
        let extension = entry
            .name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "txt" | "md" | "rst" | "log" | "doc" | "docx" | "odt" | "rtf" | "csv" | "xls"
            | "xlsx" | "ods" => Self::Document,
            "rs" | "py" | "c" | "h" | "cpp" | "hpp" | "js" | "ts" | "tsx" | "jsx" | "html"
            | "css" | "json" | "toml" | "yaml" | "yml" | "xml" | "lua" | "go" | "java"
            | "service" | "env" | "conf" => Self::Code,
            "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tgz" => Self::Archive,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" | "avif"
            | "heic" | "ico" => Self::Image,
            "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" => Self::Audio,
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" => Self::Video,
            "pdf" => Self::Pdf,
            "exe" | "appimage" | "deb" | "rpm" | "sh" | "bin" => Self::Application,
            "ttf" | "otf" | "woff" | "woff2" => Self::Font,
            _ => Self::File,
        }
    }

    pub fn color_role(self) -> &'static str {
        match self {
            Self::Folder | Self::Image | Self::Video => "folder_icon",
            Self::Archive | Self::Application | Self::Font => "selection_accent",
            Self::Code | Self::Audio | Self::Pdf => "help_heading",
            Self::File | Self::Document | Self::Link => "folder_path",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconSlot {
    pub kind: IconKind,
    pub area: Rect,
    pub foreground: [u8; 3],
    pub accent: [u8; 3],
    pub background: [u8; 3],
}

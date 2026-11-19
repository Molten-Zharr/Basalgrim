//! Personal folder decorations. The filesystem object itself is never modified.
use crate::{preview::ImagePreview, text_edit};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::Metadata,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FolderId(pub(crate) Vec<u8>);

impl FolderId {
    pub fn capture(path: &Path, meta: &Metadata) -> Option<Self> {
        if !meta.is_dir() {
            return None;
        }
        let mut bytes = Vec::from(meta.dev().to_le_bytes());
        bytes.extend(meta.ino().to_le_bytes());
        if let Some(birth) = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            bytes.push(1);
            bytes.extend(birth.as_secs().to_le_bytes());
            bytes.extend(birth.subsec_nanos().to_le_bytes());
        } else {
            bytes.push(0);
            bytes.extend(path.as_os_str().as_encoded_bytes());
        }
        Some(Self(bytes))
    }
    pub(crate) fn rebind_scan_path(&mut self, path: &Path) {
        if self.0.get(16) == Some(&0) {
            self.0.truncate(17);
            self.0.extend(path.as_os_str().as_encoded_bytes());
        }
    }
}

pub const SYMBOLS: [&str; 5] = ["♣", "◆", "♥", "♪", "▲"];
pub const COLORS: [[u8; 3]; 5] = [
    [235, 99, 78],
    [237, 174, 66],
    [94, 183, 126],
    [87, 160, 225],
    [177, 126, 213],
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderStyle {
    pub icon: String,
    pub color: Option<[u8; 3]>,
    pub tag: String,
}
impl FolderStyle {
    pub fn image_icon(&self) -> bool {
        Path::new(&self.icon).is_absolute()
    }
    pub fn symbol(&self) -> Option<&str> {
        (!self.icon.is_empty() && !self.image_icon()).then_some(&self.icon)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.tag.len() <= 128 && !self.tag.chars().any(char::is_control),
            "Тег: до 128 байт без управляющих символов"
        );
        if self.image_icon() {
            ensure!(
                self.icon.len() <= 4096
                    && !self.icon.chars().any(char::is_control)
                    && Path::new(&self.icon)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("png")
                            || ext.eq_ignore_ascii_case("svg")),
                "Укажите полный путь к файлу PNG или SVG"
            );
        } else if !self.icon.is_empty() {
            ensure!(
                self.icon.len() <= 64
                    && !self.icon.chars().any(char::is_control)
                    && !self.icon.trim().is_empty()
                    && text_edit::boundaries(&self.icon).len() == 2
                    && (1..=2).contains(&text_edit::width(&self.icon)),
                "Введите один символ или полный путь к PNG/SVG"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoration {
    pub revision: i64,
    pub style: FolderStyle,
    pub image: Option<Arc<ImagePreview>>,
}
#[derive(Debug, Clone)]
pub struct Change {
    pub path: PathBuf,
    pub folder: FolderId,
    pub expected: Option<i64>,
    pub style: FolderStyle,
}

#[cfg(any(feature = "guitui", feature = "tui"))]
pub(crate) fn import_icon(path: &Path) -> Result<ImagePreview> {
    use anyhow::Context;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .context("Не удалось открыть файл значка")?;
    ensure!(file.metadata()?.is_file(), "Для значка нужен обычный файл");
    let frame = crate::preview::read(path, &mut file, &[], true)
        .context("Нужен значок PNG или SVG")??
        .image
        .context("Пустое изображение")?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
        .context("Повреждённое изображение")?;
    let small = image::DynamicImage::ImageRgba8(image)
        .thumbnail(64, 64)
        .into_rgba8();
    ImagePreview::new(small.width(), small.height(), small.into_raw())
}
#[cfg(not(any(feature = "guitui", feature = "tui")))]
pub(crate) fn import_icon(_: &Path) -> Result<ImagePreview> {
    anyhow::bail!("Сборка без поддержки изображений")
}

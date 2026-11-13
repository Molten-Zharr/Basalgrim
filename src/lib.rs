pub mod app;
pub mod appearance;
pub mod batch_rename;
pub mod clipboard;
pub mod conflicts;
pub mod dnd;
pub mod folder_size;
pub mod fs;
pub mod icons;
pub mod input;
pub mod journal;
pub mod model;
pub mod operations;
pub mod options;
pub mod preferences;
pub mod preview;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod text_edit;
pub mod trash;
pub mod ui;
pub mod worker;

#[cfg(feature = "guitui")]
pub mod raster;

pub mod search;

pub mod archive;
pub mod thumbnails;

pub mod devices;
pub mod places;

pub mod details;
pub mod network;

pub mod desktop;

pub mod extensions;
pub mod privileged;

#[cfg(feature = "tui")]
pub mod tui_input;

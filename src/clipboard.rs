//! Text clipboard requests are separate from the file copy/cut clipboard.
pub mod files;
#[cfg(feature = "file-clipboard")]
pub mod native;
pub mod service;
#[cfg(feature = "tui")]
pub mod terminal;

#[derive(Clone, Debug)]
pub struct TextCopy {
    pub id: u64,
    pub text: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    System,
    Terminal,
}

use anyhow::{Result, bail};
use std::path::PathBuf;

pub struct Options {
    pub directory: PathBuf,
}

impl Options {
    pub fn parse() -> Result<Option<Self>> {
        let mut directory = None;
        for arg in std::env::args_os().skip(1) {
            if arg == "--help" || arg == "-h" {
                println!(
                    "basalgrim · Molten-Zharr\n\nUsage: basalgrim [DIRECTORY]\n       basalgrim-tui [DIRECTORY]\n\nDefault keys: F1 help · F10 settings · Ctrl+Q quit\n--help, -h      Show help\n--version, -V   Show version"
                );
                return Ok(None);
            }
            if arg == "--version" || arg == "-V" {
                println!("basalgrim {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            if arg.as_encoded_bytes().starts_with(b"-") {
                bail!("Unknown option: {}", arg.to_string_lossy());
            }
            if directory.replace(PathBuf::from(arg)).is_some() {
                bail!("Expected at most one directory");
            }
        }
        let directory = directory.unwrap_or(std::env::current_dir()?);
        let directory = directory.canonicalize()?;
        anyhow::ensure!(
            directory.is_dir(),
            "Not a directory: {}",
            directory.display()
        );
        Ok(Some(Self { directory }))
    }
}

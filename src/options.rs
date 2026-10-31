use anyhow::{Result, bail};
use std::path::PathBuf;

pub struct Options {
    pub directory: PathBuf,
    pub explicit_directory: bool,
    pub no_session: bool,
}

impl Options {
    pub fn parse() -> Result<Option<Self>> {
        let mut directory = None;
        let mut no_session = false;
        for arg in std::env::args_os().skip(1) {
            if arg == "--help" || arg == "-h" {
                println!(
                    "basalgrim · Molten-Zharr\n\nUsage: basalgrim [--no-session] [DIRECTORY]\n       basalgrim-tui [--no-session] [DIRECTORY]\n\nWithout DIRECTORY, restore the saved session.\nDefault keys: F1 help · F10 settings · Ctrl+Q quit\n--no-session   Start without loading or saving the session\n--help, -h      Show help\n--version, -V   Show version"
                );
                return Ok(None);
            }
            if arg == "--version" || arg == "-V" {
                println!("basalgrim {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            if arg == "--no-session" {
                no_session = true;
                continue;
            }
            if arg.as_encoded_bytes().starts_with(b"-") {
                bail!("Unknown option: {}", arg.to_string_lossy());
            }
            if directory.replace(PathBuf::from(arg)).is_some() {
                bail!("Expected at most one directory");
            }
        }
        let explicit_directory = directory.is_some();
        let directory = directory.unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| "/".into()))
        });
        let directory = directory.canonicalize().or_else(|error| {
            if explicit_directory {
                Err(error)
            } else {
                PathBuf::from("/").canonicalize()
            }
        })?;
        anyhow::ensure!(
            directory.is_dir(),
            "Not a directory: {}",
            directory.display()
        );
        Ok(Some(Self {
            directory,
            explicit_directory,
            no_session,
        }))
    }
}

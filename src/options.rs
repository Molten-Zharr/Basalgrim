use crate::desktop::{Kind, Request};
use anyhow::{Result, bail, ensure};
use std::{ffi::OsString, path::PathBuf};

pub struct Options {
    pub directory: PathBuf,
    pub explicit_directory: bool,
    pub no_session: bool,
    pub trash_home_only: bool,
    pub desktop: bool,
    pub file_manager: bool,
    pub request: Option<Request>,
}
impl Options {
    pub fn parse() -> Result<Option<Self>> {
        Self::from_args(std::env::args_os().skip(1))
    }
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<Self>> {
        let mut paths = Vec::new();
        let (mut no_session, mut trash_home_only, mut desktop, mut file_manager, mut positional) =
            (false, false, false, false, false);
        let mut kind = Kind::Open;
        let mut integration = None;
        for arg in args {
            if !positional {
                match arg.to_str() {
                    Some("--help" | "-h") => {
                        println!(
                            "basalgrim · Molten-Zharr\n\nUsage: basalgrim [OPTIONS] [--] [PATH | file:URI ...]\n       basalgrim-tui [OPTIONS] [--] [PATH ...]\n\nWithout paths, restore the saved session.\n--select             Show items in their containing folders\n--properties         Show item properties\n--desktop            Forward to the existing desktop instance\n--file-manager       Serve org.freedesktop.FileManager1 (no takeover)\n--install-desktop    Install the application launcher\n--make-default       Install and use Basalgrim to open folders\n--restore-default    Restore the previous folder application\n--no-session         Do not load or save the session\n--trash-home-only    Use only XDG_DATA_HOME/Trash\n--help, -h           Show help\n--version, -V        Show version"
                        );
                        return Ok(None);
                    }
                    Some("--version" | "-V") => {
                        println!("basalgrim {}", env!("CARGO_PKG_VERSION"));
                        return Ok(None);
                    }
                    Some("--") => {
                        positional = true;
                        continue;
                    }
                    Some("--no-session") => {
                        no_session = true;
                        continue;
                    }
                    Some("--trash-home-only") => {
                        trash_home_only = true;
                        continue;
                    }
                    Some("--desktop") => {
                        desktop = true;
                        continue;
                    }
                    Some("--file-manager") => {
                        file_manager = true;
                        desktop = true;
                        continue;
                    }
                    Some("--select" | "--properties") => {
                        ensure!(kind == Kind::Open, "Choose either --select or --properties");
                        kind = if arg == "--select" {
                            Kind::Items
                        } else {
                            Kind::Properties
                        };
                        continue;
                    }
                    Some("--install-desktop" | "--make-default" | "--restore-default") => {
                        ensure!(integration.is_none(), "Choose one integration action");
                        integration = Some(if arg == "--install-desktop" {
                            crate::desktop::InstallAction::Install
                        } else if arg == "--make-default" {
                            crate::desktop::InstallAction::Default
                        } else {
                            crate::desktop::InstallAction::Restore
                        });
                        continue;
                    }
                    _ if arg.as_encoded_bytes().starts_with(b"-") => {
                        bail!("Unknown option: {}", arg.to_string_lossy())
                    }
                    _ => {}
                }
            }
            ensure!(paths.len() < crate::desktop::MAX_ITEMS, "Too many paths");
            let path = if let Some(uri) = arg.to_str().filter(|s| s.starts_with("file:")) {
                crate::clipboard::files::path(uri)?
            } else {
                ensure!(
                    !arg.to_str().is_some_and(|s| s.contains("://")),
                    "Only local file: URIs are supported; use the Network dialog"
                );
                PathBuf::from(arg)
            };
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()?.join(path)
            };
            // Canonicalize the parent only; revealing a symlink must select the link.
            let path = if let Some(name) = path.file_name() {
                path.parent().unwrap().canonicalize()?.join(name)
            } else {
                path.canonicalize()?
            };
            paths.push(path);
        }
        if let Some(action) = integration {
            ensure!(
                paths.is_empty() && !desktop && kind == Kind::Open,
                "Integration actions do not accept paths"
            );
            println!(
                "{}",
                crate::desktop::integrate(&crate::desktop::Config::current()?, action)?
            );
            if let Some(warning) = crate::desktop::service::notify_registration(action) {
                eprintln!("{warning}");
            }
            return Ok(None);
        }
        ensure!(
            kind == Kind::Open || !paths.is_empty(),
            "Provide at least one path"
        );
        let explicit_directory = !paths.is_empty();
        let fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut directory = fallback;
        let mut request = None;
        if paths.len() == 1 && kind == Kind::Open && !desktop && paths[0].is_dir() {
            directory = paths.remove(0).canonicalize()?;
        } else if !paths.is_empty() || desktop {
            let uris = paths
                .iter()
                .map(|p| crate::clipboard::files::uri(p))
                .collect::<Result<Vec<_>>>()?;
            request = Some(Request::from_uris(
                kind,
                &uris,
                std::env::var("DESKTOP_STARTUP_ID").unwrap_or_default(),
            )?);
        }
        Ok(Some(Self {
            directory,
            explicit_directory,
            no_session,
            trash_home_only,
            desktop,
            file_manager,
            request,
        }))
    }
}

//! Unprivileged protocol peer for isolated tests; never used by application startup.
fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        !rustix::process::geteuid().is_root(),
        "Fixture must not run as root"
    );
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o022));
    let path = std::env::args_os().nth(1).expect("private journal path");
    let database = basalgrim::journal::Database::open(std::path::Path::new(&path))?;
    basalgrim::privileged::serve(
        std::io::stdin(),
        std::io::stdout(),
        database,
        rustix::process::geteuid().as_raw(),
    )
}

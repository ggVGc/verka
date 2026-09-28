//! Process-local diagnostic logging for the interactive client.
//!
//! The TUI cannot use stderr after it takes over the terminal, so its tracing
//! subscriber writes an append-only log beside the server socket instead.

use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tracing::Level;
use tracing_subscriber::fmt::time::UtcTime;

const LOG_NAME: &str = "styra-tui.log";

/// The client diagnostic log belongs beside the socket it talks to, just like
/// `styra-server.log`. This also makes a custom `--socket` self-contained.
pub(crate) fn path_for_socket(socket: &Path) -> PathBuf {
    socket
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOG_NAME)
}

/// Install the process-wide tracing subscriber, appending UTC, tagged records
/// to `path`. The caller chooses whether a missing runtime directory is a
/// reason to skip logging (as it is for standalone mode).
pub(crate) fn install(path: &Path, private_parent: bool) -> Result<()> {
    let parent = path
        .parent()
        .expect("a log path made from a socket always has a parent");
    fs::create_dir_all(parent)
        .with_context(|| format!("creating TUI log directory {}", parent.display()))?;
    if private_parent {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting TUI log directory {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening TUI log {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting TUI log {}", path.display()))?;
    install_file(file)
}

fn install_file(file: File) -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_target(true)
        .with_timer(UtcTime::rfc_3339())
        .with_writer(file)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("installing the TUI tracing subscriber")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lives_beside_socket() {
        assert_eq!(
            path_for_socket(Path::new("/run/user/1000/styra/styra.sock")),
            PathBuf::from("/run/user/1000/styra/styra-tui.log")
        );
    }
}

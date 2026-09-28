//! Process-local diagnostic logging for the interactive client.
//!
//! The TUI cannot use stderr after it takes over the terminal, so its tracing
//! subscriber writes an append-only log beside the server socket instead.

use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::Level;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::fmt::MakeWriter;

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
        .with_writer(LineWriterFactory::new(file))
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("installing the TUI tracing subscriber")
}

/// Makes one tracing event one append operation. `flock` serializes that
/// operation with other Styra TUI processes, while the mutex covers writers in
/// this process (which share one open file description, and thus one flock).
#[derive(Clone)]
struct LineWriterFactory {
    file: Arc<Mutex<File>>,
}

impl LineWriterFactory {
    fn new(file: File) -> Self {
        Self {
            file: Arc::new(Mutex::new(file)),
        }
    }
}

impl<'a> MakeWriter<'a> for LineWriterFactory {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            file: Arc::clone(&self.file),
            bytes: Vec::new(),
        }
    }
}

/// Buffers formatter fragments until the event is complete, then appends them
/// as a locked whole line when tracing drops the writer.
struct LineWriter {
    file: Arc<Mutex<File>>,
    bytes: Vec<u8>,
}

impl Write for LineWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let descriptor = file.as_raw_fd();
        // Every Styra TUI writer takes this advisory lock. If it cannot be
        // acquired, skip this diagnostic event rather than delaying or
        // destabilising the terminal UI.
        if unsafe { libc::flock(descriptor, libc::LOCK_EX) } == -1 {
            return;
        }
        let _ = file.write_all(&self.bytes);
        let _ = unsafe { libc::flock(descriptor, libc::LOCK_UN) };
    }
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

    #[test]
    fn formatter_fragments_become_one_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("styra-tui.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        let factory = LineWriterFactory::new(file);
        let mut writer = factory.make_writer();

        writer.write_all(b"first ").unwrap();
        writer.write_all(b"record\n").unwrap();
        drop(writer);

        assert_eq!(fs::read_to_string(path).unwrap(), "first record\n");
    }
}

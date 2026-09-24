//! Print the transcript of one audio file, and fetch the model that does it.
//!
//! The same transcription `styra-server` answers a `transcribe_audio` request
//! with, reachable without a server, a socket, or a session — which is what
//! makes it useful for checking a recording, or a model, on its own.
//!
//!   styra-transcribe --download-model     # once, before the first transcript
//!   styra-transcribe recording.wav
//!
//! Transcribing never downloads, so the first of those is what puts the model
//! in reach of the second and of the server. `STYRA_WHISPER_MODEL` selects
//! which model that is, and `STYRA_WHISPER_DEVICE` selects an accelerated
//! build's GPU or CPU fallback.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let path = match arguments.next().as_deref() {
        None | Some("--help" | "-h") => {
            println!(
                "usage: styra-transcribe FILE\n       \
                        styra-transcribe --download-model\n\n\
                 Prints the transcript of one audio file. Transcribing reads\n\
                 the Whisper model from the local cache and never downloads\n\
                 it; --download-model is what puts it there, for this command\n\
                 and for styra-server alike. STYRA_WHISPER_MODEL selects the\n\
                 model; STYRA_WHISPER_DEVICE selects auto, cpu, or gpu."
            );
            return Ok(());
        }
        // Downloading reports progress as it goes, so it keeps stdout.
        Some("--download-model") => {
            if arguments.next().is_some() {
                bail!("styra-transcribe --download-model takes no other argument");
            }
            return styra_transcription::download();
        }
        Some(path) => PathBuf::from(path),
    };
    if arguments.next().is_some() {
        bail!("styra-transcribe takes exactly one file");
    }
    let transcript = aside(|| styra_transcription::transcribe(&path))?;
    // The transcript alone on stdout, so the command composes with a pipe.
    println!("{transcript}");
    Ok(())
}

/// Run `work` with stdout pointed at stderr, and restore it afterwards.
///
/// Loading the model may print notes about the accelerator it found. Those are
/// worth seeing, but not worth putting in the middle of a transcript someone
/// is piping into a file, so stdout diagnostics are moved to stderr instead of
/// being silenced.
fn aside<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let saved = duplicate(libc::STDOUT_FILENO).context("saving stdout")?;
    redirect(std::io::stderr().as_raw_fd(), libc::STDOUT_FILENO).context("diverting stdout")?;
    let outcome = work();
    // Anything the work left buffered belongs on stderr with the rest of it,
    // so flush before stdout is itself again.
    let _ = std::io::stdout().flush();
    redirect(saved.as_raw_fd(), libc::STDOUT_FILENO).context("restoring stdout")?;
    outcome
}

fn duplicate(fd: i32) -> Result<OwnedFd> {
    // SAFETY: `dup` returns a new descriptor this process owns outright, so
    // handing it to `OwnedFd` gives it exactly one closer.
    match unsafe { libc::dup(fd) } {
        -1 => Err(std::io::Error::last_os_error().into()),
        copy => Ok(unsafe { OwnedFd::from_raw_fd(copy) }),
    }
}

fn redirect(from: i32, to: i32) -> Result<()> {
    // SAFETY: both descriptors are open for the whole call; `dup2` closes
    // `to` and reopens it onto `from`, which is the point.
    match unsafe { libc::dup2(from, to) } {
        -1 => Err(std::io::Error::last_os_error().into()),
        _ => Ok(()),
    }
}

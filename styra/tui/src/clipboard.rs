//! Copies text to the operator's system clipboard.
//!
//! Owning a selection is not a write-and-forget call: on X11 the content lives
//! in the owning process, which has to stay connected and answer selection
//! requests for as long as anyone might paste. Styra hands the text to the
//! platform clipboard utility instead — `wl-copy`, `xclip`, `xsel`, `pbcopy` —
//! which does exactly that job, takes text of any length over a pipe, and
//! keeps the selection alive after Styra exits.
//!
//! Where there is no display server to reach (over SSH, on a bare console) it
//! falls back to the OSC 52 escape sequence, which asks the terminal emulator
//! on the far end to do the copying. That path is best-effort: the sequence is
//! unacknowledged, terminals cap how much they will accept, and a multiplexer
//! only forwards it when configured to, so `OSC52_LIMIT` keeps Styra from
//! claiming a copy the terminal will have silently dropped.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// The largest text worth pushing through OSC 52. Terminal limits vary and are
/// undiscoverable; xterm's default is the small one, and this sits under it.
const OSC52_LIMIT: usize = 64 * 1024;

pub fn copy(text: &str) -> io::Result<()> {
    if copy_via_utility(text) {
        return Ok(());
    }
    copy_via_terminal(text)
}

/// Hand the text to a platform clipboard utility, reporting whether one took
/// it. Candidates are filtered by what the environment can actually reach: a
/// Wayland or X11 tool with no display server would either fail or, worse,
/// hang waiting for one.
fn copy_via_utility(text: &str) -> bool {
    let mut candidates: Vec<(&str, &[&str])> = Vec::new();
    if is_set("WAYLAND_DISPLAY") {
        candidates.push(("wl-copy", &[]));
    }
    if is_set("DISPLAY") {
        candidates.push(("xclip", &["-selection", "clipboard"]));
        candidates.push(("xsel", &["--clipboard", "--input"]));
    }
    if cfg!(target_os = "macos") {
        candidates.push(("pbcopy", &[]));
    }
    candidates
        .into_iter()
        .any(|(program, arguments)| pipe_to(program, arguments, text))
}

fn is_set(variable: &str) -> bool {
    std::env::var_os(variable).is_some_and(|value| !value.is_empty())
}

/// Spawn `program` and write the text to its stdin. No shell is involved, so
/// the text needs no quoting and cannot be read as arguments.
fn pipe_to(program: &str, arguments: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // Dropping the handle closes the pipe, which is what tells the utility the
    // selection is complete. `wl-copy` then daemonises and `xclip` forks, so
    // both exit immediately; neither wait blocks on the paste.
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    written && child.wait().is_ok_and(|status| status.success())
}

/// Write an OSC 52 sequence to the controlling terminal — `/dev/tty` rather
/// than stdout, so a redirected stdout cannot swallow it.
fn copy_via_terminal(text: &str) -> io::Result<()> {
    if text.len() > OSC52_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "no clipboard utility available, and {} bytes is too much to \
                 send through the terminal; install wl-copy or xclip",
                text.len()
            ),
        ));
    }
    let sequence = osc52(text, Multiplexer::detect());
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut tty) => {
            tty.write_all(sequence.as_bytes())?;
            tty.flush()
        }
        Err(_) => {
            let mut stdout = io::stdout();
            stdout.write_all(sequence.as_bytes())?;
            stdout.flush()
        }
    }
}

/// Which multiplexer, if any, sits between Styra and the terminal that owns
/// the clipboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Multiplexer {
    None,
    Tmux,
    Screen,
}

impl Multiplexer {
    fn detect() -> Self {
        if is_set("TMUX") {
            Self::Tmux
        } else if is_set("STY") {
            Self::Screen
        } else {
            Self::None
        }
    }
}

/// The copied text is base64-encoded, so anything it contains — ESC, BEL, the
/// string terminator — is inert and cannot break out of the sequence.
fn osc52(text: &str, multiplexer: Multiplexer) -> String {
    let encoded = STANDARD.encode(text);
    match multiplexer {
        Multiplexer::None => format!("\x1b]52;c;{encoded}\x1b\\"),
        // A multiplexer consumes the sequence itself unless it is wrapped in a
        // DCS passthrough (tmux additionally wants `allow-passthrough on`).
        Multiplexer::Tmux => format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x1b\x1b\\\x1b\\"),
        Multiplexer::Screen => format!("\x1bP\x1b]52;c;{encoded}\x07\x1b\\"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_text_as_an_osc_52_clipboard_sequence() {
        assert_eq!(
            osc52("hello", Multiplexer::None),
            "\x1b]52;c;aGVsbG8=\x1b\\"
        );
    }

    #[test]
    fn wraps_the_sequence_in_a_passthrough_under_a_multiplexer() {
        assert_eq!(
            osc52("hello", Multiplexer::Tmux),
            "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x1b\x1b\\\x1b\\"
        );
        assert_eq!(
            osc52("hello", Multiplexer::Screen),
            "\x1bP\x1b]52;c;aGVsbG8=\x07\x1b\\"
        );
    }

    #[test]
    fn refuses_to_pretend_a_large_copy_went_through_the_terminal() {
        let error = copy_via_terminal(&"x".repeat(OSC52_LIMIT + 1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("too much"));
    }

    #[test]
    fn a_missing_utility_is_not_a_copy() {
        assert!(!pipe_to("styra-no-such-clipboard-utility", &[], "hello"));
    }
}

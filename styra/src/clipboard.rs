//! Copies text to the operator's system clipboard via the OSC 52 terminal
//! escape sequence. Unlike shelling out to a platform-specific clipboard
//! utility (`xclip`, `wl-copy`, `pbcopy`, ...), OSC 52 works locally, over
//! SSH, and inside a multiplexer with clipboard passthrough enabled — every
//! terminal Styra already targets in `terminal.rs` supports it natively —
//! without Styra needing to know which one is running or spawn a process.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use std::io::{self, Write};

pub fn copy(text: &str) -> io::Result<()> {
    copy_to(&mut io::stdout(), text)
}

fn copy_to(writer: &mut impl Write, text: &str) -> io::Result<()> {
    let encoded = STANDARD.encode(text);
    write!(writer, "\x1b]52;c;{encoded}\x07")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_text_as_an_osc_52_clipboard_sequence() {
        let mut buffer = Vec::new();
        copy_to(&mut buffer, "hello").unwrap();
        assert_eq!(buffer, b"\x1b]52;c;aGVsbG8=\x07");
    }
}

//! Newline-delimited JSON framing shared by the client and server.

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{BufRead, Read, Write};

/// Maximum encoded request size accepted by the server.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// Write one JSON value followed by a newline and flush it.
pub fn write_message<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: Write,
    T: Serialize,
{
    serde_json::to_writer(&mut *writer, value).context("encoding the Styra protocol message")?;
    writer
        .write_all(b"\n")
        .context("writing the Styra protocol message")?;
    writer
        .flush()
        .context("flushing the Styra protocol message")?;
    Ok(())
}

/// Read and decode one newline-delimited JSON value.
pub fn read_message<R, T>(reader: &mut R) -> Result<T>
where
    R: BufRead,
    T: DeserializeOwned,
{
    let mut bytes = Vec::new();
    let read = reader
        .read_until(b'\n', &mut bytes)
        .context("reading the Styra protocol message")?;
    decode_message(read, &bytes)
}

/// Read and decode one newline-delimited JSON value, enforcing `max_bytes`.
pub fn read_message_limited<R, T>(reader: &mut R, max_bytes: usize) -> Result<T>
where
    R: BufRead,
    T: DeserializeOwned,
{
    let mut bytes = Vec::new();
    let read = Read::take(reader, (max_bytes as u64) + 1)
        .read_until(b'\n', &mut bytes)
        .context("reading the Styra protocol message")?;
    if bytes.len() > max_bytes {
        bail!("protocol message exceeds the {max_bytes}-byte limit");
    }
    decode_message(read, &bytes)
}

fn decode_message<T: DeserializeOwned>(read: usize, bytes: &[u8]) -> Result<T> {
    if read == 0 {
        bail!("peer closed the socket without a protocol message");
    }
    serde_json::from_slice(bytes).context("decoding the Styra protocol message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn messages_are_newline_delimited_json() {
        let mut output = Vec::new();
        write_message(&mut output, &json!({"operation": "health"})).unwrap();
        assert_eq!(output, b"{\"operation\":\"health\"}\n");

        let decoded: serde_json::Value = read_message(&mut Cursor::new(output)).unwrap();
        assert_eq!(decoded, json!({"operation": "health"}));
    }

    #[test]
    fn oversized_messages_are_rejected() {
        let mut input = Cursor::new(b"12345\n");
        let error = read_message_limited::<_, serde_json::Value>(&mut input, 4).unwrap_err();
        assert!(error.to_string().contains("4-byte limit"));
    }
}

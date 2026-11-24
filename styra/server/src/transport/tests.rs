use super::*;
use serde_json::json;
use std::io::{Cursor, Write};

#[derive(Default)]
struct CountingWriter {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn messages_are_newline_delimited_json() {
    let mut output = Vec::new();
    write_message(&mut output, &json!({"operation": "health"})).unwrap();
    assert_eq!(output, b"{\"operation\":\"health\"}\n");

    let decoded: serde_json::Value = read_message(&mut Cursor::new(output)).unwrap();
    assert_eq!(decoded, json!({"operation": "health"}));
}

#[test]
fn large_messages_use_buffered_writes() {
    let value: Vec<usize> = (0..10_000).collect();
    let mut output = CountingWriter::default();
    write_message(&mut output, &value).unwrap();

    let mut expected = serde_json::to_vec(&value).unwrap();
    expected.push(b'\n');
    assert_eq!(output.bytes, expected);
    assert!(output.writes < 20, "used {} writes", output.writes);
    assert_eq!(output.flushes, 1);
}

#[test]
fn oversized_messages_are_rejected() {
    let mut input = Cursor::new(b"12345\n");
    let error = read_message_limited::<_, serde_json::Value>(&mut input, 4).unwrap_err();
    assert!(error.to_string().contains("4-byte limit"));
}

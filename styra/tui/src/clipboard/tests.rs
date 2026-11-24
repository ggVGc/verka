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

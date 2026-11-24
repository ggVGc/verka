use super::*;

fn block(body: &str) -> String {
    format!("Here you go.\n{OPEN}\n{body}\n{CLOSE}\n")
}

/// Framing and unframing are one agreement read in two directions, so
/// every contract must name its shape and survive the round trip exactly.
#[test]
fn every_contract_frames_its_shape_and_recovers_the_operator_message() {
    for contract in CONTRACTS {
        let framed = frame("which files handle auth?", contract);
        assert!(
            framed.starts_with("which files handle auth?"),
            "{contract:?}"
        );
        assert!(
            framed.contains(OPEN),
            "{contract:?} named no opening delimiter"
        );
        assert!(
            framed.contains(CLOSE),
            "{contract:?} named no closing delimiter"
        );
        assert!(
            framed.contains(shape(contract)),
            "{contract:?} did not state its shape"
        );
        if contract == Contract::Files {
            assert!(framed.contains("one file location per line"));
        }
        assert_eq!(
            unframe(&framed),
            Some(("which files handle auth?", contract)),
            "{contract:?} did not survive the round trip"
        );
    }
}

/// Only a message this server framed unframes. A message that merely
/// talks about answer blocks is the operator's own text, all of it.
#[test]
fn an_unframed_message_is_left_alone() {
    assert_eq!(unframe("just a question"), None);
    assert_eq!(unframe("explain the <styra:answer> convention to me"), None);
    // Including one that quotes the instructions without ending in them.
    let quoted = format!("{}\nand then explain why", frame("go", Contract::Text));
    assert_eq!(unframe(&quoted), None);
}

/// An agent that echoes the requested format while thinking must not have
/// its rehearsal read as the answer.
#[test]
fn the_last_block_is_the_answer() {
    let message = format!("{OPEN}\n...your answer...\n{CLOSE}\nActually:\n{OPEN}\nreal\n{CLOSE}");
    assert_eq!(extract(&message).unwrap().trim(), "real");
}

#[test]
fn an_unclosed_block_is_an_error() {
    let error = extract(&format!("{OPEN}\nhalf an answer"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("never closed"), "{error}");
}

#[test]
fn text_is_the_block_with_its_surrounding_whitespace_removed() {
    let value = parse(&block("  it caches nothing.  "), Contract::Text).unwrap();
    assert_eq!(value, AnswerValue::Text("it caches nothing.".into()));
}

#[test]
fn lines_drops_blanks_and_comments() {
    let value = parse(&block("first\n\n# a note\n  second  "), Contract::Lines).unwrap();
    assert_eq!(
        value,
        AnswerValue::Lines(vec!["first".into(), "second".into()])
    );
}

#[test]
fn single_file_locations_preserve_positions_and_descriptions() {
    for (input, expected) in [
        (
            "src/auth.rs",
            FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: None,
                column: None,
                end_line: None,
                description: String::new(),
            },
        ),
        (
            "src/auth.rs:12:5: checks the token",
            FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: Some(12),
                column: Some(5),
                end_line: None,
                description: "checks the token".into(),
            },
        ),
        (
            "a.rs:3: see also: b.rs",
            FileLocation {
                path: PathBuf::from("a.rs"),
                line: Some(3),
                column: None,
                end_line: None,
                description: "see also: b.rs".into(),
            },
        ),
        // A non-numeric field ends the position and starts the description.
        (
            "README: the entry point",
            FileLocation {
                path: PathBuf::from("README"),
                line: None,
                column: None,
                end_line: None,
                description: "the entry point".into(),
            },
        ),
    ] {
        assert_eq!(
            parse(&block(input), Contract::Files).unwrap(),
            AnswerValue::Files(vec![expected]),
            "{input}"
        );
    }
}

/// Agents name a function or a block as a range of lines unasked. The
/// range is a position, not the start of the description.
#[test]
fn a_range_of_lines_is_a_position() {
    let AnswerValue::Files(files) = parse(
        &block(
            "IndexHandler.scala:66-79: Throttled index flushing\n\
                 IndexHandler.scala:97–119: Kryo setup\n\
                 a.rs:5-5: one line\n\
                 b.rs:9-3: backwards",
        ),
        Contract::Files,
    )
    .unwrap() else {
        panic!("the files contract must parse to files");
    };
    assert_eq!(files[0].path, PathBuf::from("IndexHandler.scala"));
    assert_eq!(files[0].line, Some(66));
    assert_eq!(files[0].end_line, Some(79));
    assert_eq!(files[0].column, None);
    assert_eq!(files[0].description, "Throttled index flushing");
    assert_eq!(files[0].located(), "IndexHandler.scala:66-79");
    // An en dash, as agents write ranges in prose.
    assert_eq!((files[1].line, files[1].end_line), (Some(97), Some(119)));
    assert_eq!(files[1].description, "Kryo setup");
    // A range of one line is that line.
    assert_eq!((files[2].line, files[2].end_line), (Some(5), None));
    // A range running backwards is not one, and is left as text.
    assert_eq!(files[3].line, None);
    assert_eq!(files[3].description, "9-3: backwards");
}

#[test]
fn an_empty_files_block_is_an_error() {
    let error = parse(&block("\n# nothing found\n"), Contract::Files)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no file locations"), "{error}");
}

#[test]
fn json_is_decoded() {
    let value = parse(&block(r#"{"crate": "styra"}"#), Contract::Json).unwrap();
    let AnswerValue::Json(json) = value else {
        panic!("the json contract must parse to json");
    };
    assert_eq!(json["crate"], "styra");
}

#[test]
fn malformed_json_is_an_error_that_says_so() {
    let error = parse(&block("{oh no"), Contract::Json)
        .unwrap_err()
        .to_string();
    assert!(error.contains("not valid JSON"), "{error}");
}

#[test]
fn the_answer_comes_from_the_last_agent_message() {
    let events = vec![
        AgentEvent::UserMessage {
            text: "which files?".into(),
        },
        AgentEvent::AgentMessage {
            text: "Let me look.".into(),
        },
        AgentEvent::CommandStarted {
            command: "rg auth".into(),
        },
        AgentEvent::AgentMessage {
            text: block("src/auth.rs:12"),
        },
    ];
    let answer = answer_from_events(&events, Contract::Files).unwrap();
    assert_eq!(
        answer.value,
        Some(AnswerValue::Files(vec![FileLocation {
            path: PathBuf::from("src/auth.rs"),
            line: Some(12),
            column: None,
            end_line: None,
            description: String::new(),
        }]))
    );
    assert_eq!(answer.error, None);
    assert!(answer.source.contains("src/auth.rs"));
}

/// A reply that ignored the contract is still an answer: the operator needs
/// to see what was said, and why it did not satisfy what was asked.
#[test]
fn a_reply_that_misses_the_contract_keeps_the_message_and_says_why() {
    // This also covers extract's missing-block error through parse and read.
    let events = vec![AgentEvent::AgentMessage {
        text: "I could not determine that.".into(),
    }];
    let answer = answer_from_events(&events, Contract::Files).unwrap();
    assert_eq!(answer.contract, Contract::Files);
    assert_eq!(answer.value, None);
    assert!(!answer.is_parsed());
    assert!(answer.error.unwrap().contains(OPEN));
    assert_eq!(answer.source, "I could not determine that.");
}

#[test]
fn a_session_that_has_not_answered_yet_says_so() {
    let events = vec![AgentEvent::UserMessage {
        text: "which files?".into(),
    }];
    let error = answer_from_events(&events, Contract::Files)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no agent message"), "{error}");
}

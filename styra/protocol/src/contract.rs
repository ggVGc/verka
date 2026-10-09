//! Typed turn answers: framing a message with the shape its reply must take,
//! and parsing that reply back into a value.
//!
//! A [`Contract`] is applied at both ends of one turn. On the way out,
//! [`frame`] appends instructions describing the shape and the delimiters the
//! answer must sit in. On the way back, [`parse`] pulls the delimited block out
//! of the agent's message and reads it as the named shape.
//!
//! Both halves live here, in the protocol crate, deliberately. The instructions
//! and the parser are two views of one agreement, and keeping them in the same
//! file is what stops them drifting apart. Owning them here also means every
//! client — the Styra interface, an editor plugin, a script — asks for a shape
//! with the same words, so the parser only ever sees one phrasing of the
//! request.
//!
//! The parser reads decoded [`AgentEvent::AgentMessage`] text, not the
//! provider's wire output. Genta has already normalised Claude and Codex into
//! the same event vocabulary by that point, so a contract is provider-agnostic
//! without knowing that either provider exists.

use crate::event::AgentEvent;
use crate::protocol::{Answer, AnswerValue, Contract, FileLocation};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Delimiters the agent is asked to wrap its answer in. Deliberately unlike
/// anything in ordinary prose or Markdown, so an answer that merely discusses
/// code cannot open one by accident.
pub const OPEN: &str = "<styra:answer>";
pub const CLOSE: &str = "</styra:answer>";

/// Append the contract's instructions to an operator message.
///
/// The message keeps its own text unchanged and unquoted: the agent should read
/// it as the request it is, with the shape requirement following as a separate
/// instruction rather than woven into the question.
pub fn frame(message: &str, contract: Contract) -> String {
    format!("{}\n{}", message.trim_end(), instructions(contract))
}

/// The instruction block [`frame`] appends. Public so a client can show an
/// operator exactly what was added to their message.
pub fn instructions(contract: Contract) -> String {
    format!(
        "\n---\nWhen you have finished, end your reply with a single answer \
         block, delimited exactly like this and containing nothing else:\n\n\
         {OPEN}\n...your answer...\n{CLOSE}\n\n{}\nEverything outside the \
         block is ignored, so do any explaining there rather than inside it.",
        shape(contract)
    )
}

/// Split a framed message back into the operator's own text and the contract
/// it was framed with, or `None` if it was never framed.
///
/// The exact inverse of [`frame`], and only that: it matches each contract's
/// instructions as a literal suffix rather than guessing from anything the
/// text resembles. A client uses it to show the operator's message as they
/// wrote it, without the boilerplate the server appended — the verbatim wire
/// line is still in the journal for anyone who wants to see what was sent.
pub fn unframe(message: &str) -> Option<(&str, Contract)> {
    for contract in CONTRACTS {
        if let Some(text) = message.strip_suffix(&instructions(contract)) {
            return Some((text.trim_end(), contract));
        }
    }
    None
}

/// Every contract, for callers that offer or test them all.
pub const CONTRACTS: [Contract; 4] = [
    Contract::Text,
    Contract::Lines,
    Contract::Files,
    Contract::Json,
];

/// The one sentence describing a contract's shape, inside [`instructions`].
fn shape(contract: Contract) -> &'static str {
    match contract {
        Contract::Text => "The block holds prose. Keep it brief.",
        Contract::Lines => {
            "The block holds one item per line, with no numbering, bullets, or \
             surrounding prose."
        }
        Contract::Files => {
            "The block holds one file location per line, written as `path`, \
             `path:line`, or `path:line:column`, each optionally followed by \
             `: description`. Paths are relative to the Workspace root. No \
             numbering, bullets, or surrounding prose."
        }
        Contract::Json => {
            "The block holds a single JSON value and nothing else — no code \
             fence, no commentary."
        }
    }
}

/// Read the last agent message in an event stream under `contract`.
///
/// Answering from the last message rather than the last event is what lets a
/// contract survive a turn that used tools: the agent's closing message is the
/// answer, and everything it did to get there is passed over.
///
/// A reply that does not satisfy the contract is reported inside the [`Answer`]
/// rather than as an error, so the caller still has the message to show. Only
/// the absence of any reply to read is an error — there is nothing to return.
pub fn answer_from_events(events: &[AgentEvent], contract: Contract) -> Result<Answer> {
    let source = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::AgentMessage { text } => Some(text),
            _ => None,
        })
        .context("the session has no agent message to answer from yet")?;
    Ok(read(source, contract))
}

/// Read one agent message as a typed answer, keeping the message either way.
pub fn read(message: &str, contract: Contract) -> Answer {
    let (value, error) = match parse(message, contract) {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    Answer {
        contract,
        value,
        error,
        source: message.to_owned(),
    }
}

/// Read one agent message as a typed answer.
pub fn parse(message: &str, contract: Contract) -> Result<AnswerValue> {
    let body = extract(message)?;
    Ok(match contract {
        Contract::Text => AnswerValue::Text(body.trim().to_owned()),
        Contract::Lines => AnswerValue::Lines(items(&body)),
        Contract::Files => AnswerValue::Files(locations(&body)?),
        Contract::Json => AnswerValue::Json(
            serde_json::from_str(body.trim()).context("the answer block is not valid JSON")?,
        ),
    })
}

/// Pull the delimited answer out of a message.
///
/// The last block wins. Agents narrate before they answer and routinely echo
/// the requested format while thinking about it, so an earlier block is far
/// more likely to be a rehearsal than the answer.
pub fn extract(message: &str) -> Result<String> {
    let Some(open) = message.rfind(OPEN) else {
        bail!("the agent's reply contains no {OPEN} block");
    };
    let after = open + OPEN.len();
    let Some(close) = message[after..].find(CLOSE) else {
        bail!("the agent's {OPEN} block was never closed with {CLOSE}");
    };
    Ok(message[after..after + close].to_owned())
}

/// Content lines of a block: blanks and `#` comments carry no items.
fn items(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn locations(body: &str) -> Result<Vec<FileLocation>> {
    let locations: Vec<FileLocation> = items(body).iter().map(|line| location(line)).collect();
    if locations.is_empty() {
        bail!("the answer block names no file locations");
    }
    Ok(locations)
}

/// Read one `path[:line[:column]][: description]` line.
///
/// Splitting on `:` from the left and stopping at the first field that is not a
/// number keeps Windows-style and colon-bearing paths from being mistaken for
/// positions, and lets a description contain colons freely.
///
/// A range of lines, `path:66-79`, is read too, though the agent is not asked
/// for one: agents pointing at a function or a block write it unprompted, and
/// reading it as no position at all would put the range in the description
/// and send a jump to the top of the file. A range names no column, so none
/// is looked for after it.
fn location(line: &str) -> FileLocation {
    let mut fields = line.split(':');
    let path = fields.next().unwrap_or_default().trim();
    let mut rest = fields.collect::<Vec<_>>();
    let mut line_number = None;
    let mut end_line = None;
    let mut column = None;
    if let Some((first, last)) = rest.first().and_then(|field| line_range(field)) {
        line_number = Some(first);
        end_line = last;
        rest.remove(0);
        if end_line.is_none() {
            if let Some(second) = rest
                .first()
                .and_then(|field| field.trim().parse::<u32>().ok())
            {
                column = Some(second);
                rest.remove(0);
            }
        }
    }
    FileLocation {
        path: PathBuf::from(path),
        line: line_number,
        column,
        end_line,
        description: rest.join(":").trim().to_owned(),
    }
}

/// A line, `66`, or a range of lines, `66-79`, as the first line and the last
/// when there is one. Agents write ranges with an en dash as often as with a
/// hyphen, so both are read. A range running backwards is not a range, and
/// one ending where it starts is just its line.
fn line_range(field: &str) -> Option<(u32, Option<u32>)> {
    let field = field.trim();
    let Some((first, last)) = field.split_once(['-', '–']) else {
        return field.parse().ok().map(|line| (line, None));
    };
    let first = first.trim().parse::<u32>().ok()?;
    let last = last.trim().parse::<u32>().ok()?;
    if last < first {
        return None;
    }
    Some((first, (last > first).then_some(last)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &str) -> String {
        format!("Here you go.\n{OPEN}\n{body}\n{CLOSE}\n")
    }

    #[test]
    fn framing_keeps_the_operator_message_and_names_the_delimiters() {
        let framed = frame("which files handle auth?", Contract::Files);
        assert!(framed.starts_with("which files handle auth?"));
        assert!(framed.contains(OPEN));
        assert!(framed.contains(CLOSE));
        assert!(framed.contains("one file location per line"));
    }

    /// Framing and unframing are one agreement read in two directions, so
    /// every contract has to survive the round trip exactly.
    #[test]
    fn unframing_recovers_the_operator_message_and_its_contract() {
        for contract in CONTRACTS {
            let framed = frame("which files handle auth?", contract);
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

    #[test]
    fn every_contract_frames_with_its_own_shape() {
        for contract in [
            Contract::Text,
            Contract::Lines,
            Contract::Files,
            Contract::Json,
        ] {
            let framed = frame("go", contract);
            assert!(framed.contains(OPEN), "{contract:?} named no delimiter");
            assert!(
                framed.contains(shape(contract)),
                "{contract:?} did not state its shape"
            );
        }
    }

    /// An agent that echoes the requested format while thinking must not have
    /// its rehearsal read as the answer.
    #[test]
    fn the_last_block_is_the_answer() {
        let message =
            format!("{OPEN}\n...your answer...\n{CLOSE}\nActually:\n{OPEN}\nreal\n{CLOSE}");
        assert_eq!(extract(&message).unwrap().trim(), "real");
    }

    #[test]
    fn a_reply_with_no_block_is_an_error_naming_the_delimiter() {
        let error = extract("I could not determine that.")
            .unwrap_err()
            .to_string();
        assert!(error.contains(OPEN), "{error}");
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
    fn a_bare_path_is_a_file_with_no_position() {
        let value = parse(&block("src/auth.rs"), Contract::Files).unwrap();
        assert_eq!(
            value,
            AnswerValue::Files(vec![FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: None,
                column: None,
                end_line: None,
                description: String::new(),
            }])
        );
    }

    #[test]
    fn a_file_carries_line_column_and_description() {
        let value = parse(
            &block("src/auth.rs:12:5: checks the token"),
            Contract::Files,
        )
        .unwrap();
        assert_eq!(
            value,
            AnswerValue::Files(vec![FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: Some(12),
                column: Some(5),
                end_line: None,
                description: "checks the token".into(),
            }])
        );
    }

    #[test]
    fn a_description_may_contain_colons() {
        let AnswerValue::Files(files) =
            parse(&block("a.rs:3: see also: b.rs"), Contract::Files).unwrap()
        else {
            panic!("the files contract must parse to files");
        };
        assert_eq!(files[0].line, Some(3));
        assert_eq!(files[0].description, "see also: b.rs");
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

    /// A description that opens with a non-numeric word must not be read as a
    /// position, or `README: the entry point` loses its path.
    #[test]
    fn a_non_numeric_field_ends_the_position() {
        let AnswerValue::Files(files) =
            parse(&block("README: the entry point"), Contract::Files).unwrap()
        else {
            panic!("the files contract must parse to files");
        };
        assert_eq!(files[0].path, PathBuf::from("README"));
        assert_eq!(files[0].line, None);
        assert_eq!(files[0].description, "the entry point");
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
}

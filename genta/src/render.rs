//! Text transcript rendering for recorded session logs.
//!
//! Turns a recorded session log — the newline-delimited wire events of a
//! codex or Claude Code run, as captured verbatim by hosts like Styra and
//! Orka — into a readable text transcript through the same decoders a live
//! session uses.

use crate::event::{decode_line, AgentEvent, DetailBlock, Protocol};

/// Render a whole session log: decode each line and lay the events out as a
/// tagged transcript. Multi-line bodies (agent messages, plans, command
/// output) follow their tag line, indented, so the transcript stays scannable.
///
/// Events with no rendered view (control traffic, unknown envelopes) are
/// skipped unless `all` is set. High-frequency lifecycle events (thread/turn
/// markers, token usage — see [`AgentEvent::is_minor`]) are skipped unless
/// `show_minor` is set.
pub fn render(log: &str, protocol: Protocol, all: bool, show_minor: bool) -> String {
    let events: Vec<AgentEvent> = log
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| decode_line(protocol, line))
        .collect();
    render_events(&events, all, show_minor)
}

/// Lay out already-decoded events as a tagged transcript — the same rendering
/// [`render`] does, for a caller (e.g. a host reconstructing a journal via its
/// own record format) that already has a `Vec<AgentEvent>` rather than raw
/// wire lines to decode.
pub fn render_events(events: &[AgentEvent], all: bool, show_minor: bool) -> String {
    let mut out = String::new();
    for event in events {
        if !all && matches!(event, AgentEvent::Unknown { .. }) {
            continue;
        }
        if !show_minor && event.is_minor() {
            continue;
        }
        render_event(&mut out, event);
    }
    out
}

fn render_event(out: &mut String, event: &AgentEvent) {
    match event {
        // Prose bodies are worth reading in full; everything else reads best
        // as its one-line summary.
        AgentEvent::AgentMessage { .. } | AgentEvent::PlanUpdated { .. } | AgentEvent::UserMessage { .. } => {
            out.push_str(&format!("{:>8} ", event.tag()));
            let mut first = true;
            for block in event.detail() {
                let text = match &block {
                    DetailBlock::Text(text) => text.clone(),
                    DetailBlock::Code { language, text } => format!(
                        "```{}\n{}```",
                        language.as_deref().unwrap_or_default(),
                        text
                    ),
                };
                for line in text.lines() {
                    if first {
                        first = false;
                    } else {
                        out.push_str("         ");
                    }
                    out.push_str(line);
                    out.push('\n');
                }
            }
            if first {
                out.push('\n');
            }
        }
        _ => {
            out.push_str(&format!("{:>8} {}\n", event.tag(), event.summary()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_codex_log_renders_as_a_tagged_transcript() {
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n",
            "{\"type\":\"item.started\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"command\":\"cargo test\",\"status\":\"completed\",\"exit_code\":0}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"All good.\\nTests pass.\"}}\n",
        );
        let text = render(log, Protocol::CodexJsonl, false, true);
        assert_eq!(
            text,
            "  thread session t-1\n\
             \x20command cargo test\n\
             \x20command cargo test (completed, exit 0)\n\
             \x20  agent All good.\n\
             \x20        Tests pass.\n"
        );
    }

    #[test]
    fn a_claude_log_renders_through_the_claude_decoder() {
        let log = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}]}}\n",
        );
        let text = render(log, Protocol::ClaudeJsonl, false, true);
        assert!(text.contains("session s-1"));
        assert!(text.contains("agent Done."));
    }

    #[test]
    fn render_events_matches_rendering_the_equivalent_raw_log() {
        // A caller that already decoded events (e.g. a host replaying its own
        // journal format) must get exactly what render() would have produced
        // from the equivalent raw wire lines.
        let log = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"t-1\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"done\"}}\n",
        );
        let events = vec![
            AgentEvent::ThreadStarted { thread_id: "t-1".into() },
            AgentEvent::AgentMessage { text: "done".into() },
        ];
        assert_eq!(
            render_events(&events, false, true),
            render(log, Protocol::CodexJsonl, false, true)
        );
    }

    #[test]
    fn minor_events_are_skipped_unless_show_minor_is_requested() {
        let events = vec![
            AgentEvent::ThreadStarted { thread_id: "t-1".into() },
            AgentEvent::TurnStarted,
            AgentEvent::AgentMessage { text: "done".into() },
            AgentEvent::TurnCompleted { usage: Default::default() },
        ];
        let hidden = render_events(&events, false, false);
        assert!(!hidden.contains("t-1"));
        assert!(!hidden.contains("turn started"));
        assert!(!hidden.contains("usage"));
        assert!(hidden.contains("agent done"));

        let shown = render_events(&events, false, true);
        assert!(shown.contains("t-1"));
        assert!(shown.contains("turn started"));
    }

    #[test]
    fn unknown_events_are_skipped_unless_all_is_requested() {
        let log = "{\"type\":\"future.event\"}\n";
        assert_eq!(render(log, Protocol::CodexJsonl, false, true), "");
        assert!(render(log, Protocol::CodexJsonl, true, true).contains("future.event"));
    }
}

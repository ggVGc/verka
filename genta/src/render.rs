//! Text transcript rendering for recorded session logs.
//!
//! Turns a recorded session log — the newline-delimited wire events of a
//! codex or Claude Code run, as captured verbatim by hosts like Styra and
//! Orka — into a readable text transcript through the same decoders a live
//! session uses.

use crate::event::{decode_line, AgentEvent, DetailBlock, Protocol, UsageTracker};

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
    // A turn's end states what the turn cost and what the thread has cost, but
    // no provider reports both; running the log past a tracker is what makes
    // the end-of-turn line able to say either.
    let mut usage = UsageTracker::default();
    for event in events {
        let mut filled;
        let event = match event {
            AgentEvent::ThreadStarted { .. }
            | AgentEvent::TurnStarted
            | AgentEvent::UsageUpdated { .. }
            | AgentEvent::TurnCompleted { .. } => {
                filled = event.clone();
                usage.observe(&mut filled);
                &filled
            }
            _ => event,
        };
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
        AgentEvent::AgentMessage { .. }
        | AgentEvent::PlanUpdated { .. }
        | AgentEvent::UserMessage { .. }
        | AgentEvent::Thinking { .. }
        | AgentEvent::FileChanged { diff: Some(_), .. }
        | AgentEvent::DiffUpdated { .. } => {
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
mod tests;

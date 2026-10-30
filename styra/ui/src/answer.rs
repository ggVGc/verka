//! Typed-answer presentation, independent of application controllers.

use crate::chrome::PanelChrome;
use styra_protocol::Answer;

/// Internal links name a section in the original agent message.
pub const LINK_PREFIX: &str = "styra:answer:";

pub fn link_section(destination: &str) -> Option<usize> {
    destination.strip_prefix(LINK_PREFIX)?.parse().ok()
}

/// Complete sections only: unfinished streamed content stays readable.
pub fn sections(message: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    use styra_protocol::contract::{CLOSE, OPEN};
    let mut offset = 0;
    std::iter::from_fn(move || {
        let start = offset + message[offset..].find(OPEN)?;
        let body = start + OPEN.len();
        let end = body + message[body..].find(CLOSE)? + CLOSE.len();
        offset = end;
        Some((start, end))
    })
}

/// Use the same replacement for rendering and link navigation. The original
/// event remains available for parsing, copying and the raw view.
pub fn presented_detail(
    event: &styra_protocol::event::AgentEvent,
    protocol: styra_protocol::event::Protocol,
) -> Vec<styra_protocol::event::DetailBlock> {
    use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode};
    if let AgentEvent::AgentMessage { text } = event {
        let mut output = String::new();
        let mut offset = 0;
        for (index, (start, end)) in sections(text).enumerate() {
            output.push_str(&text[offset..start]);
            output.push_str(&format!("[answer]({LINK_PREFIX}{index})"));
            offset = end;
        }
        if offset > 0 {
            output.push_str(&text[offset..]);
            return vec![DetailBlock::Text(output)];
        }
    }
    protocol.presented_detail(event, PresentationMode::Pretty)
}

pub struct AnswerView<'a> {
    pub chrome: PanelChrome,
    pub answer: Option<&'a Answer>,
    pub error: Option<&'a str>,
    pub selected: usize,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/answer.rs"]
mod rendering;

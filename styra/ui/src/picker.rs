//! The session and Workspace picker screens. These stand apart from
//! [`crate::app::App`] because each overlays before (or instead of) any loaded
//! session, so they render from their own borrowed data rather than app state.

use styra_protocol::{InteractionUpdate, SessionSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOrder {
    LastActivity,
    Creation,
}

impl SessionOrder {
    pub fn label(self) -> &'static str {
        match self {
            Self::LastActivity => "activity",
            Self::Creation => "creation",
        }
    }
}

/// Whether the picker has the selected session's conversation yet. Loading is
/// a round-trip to the server, so "nothing to show" and "nothing yet" are
/// genuinely different states and must not read the same.
#[derive(Debug, Clone, Copy)]
pub enum Preview<'a> {
    Loading,
    Ready(&'a [InteractionUpdate]),
}

/// Whether the picker has the selected Workspace's Session list yet. Loading
/// is a round-trip to the server, so an unread Workspace and an empty one
/// must not read the same.
#[derive(Debug, Clone, Copy)]
pub enum SessionsPreview<'a> {
    Loading,
    Ready(&'a [SessionSummary]),
}

pub fn short_id(id: &str) -> &str {
    id.get(id.len().saturating_sub(12)..).unwrap_or(id)
}

/// The newest messages selected by the host, in conversation order.
pub struct ConversationView<'a> {
    pub events: Vec<&'a styra_protocol::event::AgentEvent>,
    pub protocol: styra_protocol::event::Protocol,
}

pub fn conversation<'a>(
    events: impl DoubleEndedIterator<Item = &'a styra_protocol::event::AgentEvent>,
    protocol: styra_protocol::event::Protocol,
    count: usize,
) -> ConversationView<'a> {
    use styra_protocol::event::AgentEvent;
    let mut events: Vec<_> = events
        .rev()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
            )
        })
        .take(count)
        .collect();
    events.reverse();
    ConversationView { events, protocol }
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/picker.rs"]
mod rendering;

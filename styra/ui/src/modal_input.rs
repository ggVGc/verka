//! The message box itself: one centered, modal input box over whatever is
//! already on screen. A message built from several boxes shows them stacked
//! in it, each under a rule naming it.
//!
//! Kept separate from the surrounding screen renderer so input wrapping,
//! backdrop styling, and cursor placement remain one focused component.

/// Everything the box draws: its titles, whatever stands above the buffer
/// (the main view's queued messages), and the buffer being typed.
pub struct ModalInput<'a> {
    /// Left-hand title, naming the box and how to send from it.
    pub title: String,
    /// Right-hand title for a standing qualifier on the message — the main
    /// view's answer contract. Drawn in the accent color to set it apart from
    /// the box's own name.
    pub note: Option<String>,
    /// The model the message will be sent to, named at the top right so the
    /// box says what is about to answer — the status line it covers is not
    /// readable while the box is open.
    pub model: Option<String>,
    /// Whether the agent confirmed that model, rather than it only being what
    /// the launch asked for. Dimmed until it has, as the status line does.
    pub model_reported: bool,
    /// The reasoning effort alongside the model, as the status line shows it.
    pub effort: Option<String>,
    /// Whether the agent confirmed that effort. Dimmed until it has, as the
    /// status line does.
    pub effort_reported: bool,
    /// Text above the buffer: already-composed messages still waiting. Wrapped
    /// with the buffer, and dimmed to set it apart from what is being typed.
    pub preceding: Vec<String>,
    /// A transient line under the box: how the last path was resolved, or why
    /// it could not be. The session view has action messages for this; a box
    /// opened over a picker has nowhere else to put it.
    pub notice: Option<String>,
    /// The boxes the message is built from, in order; never empty.
    pub parts: &'a [String],
    /// The box being typed into, or highlighted while choosing.
    pub focused: usize,
    /// Whether the operator is choosing between boxes rather than typing.
    pub choosing: bool,
    /// What the message is about, such as the diff's file and line it was
    /// started from: noted under the boxes, as it is under the message sent.
    pub source: Option<String>,
    /// What an empty buffer says instead, so the box explains itself.
    pub placeholder: &'a str,
    /// Whether the terminal cursor belongs in this box. Only the innermost
    /// modal owns the cursor, so the main view hands it to its own file prompt.
    pub cursor: bool,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/modal_input.rs"]
mod rendering;

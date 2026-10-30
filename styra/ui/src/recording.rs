//! The level meter that stands in the message box's place while the
//! microphone is open.
//!
//! It occupies the box's geometry deliberately: recording is the same act as
//! typing a message, so it happens where typing happens rather than in a
//! notice somewhere else on the screen. What it has to answer, and what a
//! spinner or a word could not, is whether the machine is hearing anything —
//! and if it is barely hearing it, that the boost is the thing to reach for.

/// What the meter draws: where the level is now, where it has been, and how
/// much the input is being boosted on its way to the file.
pub struct RecordingView {
    /// The level now, as a linear amplitude: 1.0 is full scale, and anything
    /// above it is being clipped.
    pub level: f32,
    /// The loudest the input has been for this whole recording.
    pub loudest: f32,
    /// The boost being applied, as a multiplier.
    pub gain: f32,
    /// How much audio the device has handed over, in the interface's compact
    /// duration form — or `None` while it is still opening and has handed over
    /// none.
    ///
    /// The difference is the whole of what the operator needs to know before
    /// they start speaking: until the device answers, a microphone that looks
    /// open is recording nothing.
    pub captured: Option<String>,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/recording.rs"]
mod rendering;

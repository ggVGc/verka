//! Launch and focused selection pickers, with the selection they would
//! confirm spelled out along the top border.

use crate::fuzzy_list::FuzzyList;

pub struct LauncherView {
    pub title: String,
    pub hint: String,
    /// The triple the picker would confirm, drawn in the title.
    pub selection: String,
    /// Whether a live session pins the agent. The list then holds only that
    /// agent's rows, and the title says why it is the only one on offer.
    pub provider_locked: bool,
    /// Labels for the choices, in the order the list offers them.
    pub rows: Vec<String>,
    /// The query typed at the list and the cursor among what it left standing.
    pub list: FuzzyList,
    pub effort: Option<EffortView>,
}

pub struct EffortView {
    pub rows: Vec<String>,
    pub list: FuzzyList,
    pub focused: bool,
}

/// What the list shows when there is nothing to launch at all. Unreachable
/// while either agent has a catalog, but a blank box would read as a bug.
pub const NO_ROWS: &str = "no matching choices";

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/launcher.rs"]
mod rendering;

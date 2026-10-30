use crate::fuzzy_list::FuzzyList;

#[derive(Clone, Copy)]
pub struct BranchPromptView {
    pub selected: usize,
    pub history: styra_protocol::BranchHistory,
    pub checkout: styra_protocol::BranchCheckout,
}

#[derive(Clone, Copy)]
pub struct LinkMenuView<'a> {
    /// The link the actions apply to, as the reply wrote it.
    pub destination: &'a str,
    pub actions: &'a [&'a str],
    pub selected: usize,
}

#[derive(Clone, Copy)]
pub struct ActionMenuView<'a> {
    /// The window whose actions these are, which the menu covers.
    pub window: &'a str,
    /// Each action with its keys, in the order the reference lists them.
    pub rows: &'a [String],
    pub list: &'a FuzzyList,
}

#[derive(Clone, Copy)]
pub struct WorkspaceJumpView<'a> {
    /// Each Workspace with live work, with how many of its Interactions are.
    pub rows: &'a [String],
    pub list: &'a FuzzyList,
}

#[derive(Clone, Copy)]
pub struct TagPickerView<'a> {
    pub available: &'a [String],
    pub selected: &'a [String],
    pub list: &'a FuzzyList,
    pub new_tag: Option<&'a str>,
    /// Choosing the navigator's tag filter rather than editing tags: the
    /// same list, but nothing new can be added to it.
    pub filter: bool,
}

/// A file citation displayed by the references overlay.
#[derive(Clone, Copy)]
pub struct ReferenceView<'a> {
    pub label: &'a str,
    pub line: Option<u32>,
}

/// Presentation data for the file-reference chooser.
#[derive(Clone, Copy)]
pub struct ReferencesView<'a> {
    pub items: &'a [ReferenceView<'a>],
    pub selected: usize,
}

/// The visible state of the path-insertion prompt.
#[derive(Clone)]
pub enum InsertPromptView<'a> {
    Typing(&'a str),
    /// The host path no mount carries, and whether granting it restarts the
    /// interaction — true for an idle one, whose sandbox has to be relaunched
    /// to take a new mount.
    Grant {
        host: String,
        restarts: bool,
    },
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/overlays.rs"]
mod rendering;

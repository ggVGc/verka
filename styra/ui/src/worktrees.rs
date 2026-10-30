//! The worktree picker: every linked checkout Styra knows of, one row per
//! Session working in it, narrowed as it is typed at. What the cursor is on is
//! spelled out beneath the list, because a row has room for the checkout's
//! name and the Session's, and not for the path or the branch.

use crate::fuzzy_list::FuzzyList;

pub struct WorktreePickerView<'a> {
    /// Which Workspaces the list covers, drawn in the title.
    pub scope: &'a str,
    /// One row per Session in a worktree, and one for a worktree no Session
    /// records, in the order the caller grouped them.
    pub rows: &'a [String],
    /// The query typed at the list and the cursor among what it left standing.
    pub list: &'a FuzzyList,
    /// What the row under the cursor is, one `(label, value)` line each.
    pub detail: &'a [(&'a str, String)],
    /// A one-off answer to the last key, such as why Enter did nothing.
    pub notice: Option<&'a str>,
}

/// What the list says when no Session in scope has a worktree.
pub const NO_ROWS: &str = "no session has a worktree";

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/worktrees.rs"]
mod rendering;

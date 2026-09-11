//! The main-view choice of how a selected entry seeds a new Session.

use styra_protocol::BranchHistory;

/// The open branch chooser. The timestamp fixes the source entry at the
/// moment the chooser opens, so later redraws cannot move the branch point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchPrompt {
    at_ms: u64,
    selected: BranchHistory,
}

impl BranchPrompt {
    pub fn new(at_ms: u64) -> Self {
        Self {
            at_ms,
            selected: BranchHistory::ThroughSelected,
        }
    }

    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }

    pub fn selected(&self) -> BranchHistory {
        self.selected
    }

    pub fn selected_index(&self) -> usize {
        match self.selected {
            BranchHistory::ThroughSelected => 0,
            BranchHistory::SelectedOnly => 1,
        }
    }

    pub fn select_next(&mut self) {
        self.selected = match self.selected {
            BranchHistory::ThroughSelected => BranchHistory::SelectedOnly,
            BranchHistory::SelectedOnly => BranchHistory::ThroughSelected,
        };
    }

    pub fn select_previous(&mut self) {
        self.select_next();
    }
}

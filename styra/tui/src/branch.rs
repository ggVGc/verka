//! A branch point and its independently composed first message.

use crate::composer::Composer;
use styra_protocol::{agent::Selection, BranchHistory};

pub struct BranchPrompt {
    at_ms: u64,
    history: BranchHistory,
    pub composer: Composer,
    pub selection: Selection,
    pub contract: Option<styra_protocol::Contract>,
    /// Whether the message was carried over from the main message box, which
    /// gets it back if the branch is not made.
    pub from_input: bool,
}

impl BranchPrompt {
    pub fn new(at_ms: u64, selection: Selection) -> Self {
        Self {
            at_ms,
            history: BranchHistory::ThroughSelected,
            composer: Composer::default(),
            selection,
            contract: None,
            from_input: false,
        }
    }

    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }
    pub fn history(&self) -> BranchHistory {
        self.history
    }
    pub fn change(&mut self) {
        self.history = match self.history {
            BranchHistory::ThroughSelected => BranchHistory::SelectedOnly,
            BranchHistory::SelectedOnly => BranchHistory::ThroughSelected,
        };
    }
}

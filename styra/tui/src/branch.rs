//! Independent history and checkout choices for a branched Session.

use styra_protocol::{BranchCheckout, BranchHistory};

/// The source timestamp stays fixed while the branch dialog is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchPrompt {
    at_ms: u64,
    history: BranchHistory,
    checkout: BranchCheckout,
    selected: usize,
}

impl BranchPrompt {
    pub fn new(at_ms: u64, has_checkout: bool) -> Self {
        Self {
            at_ms,
            history: BranchHistory::ThroughSelected,
            checkout: if has_checkout {
                BranchCheckout::NewWorktree
            } else {
                BranchCheckout::ShareSource
            },
            selected: 0,
        }
    }

    pub fn at_ms(&self) -> u64 {
        self.at_ms
    }
    pub fn history(&self) -> BranchHistory {
        self.history
    }
    pub fn checkout(&self) -> BranchCheckout {
        self.checkout
    }
    pub fn selected_index(&self) -> usize {
        self.selected
    }
    pub fn select_next(&mut self) {
        self.selected = 1 - self.selected;
    }
    pub fn select_previous(&mut self) {
        self.select_next();
    }

    pub fn change(&mut self) {
        if self.selected == 0 {
            self.history = match self.history {
                BranchHistory::ThroughSelected => BranchHistory::SelectedOnly,
                BranchHistory::SelectedOnly => BranchHistory::ThroughSelected,
            };
        } else {
            self.checkout = match self.checkout {
                BranchCheckout::ShareSource => BranchCheckout::NewWorktree,
                BranchCheckout::NewWorktree => BranchCheckout::ShareSource,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_and_checkout_can_be_chosen_independently() {
        let mut prompt = BranchPrompt::new(42, true);
        assert_eq!(prompt.checkout(), BranchCheckout::NewWorktree);
        prompt.change();
        prompt.select_next();
        prompt.change();
        assert_eq!(prompt.history(), BranchHistory::SelectedOnly);
        assert_eq!(prompt.checkout(), BranchCheckout::ShareSource);
        assert_eq!(prompt.at_ms(), 42);
        prompt.select_previous();
        prompt.change();
        assert_eq!(prompt.history(), BranchHistory::ThroughSelected);
        assert_eq!(prompt.checkout(), BranchCheckout::ShareSource);
        assert_eq!(
            BranchPrompt::new(42, false).checkout(),
            BranchCheckout::ShareSource
        );
    }
}

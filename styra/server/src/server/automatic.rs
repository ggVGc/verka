//! Automatic actions: work the server applies to an interaction on the
//! operator's behalf, which may run turns in it, such as resolving a rebase's
//! conflicts. While one is applied the interaction belongs to it, so no client
//! may act on the interaction until it finishes.
use std::collections::HashMap;
use std::sync::Mutex;

/// What is being applied to an interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutomaticAction {
    /// Rebasing onto the interaction's source branch.
    Rebase,
    /// Moving the interaction onto another branch, or back to a saved one.
    Retarget,
    /// Rebasing, squashing and landing the interaction on its source branch.
    Integration,
}

impl std::fmt::Display for AutomaticAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rebase => "rebase",
            Self::Retarget => "retarget",
            Self::Integration => "integration",
        })
    }
}

/// The automatic action applied to each interaction that has one.
#[derive(Default)]
pub(crate) struct AutomaticActions(Mutex<HashMap<String, AutomaticAction>>);

impl AutomaticActions {
    /// Apply `action` to `id`, refused while another is applied to it.
    pub(crate) fn begin(&self, id: &str, action: AutomaticAction) -> anyhow::Result<()> {
        let mut applied = self.0.lock().expect("automatic action lock poisoned");
        if let Some(current) = applied.get(id) {
            anyhow::bail!("{}", refusal(*current));
        }
        applied.insert(id.to_owned(), action);
        Ok(())
    }

    pub(crate) fn finish(&self, id: &str) {
        self.0
            .lock()
            .expect("automatic action lock poisoned")
            .remove(id);
    }

    pub(crate) fn applied(&self, id: &str) -> Option<AutomaticAction> {
        self.0
            .lock()
            .expect("automatic action lock poisoned")
            .get(id)
            .copied()
    }

    /// Refuse to act on `id` while an automatic action is applied to it.
    pub(crate) fn ensure_none(&self, id: &str) -> anyhow::Result<()> {
        match self.applied(id) {
            Some(action) => anyhow::bail!("{}", refusal(action)),
            None => Ok(()),
        }
    }

    /// Every interaction with an automatic action applied.
    pub(crate) fn interactions(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("automatic action lock poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

fn refusal(action: AutomaticAction) -> String {
    format!("an automatic {action} is being applied to this interaction; wait for it to finish")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_action_at_a_time_and_refusals_name_it() {
        let actions = AutomaticActions::default();
        actions.ensure_none("a").unwrap();
        actions.begin("a", AutomaticAction::Rebase).unwrap();
        let error = actions
            .begin("a", AutomaticAction::Integration)
            .unwrap_err()
            .to_string();
        assert!(error.contains("automatic rebase"), "{error}");
        assert!(actions.ensure_none("a").is_err());
        actions.ensure_none("b").unwrap();
        assert_eq!(actions.applied("a"), Some(AutomaticAction::Rebase));
        assert_eq!(actions.interactions(), vec!["a".to_owned()]);
        actions.finish("a");
        actions.ensure_none("a").unwrap();
        actions.begin("a", AutomaticAction::Integration).unwrap();
    }
}

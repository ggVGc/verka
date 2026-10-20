//! An interaction’s checkout diffed against its saved comparison branch,
//! integration target, or first recorded snapshot.

use styra_diff::DiffTarget;
use styra_protocol::InteractionSummary;
use styra_server::Client;

/// Resolve the baseline when diff mode opens: the server's merge base with
/// the saved comparison branch or integration target, or else the first
/// snapshot, including any work that was uncommitted when it was recorded.
pub fn target(client: &Client, interaction: &InteractionSummary) -> anyhow::Result<DiffTarget> {
    let checkout = interaction
        .checkout
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("this interaction has no Git checkout to diff"))?;
    let base = client.diff_base(&interaction.id)?.ok_or_else(|| {
        anyhow::anyhow!("this interaction has no target branch or snapshot to diff against")
    })?;
    Ok(DiffTarget {
        worktree: checkout.worktree.clone(),
        branch: checkout.branch.clone(),
        branch_point: base.branch.is_some(),
        base_branch: Some(base.branch.unwrap_or_else(|| "first snapshot".into())),
        base: base.base,
    })
}

/// A comparison preference belonging to the interaction, independent of its
/// integration target. The first row restores the automatic baseline.
pub struct BranchPicker {
    pub id: String,
    pub rows: Vec<String>,
    pub list: styra_ui::fuzzy_list::FuzzyList,
}

pub enum BranchChoice {
    Stay,
    Cancel,
    Select(Option<String>),
}

impl BranchPicker {
    pub fn new(id: String, branches: Vec<String>) -> Self {
        let mut rows = vec!["Automatic (integration target or first snapshot)".into()];
        rows.extend(branches);
        Self {
            id,
            rows,
            list: styra_ui::fuzzy_list::FuzzyList::default().keeping_order(),
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> BranchChoice {
        use crossterm::event::KeyCode;
        use styra_ui::query::Edit;
        match key.code {
            KeyCode::Esc => return BranchChoice::Cancel,
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_row(&self.rows) {
                    return BranchChoice::Select((index > 0).then(|| self.rows[index].clone()));
                }
            }
            KeyCode::Down => self.list.next(&self.rows),
            KeyCode::Up => self.list.prev(&self.rows),
            KeyCode::PageDown => self.list.page_down(&self.rows),
            KeyCode::PageUp => self.list.page_up(&self.rows),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    self.list.edit(edit);
                }
            }
        }
        BranchChoice::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    #[test]
    fn fuzzy_filter_preserves_recency_and_selects_the_matching_branch() {
        let mut picker = BranchPicker::new(
            "interaction".into(),
            vec!["z-new-topic".into(), "a-old-topic".into(), "other".into()],
        );
        for c in "tpc".chars() {
            picker.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
        let indices: Vec<_> = picker
            .list
            .matches(&picker.rows)
            .iter()
            .map(|m| m.index)
            .collect();
        assert_eq!(indices, vec![1, 2]);
        picker.handle_key(KeyEvent::from(KeyCode::Down));
        assert!(matches!(picker.handle_key(KeyEvent::from(KeyCode::Enter)),
            BranchChoice::Select(Some(branch)) if branch == "a-old-topic"));
        picker.list.query = "nonexistent".into();
        assert!(matches!(
            picker.handle_key(KeyEvent::from(KeyCode::Enter)),
            BranchChoice::Stay
        ));
        assert!(matches!(
            picker.handle_key(KeyEvent::from(KeyCode::Esc)),
            BranchChoice::Cancel
        ));
    }
}

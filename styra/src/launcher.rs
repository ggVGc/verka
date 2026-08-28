//! The launch picker: the agent, model, and reasoning effort the *next* session
//! will start with.
//!
//! State and column arithmetic only. [`App`](crate::app::App) carries an open
//! picker as `launcher`, [`crate::keys::handle_launcher_key`] drives it, and
//! [`crate::ui::launcher`] draws it.

use styra_server::agent::{Provider, Selection, PROVIDERS};

/// Which of the launch picker's three columns has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchColumn {
    Provider,
    Model,
    Effort,
}

/// Which row of a column holds `value`, falling back to the first. Used to open
/// a column on a provider's own declared default (see
/// [`Provider::default_model`]), so switching agents lands on that provider's
/// standard model and effort.
fn row_of<T: PartialEq>(rows: &[T], value: &T) -> usize {
    rows.iter().position(|row| row == value).unwrap_or(0)
}

/// The picker itself.
///
/// It edits a pending choice, not a running session — confirming it only records
/// the selection, and the operator's own first message still starts the agent.
/// Every row is a concrete choice out of the provider's own catalogs
/// ([`Provider::models`], [`Provider::efforts`]), and a [`Selection`] always pins
/// both, so there is nothing for a row meaning "whatever the agent is configured
/// for" to express. A newly chosen agent opens on the model and effort
/// the provider's declared defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launcher {
    pub column: LaunchColumn,
    pub provider: usize,
    /// An index into [`Provider::models`], then `carried_model` if there is one.
    pub model: usize,
    /// An index into [`Provider::efforts`].
    pub effort: usize,
    /// A model the picker does not offer but the session was nonetheless
    /// launched with. Shown as a final row so
    /// the operator can leave it selected; the picker cannot type one, only
    /// carry one it was opened on.
    pub carried_model: Option<String>,
    /// Models the operator has confirmed before, most recent first. They are
    /// listed ahead of the rest of the catalog, so the handful of models
    /// actually in use sit at the top of the column instead of wherever the
    /// catalog happens to put them. Models for other agents are kept in the
    /// list too and simply never match this provider's rows.
    pub recent_models: Vec<String>,
}

impl Launcher {
    /// Open the picker on `selection` — it always names a model and an effort, so
    /// there is always a row to open on. A model the provider's catalog does not
    /// list is carried as its own final row rather than dropped, so confirming
    /// the picker cannot silently change an existing selection.
    pub fn from_selection(selection: &Selection, recent_models: &[String]) -> Self {
        let provider = row_of(&PROVIDERS, &selection.provider);
        let models = selection.provider.models();
        // A model the catalog does not list is carried as an extra row rather
        // than falling back to the first.
        let carried_model = (!models.iter().any(|candidate| *candidate == selection.model))
            .then(|| selection.model.clone());
        let effort = row_of(selection.provider.efforts(), &selection.effort);
        let mut launcher = Self {
            column: LaunchColumn::Provider,
            provider,
            model: 0,
            effort,
            carried_model,
            recent_models: recent_models.to_vec(),
        };
        // Only now that the rows are ordered can the opening model be found:
        // recency decides where it sits.
        launcher.model = row_of(&launcher.models(), &selection.model);
        launcher
    }

    pub fn provider(&self) -> Provider {
        PROVIDERS[self.provider.min(PROVIDERS.len() - 1)]
    }

    /// What the picker currently describes. Every row is a concrete choice, so
    /// this is always a fully pinned selection; the clamps cover a row index that
    /// somehow outran its column rather than any "unset" state.
    pub fn selection(&self) -> Selection {
        let provider = self.provider();
        let models = self.models();
        let model = match models.get(self.model) {
            Some(model) => model.clone(),
            None => provider.default_model().to_owned(),
        };
        let efforts = provider.efforts();
        let effort = efforts
            .get(self.effort)
            .copied()
            .unwrap_or_else(|| provider.default_effort());
        Selection {
            provider,
            model,
            effort,
        }
    }

    /// The model column's rows: the provider's catalog, plus a carried model if
    /// the picker was opened on one, ordered most recently selected first.
    ///
    /// The sort is stable and only ranks models the operator has actually
    /// confirmed, so everything else keeps the catalog's own order — and a
    /// carried model, which the catalog does not list at all, stays last
    /// until it is selected once.
    pub fn models(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .provider()
            .models()
            .iter()
            .map(|model| (*model).to_owned())
            .collect();
        rows.extend(self.carried_model.clone());
        rows.sort_by_key(|row| {
            self.recent_models
                .iter()
                .position(|recent| recent == row)
                .unwrap_or(usize::MAX)
        });
        rows
    }

    /// How many rows the model column has.
    pub fn model_rows(&self) -> usize {
        self.provider().models().len() + usize::from(self.carried_model.is_some())
    }

    /// How many rows the focused column has.
    fn rows(&self) -> usize {
        match self.column {
            LaunchColumn::Provider => PROVIDERS.len(),
            LaunchColumn::Model => self.model_rows(),
            LaunchColumn::Effort => self.provider().efforts().len(),
        }
    }

    fn row(&mut self) -> &mut usize {
        match self.column {
            LaunchColumn::Provider => &mut self.provider,
            LaunchColumn::Model => &mut self.model,
            LaunchColumn::Effort => &mut self.effort,
        }
    }

    pub fn next(&mut self) {
        let last = self.rows() - 1;
        let row = self.row();
        *row = (*row + 1).min(last);
        self.after_move();
    }

    pub fn prev(&mut self) {
        let row = self.row();
        *row = row.saturating_sub(1);
        self.after_move();
    }

    fn after_move(&mut self) {
        // A model or effort chosen for the previous provider means nothing to the
        // new one — the ladders and catalogs differ — so both reset to that
        // agent's own opening rows rather than to whatever sits at the same
        // index. That includes a carried model, which belonged to the agent the
        // picker was opened on.
        if self.column == LaunchColumn::Provider {
            let provider = self.provider();
            self.effort = row_of(provider.efforts(), &provider.default_effort());
            self.carried_model = None;
            self.model = row_of(&self.models(), &provider.default_model().to_owned());
        }
    }

    pub fn next_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Model,
            LaunchColumn::Model => LaunchColumn::Effort,
            LaunchColumn::Effort => LaunchColumn::Provider,
        };
    }

    pub fn prev_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Effort,
            LaunchColumn::Model => LaunchColumn::Provider,
            LaunchColumn::Effort => LaunchColumn::Model,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_server::agent::Effort;

    /// With no row standing for "whatever the agent is configured for", every
    /// row of every column is a concrete choice — so whatever the picker is
    /// opened on, confirming it pins both a model and an effort.
    #[test]
    fn the_picker_always_pins_a_model_and_an_effort() {
        for provider in PROVIDERS {
            let mut launcher = Launcher::from_selection(&Selection::new(provider), &[]);
            // A selection always pins both, and the picker opens on the rows
            // naming them.
            let opened = launcher.selection();
            assert_eq!(opened.model, provider.default_model());
            assert_eq!(opened.effort, provider.default_effort());

            // And no reachable row in either column yields an absent value.
            for column in [LaunchColumn::Model, LaunchColumn::Effort] {
                launcher.column = column;
                for _ in 0..provider.models().len() + provider.efforts().len() {
                    let selection = launcher.selection();
                    assert!(
                        provider.models().contains(&selection.model.as_str()),
                        "{provider:?} {column:?} reached a model outside the catalog"
                    );
                    assert!(
                        provider.efforts().contains(&selection.effort),
                        "{provider:?} {column:?} reached an effort outside the ladder"
                    );
                    launcher.next();
                }
            }
        }
    }

    /// The models the operator actually uses head the column; the rest keep
    /// the catalog's own order, so the list does not reshuffle wholesale after
    /// a single pick.
    #[test]
    fn the_model_column_lists_recently_selected_models_first() {
        let catalog = Provider::Claude.models();
        let recent = vec![
            catalog[catalog.len() - 1].to_owned(),
            "gpt-5.6-sol".to_owned(), // another agent's model: never a row here
            catalog[1].to_owned(),
        ];
        let launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &recent);

        let rows = launcher.models();
        assert_eq!(rows[0], catalog[catalog.len() - 1]);
        assert_eq!(rows[1], catalog[1]);
        assert_eq!(
            rows[2..],
            catalog[..1]
                .iter()
                .chain(&catalog[2..catalog.len() - 1])
                .map(|model| (*model).to_owned())
                .collect::<Vec<_>>()[..],
            "the unused models keep the catalog's order"
        );
        // Ordering the rows does not change which one the picker opened on.
        assert_eq!(launcher.selection().model, Provider::Claude.default_model());
    }

    /// Switching agents drops the carried model with everything else: it named a
    /// model of the agent the picker was opened on.
    #[test]
    fn changing_provider_drops_a_carried_model() {
        let mut launcher = Launcher::from_selection(
            &Selection::parse("claude:claude-opus-4-1-20250805").unwrap(),
            &[],
        );
        assert!(launcher.carried_model.is_some());

        launcher.prev(); // in the provider column, back towards codex
        assert_eq!(launcher.carried_model, None);
        // The new agent's own declared default stands in for it.
        assert_eq!(
            launcher.selection().model,
            launcher.provider().default_model()
        );
        // And the column is back to just that agent's catalog.
        launcher.next_column();
        assert_eq!(launcher.model_rows(), launcher.provider().models().len());
    }

    /// The two agents' model catalogs and effort ladders are unrelated, so a
    /// choice made for one must not carry an index across to the other.
    #[test]
    fn changing_provider_falls_back_to_the_new_agents_defaults() {
        let mut launcher =
            Launcher::from_selection(&Selection::parse("claude:claude-opus-5/max").unwrap(), &[]);
        assert_eq!(launcher.selection().name(), "claude:claude-opus-5/max");

        launcher.prev(); // in the provider column, back towards codex
        let selection = launcher.selection();
        assert_ne!(selection.provider, Provider::Claude);
        // Neither the model nor the effort carries across by index: each falls
        // back to the new agent's own declared default.
        assert_eq!(selection.model, selection.provider.default_model());
        assert_eq!(selection.effort, selection.provider.default_effort());
        // And `max` is not offered at all under codex, so it cannot be reached
        // by walking the column either.
        assert!(!launcher.provider().efforts().contains(&Effort::Max));
    }
}

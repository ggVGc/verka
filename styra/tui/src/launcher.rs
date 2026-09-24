//! The launch picker: the agent, model, and reasoning effort the *next* session
//! will start with.
//!
//! State and column arithmetic only. [`App`](crate::app::App) carries an open
//! picker as `launcher`, [`crate::keys::handle_launcher_key`] drives it, and
//! [`crate::presentation::launcher`] draws it.

use styra_protocol::agent::{
    default_effort_for, efforts_for, models_for, Effort, Provider, Selection, PROVIDERS,
};

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
    /// Whether the agent column is out of reach. A live session's agent is the
    /// process itself and cannot be changed without converting the session, so
    /// while one is up the picker never gives the column the keys — it only
    /// shows which agent the session is running. Once nothing is running the
    /// column is a choice again: see [`crate::app::App::can_configure_launch`].
    pub provider_locked: bool,
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
    /// `provider_locked` says an agent process is up: the agent column is then
    /// shown but never focused, so no key can move the cursor onto a choice
    /// the session could not adopt anyway.
    pub fn from_selection(
        selection: &Selection,
        recent_models: &[String],
        provider_locked: bool,
    ) -> Self {
        let provider = row_of(&PROVIDERS, &selection.provider);
        let models = models_for(selection.provider);
        // A model the catalog does not list is carried as an extra row rather
        // than falling back to the first.
        let carried_model = (!models.iter().any(|candidate| *candidate == selection.model))
            .then(|| selection.model.clone());
        let effort = row_of(
            efforts_for(selection.provider, &selection.model),
            &selection.effort,
        );
        let mut launcher = Self {
            column: if provider_locked {
                LaunchColumn::Model
            } else {
                LaunchColumn::Provider
            },
            provider,
            model: 0,
            effort,
            carried_model,
            provider_locked,
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
        let effort = efforts_for(provider, &model)
            .get(self.effort)
            .copied()
            .unwrap_or_else(|| default_effort_for(provider, &model));
        Selection {
            provider,
            model,
            effort,
        }
    }

    /// The effort column's rows: the ladder the *currently selected model*
    /// accepts, which is why this is not a property of the agent column alone.
    /// Empty for a model that takes no effort setting — the column then has
    /// nothing to offer and the drawing code says so.
    pub fn efforts(&self) -> &'static [Effort] {
        efforts_for(self.provider(), &self.selection().model)
    }

    /// The model column's rows: the provider's catalog, plus a carried model if
    /// the picker was opened on one, ordered most recently selected first.
    ///
    /// The sort is stable and only ranks models the operator has actually
    /// confirmed, so everything else keeps the catalog's own order — and a
    /// carried model, which the catalog does not list at all, stays last
    /// until it is selected once.
    pub fn models(&self) -> Vec<String> {
        let mut rows: Vec<String> = models_for(self.provider())
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
        models_for(self.provider()).len() + usize::from(self.carried_model.is_some())
    }

    /// How many rows the focused column has. Never zero: a model with no effort
    /// ladder leaves that column empty, and a move within it is then a move
    /// within one nonexistent row rather than a division by it.
    fn rows(&self) -> usize {
        match self.column {
            LaunchColumn::Provider => PROVIDERS.len(),
            LaunchColumn::Model => self.model_rows(),
            LaunchColumn::Effort => self.efforts().len().max(1),
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
        let effort = self.selection().effort;
        let rows = self.rows();
        let row = self.row();
        *row = (*row + 1) % rows;
        self.after_move(effort);
    }

    pub fn prev(&mut self) {
        let effort = self.selection().effort;
        let rows = self.rows();
        let row = self.row();
        *row = (*row + rows - 1) % rows;
        self.after_move(effort);
    }

    /// Put the columns back in agreement after a move, given the effort that
    /// was selected before it. `held` is passed in because the effort *row* is
    /// an index into a ladder the move itself may have replaced.
    fn after_move(&mut self, held: Effort) {
        // A model or effort chosen for the previous provider means nothing to the
        // new one — the ladders and catalogs differ — so both reset to that
        // agent's own opening rows rather than to whatever sits at the same
        // index. That includes a carried model, which belonged to the agent the
        // picker was opened on.
        if self.column == LaunchColumn::Provider {
            let provider = self.provider();
            self.carried_model = None;
            self.model = row_of(&self.models(), &provider.default_model().to_owned());
            self.effort = row_of(
                self.efforts(),
                &default_effort_for(provider, &self.selection().model),
            );
        }
        // Models of one agent do not share a ladder either: `xhigh` is a rung
        // on Opus 4.7 and not on 4.6, so stepping down the model column must
        // not leave the effort row pointing at a rung the new model rejects.
        // The rung itself is kept where the new model has it, and the model's
        // own default stands in where it does not.
        if self.column == LaunchColumn::Model {
            let model = self.selection().model;
            let efforts = efforts_for(self.provider(), &model);
            let effort = if efforts.contains(&held) {
                held
            } else {
                default_effort_for(self.provider(), &model)
            };
            self.effort = row_of(efforts, &effort);
        }
    }

    /// Move the keys to `column` directly, without cycling through the ones
    /// between it and the current one. Used by the picker's per-column
    /// shortcuts (`p`/`m`/`e`), which name a column outright rather than
    /// stepping toward it.
    pub fn jump_to_column(&mut self, column: LaunchColumn) {
        self.column = column;
    }

    pub fn next_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Model,
            LaunchColumn::Model => LaunchColumn::Effort,
            // A locked agent column is skipped rather than landed on and
            // stepped off, so the cycle stays model → effort → model.
            LaunchColumn::Effort if self.provider_locked => LaunchColumn::Model,
            LaunchColumn::Effort => LaunchColumn::Provider,
        };
    }

    pub fn prev_column(&mut self) {
        self.column = match self.column {
            LaunchColumn::Provider => LaunchColumn::Effort,
            LaunchColumn::Model if self.provider_locked => LaunchColumn::Effort,
            LaunchColumn::Model => LaunchColumn::Provider,
            LaunchColumn::Effort => LaunchColumn::Model,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_protocol::agent::Effort;

    /// With no row standing for "whatever the agent is configured for", every
    /// row of every column is a concrete choice — so whatever the picker is
    /// opened on, confirming it pins both a model and an effort.
    #[test]
    fn the_picker_always_pins_a_model_and_an_effort() {
        for provider in PROVIDERS {
            let mut launcher = Launcher::from_selection(&Selection::new(provider), &[], false);
            // A selection always pins both, and the picker opens on the rows
            // naming them.
            let opened = launcher.selection();
            assert_eq!(opened.model, provider.default_model());
            assert_eq!(opened.effort, provider.default_effort());

            // And no reachable row in either column yields an absent value —
            // nor one the model it names would refuse.
            let catalog = models_for(provider);
            for column in [LaunchColumn::Model, LaunchColumn::Effort] {
                launcher.column = column;
                for _ in 0..catalog.len() + launcher.efforts().len() {
                    let selection = launcher.selection();
                    assert!(
                        catalog.contains(&selection.model.as_str()),
                        "{provider:?} {column:?} reached a model outside the catalog"
                    );
                    let efforts = efforts_for(provider, &selection.model);
                    assert!(
                        efforts.is_empty() || efforts.contains(&selection.effort),
                        "{provider:?} {column:?} reached an effort {} rejects",
                        selection.model
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
        let catalog = models_for(Provider::Claude);
        let recent = vec![
            catalog[catalog.len() - 1].to_owned(),
            "gpt-5.6-sol".to_owned(), // another agent's model: never a row here
            catalog[1].to_owned(),
        ];
        let launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &recent, false);

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

    /// The ladders differ between models of the *same* agent, so stepping down
    /// the model column has to retune the effort row: a rung the new model
    /// shares is kept, and one it does not have gives way to its own default.
    #[test]
    fn changing_model_keeps_a_shared_rung_and_drops_an_unshared_one() {
        let mut launcher = Launcher::from_selection(
            &Selection::parse("claude:claude-opus-4-7/xhigh").unwrap(),
            // Ordered so that stepping down the column walks 4.7 → 4.6 → 4.5.
            &[
                "claude-opus-4-7".into(),
                "claude-opus-4-6".into(),
                "claude-opus-4-5-20251101".into(),
            ],
            false,
        );
        launcher.column = LaunchColumn::Model;
        assert_eq!(launcher.selection().effort, Effort::XHigh);

        // Opus 4.6 has no `xhigh` — that rung arrived with 4.7 — so the row
        // cannot stay where it is.
        launcher.next();
        let selection = launcher.selection();
        assert_eq!(selection.model, "claude-opus-4-6");
        assert_eq!(
            selection.effort,
            default_effort_for(selection.provider, &selection.model)
        );

        // `high` is on every Claude ladder, so it survives the next step.
        while launcher.selection().effort != Effort::High {
            launcher.column = LaunchColumn::Effort;
            launcher.next();
            launcher.column = LaunchColumn::Model;
        }
        launcher.next();
        assert_eq!(launcher.selection().model, "claude-opus-4-5-20251101");
        assert_eq!(launcher.selection().effort, Effort::High);
    }

    /// A model that takes no effort setting leaves the column with no rows.
    /// Moving within it is then a no-op rather than an arithmetic fault.
    #[test]
    fn an_empty_effort_column_can_still_be_moved_in() {
        let mut launcher = Launcher::from_selection(
            &Selection::parse("claude:claude-haiku-4-5-20251001").unwrap(),
            &[],
            false,
        );
        launcher.column = LaunchColumn::Effort;
        assert!(launcher.efforts().is_empty());
        launcher.next();
        launcher.prev();
        assert_eq!(launcher.selection().model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn row_navigation_wraps_at_both_ends() {
        let mut launcher = Launcher::from_selection(&Selection::new(Provider::Codex), &[], false);
        launcher.column = LaunchColumn::Model;
        let rows = launcher.model_rows();

        launcher.model = rows - 1;
        launcher.next();
        assert_eq!(launcher.model, 0);

        launcher.prev();
        assert_eq!(launcher.model, rows - 1);
    }

    /// Switching agents drops the carried model with everything else: it named a
    /// model of the agent the picker was opened on.
    #[test]
    fn changing_provider_drops_a_carried_model() {
        let mut launcher = Launcher::from_selection(
            &Selection::parse("claude:claude-opus-4-1-20250805").unwrap(),
            &[],
            false,
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
        assert_eq!(launcher.model_rows(), models_for(launcher.provider()).len());
    }

    /// A live session's agent cannot be changed without converting the
    /// session, so the picker never gives that column the keys: it opens on the
    /// model column and no amount of column stepping, in either direction,
    /// reaches the agent one.
    #[test]
    fn a_live_session_cannot_move_the_cursor_onto_the_agent_column() {
        let mut launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &[], true);
        assert_eq!(launcher.column, LaunchColumn::Model);

        for _ in 0..6 {
            launcher.next_column();
            assert_ne!(launcher.column, LaunchColumn::Provider);
        }
        for _ in 0..6 {
            launcher.prev_column();
            assert_ne!(launcher.column, LaunchColumn::Provider);
        }
        // Both of the remaining columns are still reachable.
        launcher.next_column();
        assert_eq!(launcher.column, LaunchColumn::Effort);
        launcher.next_column();
        assert_eq!(launcher.column, LaunchColumn::Model);
        // And the agent the session is running is the one it stays on.
        assert_eq!(launcher.selection().provider, Provider::Claude);
    }

    /// The two agents' model catalogs and effort ladders are unrelated, so a
    /// choice made for one must not carry an index across to the other.
    #[test]
    fn changing_provider_falls_back_to_the_new_agents_defaults() {
        let mut launcher = Launcher::from_selection(
            &Selection::parse("claude:claude-opus-5/max").unwrap(),
            &[],
            false,
        );
        assert_eq!(launcher.selection().name(), "claude:claude-opus-5/max");

        launcher.prev(); // in the provider column, back towards codex
        let selection = launcher.selection();
        assert_ne!(selection.provider, Provider::Claude);
        // Neither the model nor the effort carries across by index: each falls
        // back to the new agent's own declared default.
        assert_eq!(selection.model, selection.provider.default_model());
        assert_eq!(
            selection.effort,
            default_effort_for(selection.provider, &selection.model)
        );
    }
}

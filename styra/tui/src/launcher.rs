//! The launch picker: the agent, model, and reasoning effort the *next* session
//! will start with.
//!
//! One list, not three columns. What a launch needs is a whole
//! `agent:model/effort` triple, and the columns made the operator assemble one
//! out of three cursors — while the constraint between them (a rung belongs to
//! a model, not to an agent) had to be enforced move by move. Offering the
//! triples themselves makes every row a launchable selection by construction,
//! and the list narrows as it is typed at, so `chk45` reaches
//! `claude:claude-haiku-4-5-20251001` without a single step through the
//! catalog.
//!
//! [`App`](crate::app::App) carries an open picker as `launcher`; this module
//! owns both its state and keyboard handling. [`crate::presentation::launcher`]
//! draws it through [`styra_ui::fuzzy_list`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;
use styra_protocol::agent::{
    default_effort_for, efforts_for, models_for, supports_effort, Provider, Selection, PROVIDERS,
};
use styra_protocol::LogEntry;
use styra_ui::fuzzy_list::FuzzyList;

use crate::app::App;
use crate::keybindings as keys;
use crate::preferences;

/// Apply a key to the open launch picker.
///
/// Every printable key is a letter of the query — that is what makes this a
/// narrowing list rather than a list with a search in it — so the picker's own
/// commands are on keys no model name contains: Enter, Esc, the arrows, and
/// control chords.
pub fn handle_key(app: &mut App, key: KeyEvent, preferences_path: &Path) {
    let Some(launcher) = app.launcher.as_mut() else {
        return;
    };
    match key {
        k if keys::LAUNCHER_NEXT.matches(k) => launcher.next(),
        k if keys::LAUNCHER_PREV.matches(k) => launcher.prev(),
        k if keys::LAUNCHER_PAGE_DOWN.matches(k) => launcher.page_down(),
        k if keys::LAUNCHER_PAGE_UP.matches(k) => launcher.page_up(),
        k if keys::LAUNCHER_DELETE_WORD.matches(k) => launcher.delete_query_word(),
        k if keys::LAUNCHER_SELECT.matches(k) => confirm(app, preferences_path),
        k if keys::LAUNCHER_DEFAULT.matches(k) => {
            confirm(app, preferences_path);
            if let Err(error) = preferences::save_selection(preferences_path, &app.selection) {
                app.push_log(LogEntry::error(format!(
                    "could not save launch defaults: {error:#}"
                )));
            }
        }
        // Esc widens the list back out before it closes it: a query narrowed
        // too far is the common reason to press it, and abandoning the whole
        // picker to retype the choice is not what was meant.
        k if keys::LAUNCHER_CANCEL.matches(k) => {
            if launcher.is_filtering() {
                launcher.clear_query();
            } else {
                app.cancel_launcher();
            }
        }
        _ => match key.code {
            KeyCode::Backspace => launcher.type_query(None),
            // A chord this picker has no command for is still not a letter
            // of a model's name, so it is ignored rather than typed.
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                launcher.type_query(Some(character));
            }
            _ => {}
        },
    }
}

fn confirm(app: &mut App, preferences_path: &Path) {
    app.confirm_launcher();
    if let Err(error) = preferences::save_recent_models(preferences_path, &app.recent_models) {
        app.push_log(LogEntry::error(format!(
            "could not save the model ordering: {error:#}"
        )));
    }
}

/// What a row of the picker says, and what its title spells out.
///
/// [`Selection::name`] always names a rung, because a selection always carries
/// one. For a model that takes no effort setting that rung is a placeholder no
/// launch sends, so the label drops it rather than advertising it.
pub fn label(selection: &Selection) -> String {
    if supports_effort(selection.provider, &selection.model) {
        selection.name()
    } else {
        format!("{}:{}", selection.provider.as_str(), selection.model)
    }
}

/// The picker itself.
///
/// It edits a pending choice, not a running session — confirming it only
/// records the selection, and the operator's own first message still starts the
/// agent. Every row is a whole [`Selection`] built out of the providers' own
/// catalogs and ladders, so no reachable row names a combination an agent would
/// refuse, and there is nothing for a row meaning "whatever the agent is
/// configured for" to express.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launcher {
    /// Every launchable triple, in the order the list offers them. Fixed for
    /// the life of the picker: the catalogs do not change while it is open, and
    /// narrowing is the query's job, not the row set's.
    rows: Vec<Selection>,
    /// The query typed at the list, and which of the rows it leaves standing
    /// the cursor is on.
    pub list: FuzzyList,
    /// What the picker was opened on. It is also what a query matching nothing
    /// falls back to: with no row under the cursor, the selection that stands
    /// is the one that already stood.
    opened_on: Selection,
    /// Whether the agent is out of reach. A live session's agent is the process
    /// itself and cannot be changed without converting the session, so while one
    /// is up the list offers that agent's triples and no others. Once nothing is
    /// running every agent is a choice again: see
    /// [`crate::app::App::can_configure_launch`].
    pub provider_locked: bool,
    /// Models the operator has confirmed before, most recent first. Their rows
    /// lead the list, so the handful of models actually in use sit at the top
    /// instead of wherever the catalogs happen to put them.
    pub recent_models: Vec<String>,
}

impl Launcher {
    /// Open the picker on `selection` — it always names a model and an effort,
    /// so there is always a row to open on. A selection the catalogs do not
    /// offer is carried as its own row rather than dropped, so confirming the
    /// picker cannot silently change an existing selection.
    pub fn from_selection(
        selection: &Selection,
        recent_models: &[String],
        provider_locked: bool,
    ) -> Self {
        let providers: Vec<Provider> = if provider_locked {
            vec![selection.provider]
        } else {
            PROVIDERS.to_vec()
        };
        let mut rows: Vec<Selection> = providers
            .iter()
            .flat_map(|provider| triples(*provider))
            .collect();
        // A selection the catalogs do not offer — a model retired out of them,
        // or a rung a model has since dropped — is still what the session was
        // launched with, so it gets a row of its own.
        if !rows.contains(selection) {
            rows.push(selection.clone());
        }
        // The models in actual use lead the list; everything else keeps the
        // catalogs' own order, so the list does not reshuffle wholesale after a
        // single pick. The sort is stable, which is what holds each model's
        // rungs together in ladder order beneath it.
        rows.sort_by_key(|row| {
            recent_models
                .iter()
                .position(|recent| *recent == row.model)
                .unwrap_or(usize::MAX)
        });
        let labels = labels_of(&rows);
        let at = rows
            .iter()
            .position(|row| row == selection)
            .unwrap_or_default();
        Self {
            list: FuzzyList::at(&labels, at),
            rows,
            opened_on: selection.clone(),
            provider_locked,
            recent_models: recent_models.to_vec(),
        }
    }

    /// The rows, as the list shows and matches them.
    pub fn labels(&self) -> Vec<String> {
        labels_of(&self.rows)
    }

    /// What the picker currently describes. Every row is a whole selection, so
    /// this needs no assembling; a query matching nothing leaves no row under
    /// the cursor, and the selection the picker opened on stands.
    pub fn selection(&self) -> Selection {
        self.list
            .selected_row(&self.labels())
            .and_then(|row| self.rows.get(row))
            .cloned()
            .unwrap_or_else(|| self.opened_on.clone())
    }

    /// Step the cursor down the rows the query left standing, holding at the
    /// last of them.
    pub fn next(&mut self) {
        let labels = self.labels();
        self.list.next(&labels);
    }

    /// Step the cursor up the rows the query left standing, holding at the
    /// first of them.
    pub fn prev(&mut self) {
        let labels = self.labels();
        self.list.prev(&labels);
    }

    /// Move the cursor a page down the rows the query left standing. The list
    /// is every triple both agents offer, which is long enough that stepping
    /// is not the only way across it.
    pub fn page_down(&mut self) {
        let labels = self.labels();
        self.list.page_down(&labels);
    }

    /// Move the cursor a page up the rows the query left standing.
    pub fn page_up(&mut self) {
        let labels = self.labels();
        self.list.page_up(&labels);
    }

    /// Take a character into the query, or `None` to drop the last one.
    pub fn type_query(&mut self, character: Option<char>) {
        match character {
            Some(character) => self.list.push(character),
            None => self.list.backspace(),
        }
    }

    /// Drop the last word of the query, where a word is one part of a name:
    /// `claude:claude-opus-5/high` back to `claude:claude-opus-5/`. Typing a
    /// triple is typing an identifier, and a mistyped rung should not cost the
    /// agent and model in front of it.
    pub fn delete_query_word(&mut self) {
        self.list.delete_word();
    }

    /// Whether anything has been typed.
    pub fn is_filtering(&self) -> bool {
        self.list.is_filtering()
    }

    /// Widen the list back out, leaving the cursor on the row it had narrowed
    /// down to rather than at the top of the list it returns to.
    pub fn clear_query(&mut self) {
        let labels = self.labels();
        self.list.clear(&labels);
    }
}

/// Every launchable triple of one agent: its catalog crossed with each model's
/// own ladder. A model that takes no effort setting still gets exactly one row
/// — a selection carries a rung whether or not the launch sends it, and
/// [`label`] is what keeps that placeholder off the screen.
fn triples(provider: Provider) -> Vec<Selection> {
    models_for(provider)
        .iter()
        .flat_map(|model| {
            let efforts = efforts_for(provider, model);
            let rungs: Vec<_> = if efforts.is_empty() {
                vec![default_effort_for(provider, model)]
            } else {
                efforts.to_vec()
            };
            rungs.into_iter().map(move |effort| Selection {
                provider,
                model: (*model).to_owned(),
                effort,
            })
        })
        .collect()
}

fn labels_of(rows: &[Selection]) -> Vec<String> {
    rows.iter().map(label).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_protocol::agent::{validate_selection, Effort};

    fn opened(selection: &str) -> Launcher {
        Launcher::from_selection(&Selection::parse(selection).unwrap(), &[], false)
    }

    fn typed(launcher: &mut Launcher, query: &str) {
        for character in query.chars() {
            launcher.type_query(Some(character));
        }
    }

    /// Why the list is of triples rather than of three columns: no row can name
    /// a combination an agent would refuse, so there is no move to get wrong.
    #[test]
    fn every_row_is_a_launchable_selection() {
        let mut launcher = opened("claude");
        for row in 0..launcher.labels().len() {
            launcher.list.selected = row;
            let selection = launcher.selection();
            validate_selection(&selection).unwrap_or_else(|error| {
                panic!("{} is not launchable: {error:#}", selection.name())
            });
        }
    }

    /// Both agents' catalogs are on offer at once — that is what a flat list
    /// buys — and each model's own ladder is under it.
    #[test]
    fn the_list_offers_every_agents_models_and_their_own_rungs() {
        let launcher = opened("claude");
        let labels = launcher.labels();
        for provider in PROVIDERS {
            for model in models_for(provider) {
                assert!(
                    labels.iter().any(|label| label.contains(model)),
                    "{model} is not on offer: {labels:?}"
                );
            }
        }
        // A model with a ladder gets one row per rung, and one with no ladder
        // gets a single row that does not advertise a placeholder.
        let opus = labels
            .iter()
            .filter(|label| label.starts_with("claude:claude-opus-5/"))
            .count();
        assert_eq!(opus, efforts_for(Provider::Claude, "claude-opus-5").len());
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.contains("claude-haiku-4-5-20251001"))
                .collect::<Vec<_>>(),
            vec!["claude:claude-haiku-4-5-20251001"]
        );
    }

    #[test]
    fn the_picker_opens_on_the_selection_it_was_given() {
        for name in [
            "claude:claude-opus-5/max",
            "codex:gpt-5.6-sol/minimal",
            "claude:claude-haiku-4-5-20251001",
        ] {
            let launcher = opened(name);
            assert_eq!(label(&launcher.selection()), name);
        }
    }

    /// The point of the thing: a few letters of a triple's name reach it,
    /// without the separators between them being typed.
    #[test]
    fn typing_narrows_the_list_to_the_triple_named() {
        let mut launcher = opened("claude");
        typed(&mut launcher, "chk45");
        let selection = launcher.selection();
        assert_eq!(selection.provider, Provider::Claude);
        assert_eq!(selection.model, "claude-haiku-4-5-20251001");

        // Down to the rung: the effort is part of what is being typed at.
        let mut launcher = opened("claude");
        typed(&mut launcher, "opus-5/max");
        assert_eq!(label(&launcher.selection()), "claude:claude-opus-5/max");
    }

    /// A query can cross the agent boundary, which is the whole point of one
    /// list: reaching another agent's model no longer means selecting the agent
    /// first.
    #[test]
    fn a_query_reaches_another_agents_model_directly() {
        let mut launcher = opened("claude:claude-opus-5/max");
        assert_eq!(launcher.selection().provider, Provider::Claude);
        typed(&mut launcher, "codex:");
        assert_eq!(launcher.selection().provider, Provider::Codex);
    }

    /// The models the operator actually uses lead the list; the rest keep the
    /// catalogs' order, and a model's rungs stay together beneath it.
    #[test]
    fn recently_selected_models_lead_the_list() {
        let catalog = models_for(Provider::Claude);
        let recent = vec![catalog[catalog.len() - 1].to_owned(), catalog[1].to_owned()];
        let launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &recent, false);

        let models: Vec<String> = launcher
            .labels()
            .iter()
            .map(|label| {
                label
                    .split(':')
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .expect("a labelled model")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            models.first().map(String::as_str),
            Some(catalog[catalog.len() - 1])
        );
        // Every rung of the most recent model comes before the next model's.
        let first_run = models
            .iter()
            .take_while(|model| *model == catalog[catalog.len() - 1])
            .count();
        assert_eq!(
            models[first_run], catalog[1],
            "the second most recent model follows the first's rungs"
        );
        // Ordering the rows does not change which one the picker opened on.
        assert_eq!(launcher.selection().model, Provider::Claude.default_model());
    }

    /// A live session's agent is the process itself, so while one is up the
    /// list offers that agent's triples and no others — no query can reach a
    /// row the session could not adopt.
    #[test]
    fn a_locked_agent_leaves_only_its_own_rows() {
        let mut launcher = Launcher::from_selection(&Selection::new(Provider::Claude), &[], true);
        for label in launcher.labels() {
            assert!(label.starts_with("claude:"), "{label}");
        }
        // Even typing another agent's name reaches nothing.
        typed(&mut launcher, "codex");
        assert!(launcher.list.matches(&launcher.labels()).is_empty());
        assert_eq!(launcher.selection().provider, Provider::Claude);
    }

    /// A selection the catalogs no longer offer is still what the session was
    /// launched with, so the picker carries it as a row instead of quietly
    /// relaunching on something else.
    #[test]
    fn a_selection_outside_the_catalogs_is_carried_as_its_own_row() {
        let carried = Selection::parse("claude:claude-opus-4-1-20250805").unwrap();
        let launcher = Launcher::from_selection(&carried, &[], false);
        assert_eq!(launcher.selection(), carried);
        assert_eq!(
            launcher
                .labels()
                .iter()
                .filter(|label| label.contains("claude-opus-4-1-20250805"))
                .count(),
            1
        );
    }

    /// A query matching nothing leaves no row under the cursor. Confirming then
    /// cannot invent one, so what the picker opened on is what stands.
    #[test]
    fn an_over_narrow_query_leaves_the_opening_selection_standing() {
        let mut launcher = opened("claude:claude-opus-5/max");
        typed(&mut launcher, "zzzz");
        assert!(launcher.list.matches(&launcher.labels()).is_empty());
        // And moving within nothing is a no-op rather than an arithmetic fault.
        launcher.next();
        launcher.prev();
        assert_eq!(label(&launcher.selection()), "claude:claude-opus-5/max");
    }

    /// Widening the list back out keeps the row the query had found, so
    /// clearing a query is not also losing the choice it reached.
    #[test]
    fn clearing_the_query_keeps_the_row_it_reached() {
        let mut launcher = opened("claude");
        typed(&mut launcher, "haiku");
        let found = launcher.selection();

        launcher.clear_query();
        assert!(!launcher.is_filtering());
        assert_eq!(
            launcher.list.matches(&launcher.labels()).len(),
            launcher.labels().len()
        );
        assert_eq!(launcher.selection(), found);
    }

    /// A mistyped rung costs the rung, not the agent and model in front of
    /// it.
    #[test]
    fn deleting_a_word_takes_back_one_part_of_the_triple() {
        let mut launcher = opened("claude");
        typed(&mut launcher, "claude:claude-opus-5/mox");
        assert!(launcher.list.matches(&launcher.labels()).is_empty());

        launcher.delete_query_word();
        assert_eq!(launcher.list.query, "claude:claude-opus-5/");
        assert_eq!(launcher.selection().model, "claude-opus-5");
    }

    /// The list is long enough to page through, and a page lands on a row
    /// rather than running off the end of it.
    #[test]
    fn paging_crosses_the_list_and_stops_at_its_ends() {
        let mut launcher = opened("claude");
        assert!(
            launcher.labels().len() > styra_ui::fuzzy_list::PAGE,
            "the list is worth paging"
        );

        let opened_at = launcher.list.selected;
        launcher.page_down();
        assert_eq!(
            launcher.list.selected,
            opened_at + styra_ui::fuzzy_list::PAGE
        );
        for _ in 0..launcher.labels().len() {
            launcher.page_down();
        }
        assert_eq!(launcher.list.selected, launcher.labels().len() - 1);
        validate_selection(&launcher.selection()).expect("still a launchable row");

        for _ in 0..launcher.labels().len() {
            launcher.page_up();
        }
        assert_eq!(launcher.list.selected, 0);
    }

    /// The cursor holds at the ends of the list rather than wrapping round
    /// them: the top is where the best match of a query sits, and stepping off
    /// the bottom must not land on it.
    #[test]
    fn row_navigation_holds_at_both_ends() {
        let mut launcher = opened("codex");
        let rows = launcher.labels().len();

        launcher.list.selected = rows - 1;
        launcher.next();
        assert_eq!(launcher.list.selected, rows - 1);

        launcher.list.selected = 0;
        launcher.prev();
        assert_eq!(launcher.list.selected, 0);
    }

    /// The rung is part of the row, so a model without `xhigh` simply has no
    /// `xhigh` row — there is no cursor left that could point at one.
    #[test]
    fn a_model_only_offers_the_rungs_it_has() {
        let launcher = opened("claude");
        let labels = launcher.labels();
        assert!(labels.contains(&"claude:claude-opus-5/xhigh".to_owned()));
        assert!(
            !labels.contains(&"claude:claude-opus-4-6/xhigh".to_owned()),
            "that rung arrived after 4.6"
        );
        assert!(labels.contains(&"claude:claude-opus-4-6/high".to_owned()));
    }

    #[test]
    fn efforts_are_offered_in_ladder_order() {
        let launcher = opened("claude");
        let rungs: Vec<Effort> = launcher
            .labels()
            .iter()
            .zip(&launcher.rows)
            .filter(|(label, _)| label.starts_with("claude:claude-opus-5/"))
            .map(|(_, row)| row.effort)
            .collect();
        assert_eq!(rungs, efforts_for(Provider::Claude, "claude-opus-5"));
    }
}

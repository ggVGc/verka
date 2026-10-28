//! The launch picker: the agent, model, and reasoning effort the *next* session
//! will start with.
//!
//! The full launcher offers whole `agent:model/effort` triples. The message
//! editor's shortcuts open a provider list or separate model and effort lists
//! in one selector. Each list keeps its own filter; typing edits the list most
//! recently touched. Navigation only previews a selection; Enter applies it.
//!
//! [`App`](crate::app::App) carries an open picker as `launcher`; this module
//! owns both its state and keyboard handling. [`crate::presentation::launcher`]
//! draws it through [`styra_ui::fuzzy_list`].

use crossterm::event::KeyEvent;
use std::path::Path;
use styra_protocol::agent::{Effort, Provider, Selection, PROVIDERS};
use styra_protocol::LogEntry;
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::Edit;

use crate::app::App;
use crate::keybindings as keys;
use crate::preferences;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Launch,
    Provider,
    ModelEffort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Model,
    Effort,
}

impl Choice {
    pub fn title(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Provider => "provider",
            Self::ModelEffort => "model / effort",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Launch => " type to narrow · ? keys ",
            Self::Provider => " ctrl-p/↓ next · type filter · Enter select · Esc cancel ",
            Self::ModelEffort => " ctrl-o/e next · type filter · Enter select · Esc cancel ",
        }
    }

    fn repeats(self, key: KeyEvent) -> bool {
        match self {
            Self::Launch => false,
            Self::Provider => keys::EDITOR_CYCLE_PROVIDER.matches(key),
            Self::ModelEffort => {
                keys::EDITOR_CYCLE_MODEL.matches(key) || keys::EDITOR_CYCLE_EFFORT.matches(key)
            }
        }
    }
}

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
        k if launcher.choice == Choice::ModelEffort && keys::EDITOR_CYCLE_MODEL.matches(k) => {
            launcher.active = Field::Model;
            launcher.cycle_next();
        }
        k if launcher.choice == Choice::ModelEffort && keys::EDITOR_CYCLE_EFFORT.matches(k) => {
            launcher.active = Field::Effort;
            launcher.cycle_next();
        }
        k if launcher.choice.repeats(k) => launcher.cycle_next(),
        k if keys::LAUNCHER_NEXT.matches(k) => launcher.next(),
        k if keys::LAUNCHER_PREV.matches(k) => launcher.prev(),
        k if keys::LAUNCHER_PAGE_DOWN.matches(k) => launcher.page_down(),
        k if keys::LAUNCHER_PAGE_UP.matches(k) => launcher.page_up(),
        k if keys::LAUNCHER_SELECT.matches(k) => confirm(app, preferences_path),
        k if keys::LAUNCHER_DEFAULT.matches(k) && launcher.choice == Choice::Launch => {
            confirm(app, preferences_path);
            if let Err(error) = preferences::save_selection(preferences_path, &app.selection) {
                app.push_log(LogEntry::error(format!(
                    "could not save launch defaults: {error:#}"
                )));
            }
        }
        k if keys::LAUNCHER_CANCEL.matches(k) => {
            if launcher.choice == Choice::Launch && launcher.is_filtering() {
                launcher.clear_query();
            } else {
                app.cancel_launcher();
            }
        }
        // A chord this picker has no command for is still not a letter of a
        // model's name, so it is ignored rather than typed.
        _ => {
            if let Some(edit) = Edit::from_key(key) {
                launcher.type_query(edit);
            }
        }
    }
}

fn confirm(app: &mut App, preferences_path: &Path) {
    let Some(launcher) = app.launcher.as_ref() else {
        return;
    };
    if !launcher.has_selection() {
        return;
    }
    // A provider conversion keeps app.selection unchanged until the server
    // answers, but the confirmed model is already the one to remember.
    let model = launcher.selection().model;
    app.confirm_launcher();
    if let Err(error) = preferences::record_recent_model(preferences_path, &model) {
        app.push_log(LogEntry::error(format!(
            "could not save the model ordering: {error:#}"
        )));
    }
    app.recent_models = preferences::load_recent_models(preferences_path);
}

/// What a row of the picker says, and what its title spells out.
///
/// [`Selection::name`] always names a rung, because a selection always carries
/// one. For a model that takes no effort setting that rung is a placeholder no
/// launch sends, so the label drops it rather than advertising it.
pub fn label(selection: &Selection) -> String {
    if selection.provider.supports_effort(&selection.model) {
        selection.name()
    } else {
        format!("{}:{}", selection.provider.as_str(), selection.model)
    }
}

/// The picker itself.
///
/// Navigation edits a pending choice. The focused model/effort selector keeps
/// separate cursors and queries, with the effort list derived from the selected
/// model. The full launcher and provider picker use one list of selections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launcher {
    pub choice: Choice,
    pub active: Field,
    pub effort_list: FuzzyList,
    fallback_effort: Effort,
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
        let mut rows: Vec<Selection> = styra_server::model_picker::catalog(recent_models)
            .into_iter()
            .filter(|model| providers.contains(&model.provider))
            .flat_map(|model| {
                let efforts = if model.efforts.is_empty() {
                    vec![model.default_effort]
                } else {
                    model.efforts
                };
                efforts.into_iter().map(move |effort| Selection {
                    provider: model.provider,
                    model: model.model.clone(),
                    effort,
                })
            })
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
        styra_server::model_picker::sort(&mut rows, recent_models, |row| &row.model);
        let labels = labels_of(&rows);
        let at = rows
            .iter()
            .position(|row| row == selection)
            .unwrap_or_default();
        Self {
            choice: Choice::Launch,
            active: Field::Model,
            effort_list: FuzzyList::default(),
            fallback_effort: selection.effort,
            // Filtering keeps the recency order above rather than ranking by
            // match, so the models in use stay first however little is typed.
            list: FuzzyList::at(&labels, at).keeping_order(),
            rows,
            opened_on: selection.clone(),
            provider_locked,
            recent_models: recent_models.to_vec(),
        }
    }

    /// The rows, as the list shows and matches them.
    pub fn labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| match self.choice {
                Choice::Launch | Choice::Provider => label(row),
                Choice::ModelEffort => row.model.clone(),
            })
            .collect()
    }

    /// A provider or model/effort picker; editing it never applies a change.
    pub fn for_choice(selection: &Selection, choice: Choice, live: bool) -> Self {
        let provider = selection.provider;
        let mut rows: Vec<Selection> = match choice {
            Choice::Provider => PROVIDERS
                .iter()
                .map(|provider| {
                    if *provider == selection.provider {
                        selection.clone()
                    } else {
                        Selection::new(*provider)
                    }
                })
                .collect(),
            Choice::ModelEffort => provider
                .models()
                .iter()
                .filter(|model| {
                    !live
                        || provider != Provider::Claude
                        || model.efforts.is_empty()
                        || model.efforts.contains(&selection.effort)
                })
                .map(|model| {
                    let mut row = selection.clone();
                    if provider
                        .model(&selection.model)
                        .is_some_and(|current| current.id == model.id)
                    {
                        return row;
                    }
                    row.model = model.id.to_owned();
                    if !model.efforts.is_empty() && !model.efforts.contains(&row.effort) {
                        row.effort = provider.default_effort_for(model.id);
                    }
                    row
                })
                .collect(),
            Choice::Launch => return Self::from_selection(selection, &[], live),
        };
        if !rows.contains(selection) {
            rows.push(selection.clone());
        }
        let at = rows
            .iter()
            .position(|row| row == selection)
            .unwrap_or_default();
        let mut picker = Self {
            choice,
            active: Field::Model,
            effort_list: FuzzyList::default(),
            fallback_effort: selection.effort,
            rows,
            list: FuzzyList::default(),
            opened_on: selection.clone(),
            provider_locked: live,
            recent_models: Vec::new(),
        };
        picker.list = FuzzyList::at(&picker.labels(), at).keeping_order();
        picker.sync_effort(selection.effort);
        picker
    }

    /// The selected model row, falling back to the opening selection when the
    /// model query matches nothing. Confirmation checks for a matching row.
    fn model_selection(&self) -> Selection {
        self.list
            .selected_row(&self.labels())
            .and_then(|row| self.rows.get(row))
            .cloned()
            .unwrap_or_else(|| self.opened_on.clone())
    }

    pub fn selection(&self) -> Selection {
        let mut selection = self.model_selection();
        if self.choice == Choice::ModelEffort {
            let efforts = self.efforts();
            selection.effort = self
                .effort_list
                .selected_row(&self.effort_labels())
                .and_then(|row| efforts.get(row))
                .copied()
                .unwrap_or(self.fallback_effort);
        }
        selection
    }

    fn efforts(&self) -> Vec<Effort> {
        let model = self.model_selection();
        let efforts = model.provider.efforts_for(&model.model);
        if self.provider_locked && model.provider == Provider::Claude {
            efforts
                .iter()
                .copied()
                .filter(|effort| *effort == self.opened_on.effort)
                .collect()
        } else {
            efforts.to_vec()
        }
    }

    pub fn effort_labels(&self) -> Vec<String> {
        self.efforts()
            .iter()
            .map(|effort| effort.as_str().to_owned())
            .collect()
    }

    pub fn has_selection(&self) -> bool {
        self.list.selected_row(&self.labels()).is_some()
            && (self.choice != Choice::ModelEffort
                || self.efforts().is_empty()
                || self
                    .effort_list
                    .selected_row(&self.effort_labels())
                    .is_some())
    }

    fn sync_effort(&mut self, previous: Effort) {
        if self.choice != Choice::ModelEffort {
            return;
        }
        let model = self.model_selection();
        let efforts = self.efforts();
        let desired = if efforts.is_empty() || efforts.contains(&previous) {
            previous
        } else {
            model.provider.default_effort_for(&model.model)
        };
        self.fallback_effort = desired;
        let labels = self.effort_labels();
        self.effort_list.selected = self
            .effort_list
            .matches(&labels)
            .iter()
            .position(|found| efforts[found.index] == desired)
            .unwrap_or_default();
    }

    fn edit_active(&mut self, edit: impl FnOnce(&mut FuzzyList, &[String])) {
        if self.choice == Choice::ModelEffort && self.active == Field::Effort {
            let labels = self.effort_labels();
            edit(&mut self.effort_list, &labels);
        } else {
            let effort = self.selection().effort;
            let labels = self.labels();
            edit(&mut self.list, &labels);
            self.sync_effort(effort);
        }
    }

    pub fn next(&mut self) {
        self.edit_active(|list, labels| list.next(labels));
    }

    fn cycle_next(&mut self) {
        self.edit_active(|list, labels| {
            let count = list.matches(labels).len();
            if count > 0 {
                list.selected = (list.selected.min(count - 1) + 1) % count;
            }
        });
    }

    pub fn prev(&mut self) {
        self.edit_active(|list, labels| list.prev(labels));
    }

    pub fn page_down(&mut self) {
        self.edit_active(|list, labels| list.page_down(labels));
    }

    pub fn page_up(&mut self) {
        self.edit_active(|list, labels| list.page_up(labels));
    }

    pub fn type_query(&mut self, edit: Edit) {
        self.edit_active(|list, _| list.edit(edit));
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

fn labels_of(rows: &[Selection]) -> Vec<String> {
    rows.iter().map(label).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_protocol::agent::{validate_selection, Effort};

    #[test]
    fn confirming_a_provider_conversion_records_the_choice_and_keeps_other_clients_history() {
        let root =
            std::env::temp_dir().join(format!("styra-picker-conversion-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let path = root.join("defaults.json");
        preferences::record_recent_model(&path, "claude-haiku-4-5-20251001").unwrap();
        let original = Selection::new(Provider::Claude);
        let chosen = Selection::new(Provider::Codex);
        let mut app = App::pending(original.clone());
        app.session_id = "stored-session".into();
        app.launcher = Some(Launcher::from_selection(&chosen, &[], false));
        confirm(&mut app, &path);
        assert_eq!(app.selection, original, "conversion is pending");
        assert_eq!(
            preferences::load_recent_models(&path),
            vec![chosen.model, "claude-haiku-4-5-20251001".into()]
        );
        std::fs::remove_dir_all(root).ok();
    }

    fn opened(selection: &str) -> Launcher {
        Launcher::from_selection(&Selection::parse(selection).unwrap(), &[], false)
    }

    fn typed(launcher: &mut Launcher, query: &str) {
        for character in query.chars() {
            launcher.type_query(Edit::Push(character));
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

    #[test]
    fn separate_lists_offer_each_model_and_its_supported_efforts() {
        for provider in PROVIDERS {
            let selection = Selection::new(provider);
            let mut picker = Launcher::for_choice(&selection, Choice::ModelEffort, false);
            assert_eq!(picker.selection(), selection);
            assert_eq!(picker.labels().len(), provider.models().len());
            for (index, model) in provider.models().iter().enumerate() {
                picker.list.selected = index;
                picker.sync_effort(selection.effort);
                assert_eq!(picker.selection().model, model.id);
                assert_eq!(picker.efforts(), model.efforts);
                for effort_index in 0..model.efforts.len().max(1) {
                    picker.effort_list.selected = effort_index;
                    validate_selection(&picker.selection()).unwrap();
                }
            }
        }
    }

    #[test]
    fn changing_model_preserves_effort_when_supported_and_adapts_shorter_ladders() {
        for provider in PROVIDERS {
            let mut selection = Selection::new(provider);
            selection.effort = *provider.efforts().last().unwrap();
            let mut picker = Launcher::for_choice(&selection, Choice::ModelEffort, false);
            for index in 0..picker.rows.len() {
                picker.list.selected = index;
                picker.sync_effort(selection.effort);
                let chosen = picker.selection();
                let efforts = provider.efforts_for(&chosen.model);
                if efforts.is_empty() || efforts.contains(&selection.effort) {
                    assert_eq!(chosen.effort, selection.effort);
                } else {
                    assert_eq!(chosen.effort, provider.default_effort_for(&chosen.model));
                }
                validate_selection(&chosen).unwrap();
            }
        }
    }

    #[test]
    fn ctrl_keys_target_separate_lists_and_filters_until_enter_applies_both() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("defaults.json");
        let original = Selection::new(Provider::Codex);
        let mut app = App::pending(original.clone());
        app.open_model_picker();
        let model_query = original.model.clone();
        for ch in model_query.chars() {
            handle_key(&mut app, KeyCode::Char(ch).into(), &path);
        }
        let model = app.launcher.as_ref().unwrap().selection().model;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &path,
        );
        assert_eq!(app.launcher.as_ref().unwrap().selection().model, model);
        for ch in "high".chars() {
            handle_key(&mut app, KeyCode::Char(ch).into(), &path);
        }
        let picker = app.launcher.as_ref().unwrap();
        assert_eq!(picker.active, Field::Effort);
        assert_eq!(picker.list.query, model_query);
        assert_eq!(picker.effort_list.query, "high");
        let effort = picker.selection().effort;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &path,
        );
        handle_key(&mut app, KeyCode::Backspace.into(), &path);
        let picker = app.launcher.as_ref().unwrap();
        assert_eq!(picker.active, Field::Model);
        assert_eq!(picker.list.query, &model_query[..model_query.len() - 1]);
        assert_eq!(picker.effort_list.query, "high");
        assert_eq!(picker.selection().effort, effort);
        assert_eq!(app.selection, original);
        assert!(app.take_request().is_none());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &path,
        );
        let chosen = app.launcher.as_ref().unwrap().selection();
        handle_key(&mut app, KeyCode::Enter.into(), &path);
        assert_eq!(app.selection, chosen);
        assert!(app.launcher.is_none());
    }

    #[test]
    fn focused_pickers_keep_custom_models_and_aliases_on_open() {
        for model in ["custom-model", "opus"] {
            let selection = Selection::parse(&format!("claude:{model}/high")).unwrap();
            for choice in [Choice::Provider, Choice::ModelEffort] {
                let picker = Launcher::for_choice(&selection, choice, false);
                assert_eq!(picker.selection(), selection);
                let labels = picker.labels();
                let mut unique = labels.clone();
                unique.sort();
                unique.dedup();
                assert_eq!(unique.len(), labels.len());
            }
        }
    }

    /// Both agents' catalogs are on offer at once — that is what a flat list
    /// buys — and each model's own ladder is under it.
    #[test]
    fn the_list_offers_every_agents_models_and_their_own_rungs() {
        let launcher = opened("claude");
        let labels = launcher.labels();
        for provider in PROVIDERS {
            for model in provider.models() {
                assert!(
                    labels.iter().any(|label| label.contains(model.id)),
                    "{} is not on offer: {labels:?}",
                    model.id
                );
            }
        }
        // A model with a ladder gets one row per rung, and one with no ladder
        // gets a single row that does not advertise a placeholder.
        let opus = labels
            .iter()
            .filter(|label| label.starts_with("claude:claude-opus-5/"))
            .count();
        assert_eq!(opus, Provider::Claude.efforts_for("claude-opus-5").len());
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
        typed(&mut launcher, "opus-5-5/max");
        assert_eq!(label(&launcher.selection()), "claude:claude-opus-5-5/max");
    }

    /// Typing narrows the list without reshuffling it: a query several models
    /// match leaves the most recently selected of them on top, even where
    /// another would rank as the closer match.
    #[test]
    fn recency_order_holds_while_filtering() {
        let recent = vec!["claude-sonnet-5".to_owned(), "claude-opus-5".to_owned()];
        let mut launcher =
            Launcher::from_selection(&Selection::new(Provider::Claude), &recent, false);
        typed(&mut launcher, "co5");
        assert_eq!(launcher.selection().model, "claude-sonnet-5");

        let matched: Vec<String> = launcher
            .list
            .matches(&launcher.labels())
            .into_iter()
            .map(|found| launcher.labels()[found.index].clone())
            .collect();
        let opus = matched
            .iter()
            .position(|label| label.contains(":claude-opus-5/"))
            .expect("the older recent model still matches");
        assert!(
            matched[..opus]
                .iter()
                .all(|label| label.contains(":claude-sonnet-5/")),
            "only the most recent model's rows come before the next: {matched:?}"
        );
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
        let catalog: Vec<&str> = Provider::Claude
            .models()
            .iter()
            .map(|model| model.id)
            .collect();
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
        typed(&mut launcher, "claude:claude-opus-5-5/mox");
        assert!(launcher.list.matches(&launcher.labels()).is_empty());

        launcher.type_query(Edit::DeleteWord);
        assert_eq!(launcher.list.query, "claude:claude-opus-5-5/");
        assert_eq!(launcher.selection().model, "claude-opus-5-5");
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
        assert_eq!(rungs, Provider::Claude.efforts_for("claude-opus-5"));
    }
}

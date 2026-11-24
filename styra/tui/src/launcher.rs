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
            launcher.cycle(Field::Model);
        }
        k if launcher.choice == Choice::ModelEffort && keys::EDITOR_CYCLE_EFFORT.matches(k) => {
            launcher.cycle(Field::Effort);
        }
        k if launcher.choice.repeats(k) => launcher.cycle_next(),
        k if keys::LAUNCHER_NEXT.matches(k) => launcher.next(),
        k if keys::LAUNCHER_PREV.matches(k) => launcher.prev(),
        k if keys::LAUNCHER_PAGE_DOWN.matches(k) => launcher.page_down(),
        k if keys::LAUNCHER_PAGE_UP.matches(k) => launcher.page_up(),
        k if keys::LAUNCHER_SELECT.matches(k) => confirm(app, preferences_path),
        k if keys::LAUNCHER_DEFAULT.matches(k) && launcher.choice == Choice::Launch => {
            confirm(app, preferences_path);
            if let Err(error) =
                preferences::save_selection(preferences_path, app.editor_selection())
            {
                app.report_error(format!("could not save launch defaults: {error:#}"));
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
        app.report_error(format!("could not save the model ordering: {error:#}"));
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

    /// Focus `field` and advance it one step, as its cycling shortcut does.
    pub fn cycle(&mut self, field: Field) {
        self.active = field;
        self.cycle_next();
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
mod tests;

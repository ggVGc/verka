//! The sandbox policy a launch runs under: the two layers it is made of, the
//! keys that edit them, and the plan the server resolves them into.
//!
//! Everything launch-specific outside of rendering lives here, so the rest of
//! the client only has to know that [`App`] carries a [`Launch`], that the
//! event loop hands keys to [`crate::keys`] which call the free functions
//! below, and that [`App::can_edit_launch`] says whether any of it applies.
//! Rendering is the matching [`crate::ui::driva`] module.
//!
//! The two layers are the whole shape of this module. The Workspace's standing
//! policy applies to every launch there and outlives every interaction in it;
//! this interaction's own settings are layered over it and go when it does.
//! [`Launch`] holds both, plus which one the keys are on ([`LaunchScope`]), and
//! the same key does the same thing to whichever that is.
//!
//! The split of responsibility here is deliberate: [`Launch`] holds the data
//! and makes the decisions that are only about policy (does this mount change
//! anything, what does `w` mean on this layer, is the plan still current), and
//! the free functions below pair those with the three things that belong to the
//! wider client — refusing an edit while an interaction is running, reporting
//! what happened, and asking the event loop to reach the server.

use crate::app::{App, Request, Status};
use crate::mount;
use styra_server::agent::Selection;
use styra_server::{DrivaOptions, LaunchMount, LaunchPolicy, WorkspaceLaunchChange};

/// Which of the two policy layers the Driva view's keys are editing.
///
/// The layers were only ever *shown* apart, while every key edited the second —
/// so changing what a body of work always needs meant tuning one interaction
/// and promoting it. The view now names one of them as the one being edited,
/// and `Tab` moves between them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchScope {
    /// The Workspace's standing policy: every launch here starts from it,
    /// from this client or any other.
    Workspace,
    /// This interaction's own settings, over the Workspace's. The default: it
    /// is the layer an operator reaches for most, and the one whose edits are
    /// theirs alone to make.
    #[default]
    Interaction,
}

impl LaunchScope {
    /// What this layer is called where it titles its own pane.
    pub fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Interaction => "this interaction",
        }
    }

    /// The same, for the middle of a sentence.
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Workspace => "the Workspace",
            Self::Interaction => "this interaction",
        }
    }

    /// What editing this layer changes, as a message names it.
    pub fn subject(self) -> &'static str {
        match self {
            Self::Workspace => "every launch in this Workspace",
            Self::Interaction => "this interaction",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Workspace => Self::Interaction,
            Self::Interaction => Self::Workspace,
        }
    }
}

/// The launch policy as this client holds it: both layers, the cursor into
/// each, and the sandbox they resolve to.
///
/// The two layers are held apart rather than merged on arrival ([`Self::merge`]
/// is the server's own code, called through [`Self::effective`]) so the view
/// can say which layer each grant comes from and an edit can land in one of
/// them without disturbing the other.
#[derive(Default)]
pub struct Launch {
    /// What a new interaction in this Workspace starts from: the standing
    /// policy stored with the Workspace itself, shared by every client that
    /// launches there. Editable here, but the server owns it — see
    /// [`store_workspace`].
    pub workspace: LaunchPolicy,
    /// What *this* interaction adds to (or says instead of) that.
    pub interaction: LaunchPolicy,
    /// Which of the two the keys act on.
    pub scope: LaunchScope,
    /// The mount cursor into each layer, kept per layer rather than shared so
    /// moving between the panes returns to where each was left.
    selected: (usize, usize),
    /// The open "add a mount" prompt, while the operator is typing one.
    pub prompt: Option<String>,
    /// The Driva policy of the live session, or — before one has launched — the
    /// policy the next interaction would be launched under. `None` until either
    /// is known, as on a replayed journal, which has no sandbox to describe and
    /// none to plan.
    pub driva: Option<DrivaOptions>,
    /// Whether `driva` describes a launch that has not happened yet. The two are
    /// worth telling apart on screen: one is what the agent runs under, the
    /// other is what it would run under.
    pub planned: bool,
    /// The (selection, effective policy) `driva` was planned for, recorded even
    /// when the plan could not be fetched. Anything that changes the policy
    /// makes this differ from the current one and the plan is re-asked for; a
    /// failing server is asked once per distinct input rather than every frame.
    plan_key: Option<(Selection, LaunchPolicy)>,
}

impl Launch {
    /// The layer `scope` names.
    pub fn policy(&self, scope: LaunchScope) -> &LaunchPolicy {
        match scope {
            LaunchScope::Workspace => &self.workspace,
            LaunchScope::Interaction => &self.interaction,
        }
    }

    /// The single policy a launch from here would run under: the Workspace's
    /// standing one with this interaction's own over it. Merged by the same code
    /// the server merges with, so what the view shows is what the launch
    /// resolves.
    pub fn effective(&self) -> LaunchPolicy {
        LaunchPolicy::merge(&self.workspace, &self.interaction)
    }

    /// Which mount of `scope`'s layer the cursor is on, clamped to the list as
    /// it stands — mounts come and go under it from both this client's keys and
    /// (for the Workspace's) another client's edit.
    pub fn cursor(&self, scope: LaunchScope) -> usize {
        let (index, len) = match scope {
            LaunchScope::Workspace => (self.selected.0, self.workspace.mounts.len()),
            LaunchScope::Interaction => (self.selected.1, self.interaction.mounts.len()),
        };
        index.min(len.saturating_sub(1))
    }

    fn cursor_mut(&mut self, scope: LaunchScope) -> &mut usize {
        match scope {
            LaunchScope::Workspace => &mut self.selected.0,
            LaunchScope::Interaction => &mut self.selected.1,
        }
    }

    /// Move the mount cursor within the layer being edited.
    pub fn select_next_mount(&mut self) {
        let last = self.policy(self.scope).mounts.len().saturating_sub(1);
        let cursor = self.cursor_mut(self.scope);
        *cursor = (*cursor + 1).min(last);
    }

    pub fn select_prev_mount(&mut self) {
        let cursor = self.cursor_mut(self.scope);
        *cursor = cursor.saturating_sub(1);
    }

    /// Record the Driva policy the live session was launched under. This is the
    /// real thing, so it replaces any plan made before the launch.
    pub fn record(&mut self, options: DrivaOptions) {
        self.driva = Some(options);
        self.planned = false;
        self.plan_key = None;
    }

    /// Record the policy a new interaction under `selection` and this effective
    /// launch policy would start with, or that the server could not say
    /// (`None`). Either way the inputs are remembered as asked about.
    ///
    /// `effective` is the merged policy, not one layer of it: the server answers
    /// for the merge, so keying the plan on anything less would leave a
    /// Workspace's own grants able to change without the plan being re-asked.
    pub fn plan(
        &mut self,
        selection: Selection,
        effective: LaunchPolicy,
        options: Option<DrivaOptions>,
    ) {
        self.plan_key = Some((selection, effective));
        if let Some(options) = options {
            self.driva = Some(options);
            self.planned = true;
        }
    }

    /// Whether the sandbox a not-yet-started interaction would get still has to
    /// be asked for. Gated by the caller on the policy being editable at all:
    /// a live session's own policy is never superseded by a plan.
    ///
    /// A stopped or ended session is asked about too, and this is why it is
    /// keyed on the inputs rather than on "have we ever been told a policy".
    /// What it holds is the record of the interaction that just finished; the
    /// moment a mount or a template is added, that record no longer describes
    /// what the next message would resume under, and showing it unchanged read
    /// as the edit having been discarded.
    pub fn needs_plan(&self, selection: &Selection) -> bool {
        // A Workspace edit the server has not accepted yet would be planned
        // against the policy the server still holds, so the answer would not
        // describe what is on screen. The store is one loop iteration away;
        // when it lands, the plan is re-asked because the key kept here is
        // still the one from before the edit.
        match &self.plan_key {
            Some((planned, effective)) => planned != selection || effective != &self.effective(),
            None => true,
        }
    }

    /// Adopt the standing policy of the Workspace now being viewed.
    ///
    /// This interaction's own settings are deliberately kept: they are what
    /// *this* client is building for its next interaction, and carrying them
    /// across a Workspace switch matches how the launch selection already
    /// travels.
    pub fn set_workspace(&mut self, workspace: LaunchPolicy) {
        self.workspace = workspace;
        self.selected = (0, 0);
    }

    /// Adopt the latest server-owned Workspace policy without disturbing UI
    /// navigation state. This is used both for edit responses and change-feed
    /// polling; the client never manufactures this policy itself.
    pub fn sync_workspace(&mut self, workspace: LaunchPolicy) {
        self.workspace = workspace;
        self.selected.0 = self.cursor(LaunchScope::Workspace);
    }

    /// Take the current effective policy as the Workspace's own, once the server
    /// has stored it: this interaction's settings are now redundant, so they are
    /// emptied rather than left to be layered onto themselves.
    pub fn adopt_workspace(&mut self, workspace: LaunchPolicy) {
        self.workspace = workspace;
        self.interaction = LaunchPolicy::default();
        self.selected = (0, 0);
    }

    /// What `w` means on the layer being edited, as the new state and the words
    /// for it.
    ///
    /// On this interaction's layer that is two states, not three: either it
    /// states the opposite of what it would otherwise inherit, or it states
    /// nothing and inherits. Stating agreement with the Workspace is expressible
    /// but pointless, so `w` does not stop there — the first press always
    /// changes the effective answer and the second always returns to inheriting.
    /// With no Workspace policy in play, "inherit" is "off" and this is the plain
    /// on/off toggle it was.
    ///
    /// On the Workspace's layer nothing sits underneath, so there is nothing to
    /// inherit and it is that plain toggle by construction.
    fn cycle_network(&mut self) -> String {
        match self.scope {
            LaunchScope::Workspace => {
                let on = !self.workspace.grants_network();
                self.workspace.network = Some(on);
                if on {
                    "network on for every launch in this Workspace".to_owned()
                } else {
                    "network off for every launch here — a profile or template may still permit it"
                        .to_owned()
                }
            }
            LaunchScope::Interaction => {
                let inherited = self.workspace.grants_network();
                self.interaction.network = match self.interaction.network {
                    None => Some(!inherited),
                    Some(_) => None,
                };
                match self.interaction.network {
                    Some(true) => "network on for this interaction".to_owned(),
                    // Only ever a withdrawal of *this* permission: the profile
                    // and the templates have their own, which the server ORs in,
                    // so say so rather than promising a sandbox with no network.
                    Some(false) => {
                        "network permission withdrawn — a profile or template may still permit it"
                            .to_owned()
                    }
                    None => format!(
                        "network follows the Workspace policy again ({})",
                        if inherited { "on" } else { "off" }
                    ),
                }
            }
        }
    }

    /// Adopt the templates chosen in the picker, for the layer being edited.
    ///
    /// On the Workspace's layer the list is the list: the picker offered exactly
    /// that layer's templates and what came back replaces them.
    ///
    /// On this interaction's, the picker shows and returns the *effective* list,
    /// so the choice has to be turned back into an overlay. Keeping everything
    /// the Workspace grants means the overlay is just the additions. Dropping one
    /// of them cannot be said by adding, so the interaction stops inheriting and
    /// carries the list itself — otherwise deselecting a Workspace template would
    /// silently do nothing. Whether that happened is what comes back.
    fn set_templates(&mut self, chosen: Vec<String>) -> Option<&'static str> {
        if self.scope == LaunchScope::Workspace {
            self.workspace.templates = chosen;
            return None;
        }
        let base = &self.workspace.templates;
        if !self.interaction.standalone && base.iter().all(|name| chosen.contains(name)) {
            self.interaction.templates = chosen
                .into_iter()
                .filter(|name| !base.contains(name))
                .collect();
            return None;
        }
        let now_standalone = !self.interaction.standalone;
        self.interaction.standalone = true;
        self.interaction.templates = chosen;
        now_standalone.then_some(
            "standalone — dropping a Workspace template means this interaction carries its own list",
        )
    }

    /// Add `mount` to the layer being edited, or say why it would change
    /// nothing there.
    ///
    /// The two layers refuse a mount for different reasons. On the Workspace's,
    /// only a duplicate of its own grants is pointless. On this interaction's, a
    /// mount the Workspace already grants identically is too — an overlay row
    /// that changes nothing about the sandbox is worse than being told so.
    fn add_mount(&mut self, mount: LaunchMount) -> Result<(), &'static str> {
        self.add_mount_to(self.scope, mount)
    }

    /// Add `mount` to this interaction's own layer whatever the driva view's
    /// keys are currently editing.
    ///
    /// For the callers that are not the driva view — the message editor's path
    /// prompt — where a grant lands is not the operator's choice of pane but a
    /// property of what they are doing: a path that came up in one message is
    /// this interaction's business, and `U` moves it up a layer if it turns out
    /// to be the Workspace's.
    pub fn add_interaction_mount(&mut self, mount: LaunchMount) -> Result<(), &'static str> {
        self.add_mount_to(LaunchScope::Interaction, mount)
    }

    fn add_mount_to(&mut self, scope: LaunchScope, mount: LaunchMount) -> Result<(), &'static str> {
        match scope {
            LaunchScope::Workspace => {
                if self.workspace.mounts.contains(&mount) {
                    return Err("the Workspace policy already grants that mount");
                }
                self.workspace.mounts.push(mount);
                self.selected.0 = self.workspace.mounts.len() - 1;
            }
            LaunchScope::Interaction => {
                if self.interaction.mounts.contains(&mount) {
                    return Err("this interaction already asks for that mount");
                }
                if !self.interaction.standalone && self.workspace.mounts.contains(&mount) {
                    return Err("the Workspace policy already grants that mount");
                }
                self.interaction.mounts.push(mount);
                self.selected.1 = self.interaction.mounts.len() - 1;
            }
        }
        Ok(())
    }

    /// Drop the mount under the cursor of the layer being edited, or say why
    /// there is none to drop.
    fn remove_mount(&mut self) -> Result<LaunchMount, &'static str> {
        let scope = self.scope;
        if self.policy(scope).mounts.is_empty() {
            // Nothing under the cursor: name the pane that does hold mounts,
            // rather than leaving the operator pressing `x` at an empty list.
            return Err(match scope {
                LaunchScope::Workspace => "the Workspace policy has no mounts to remove",
                LaunchScope::Interaction if !self.workspace.mounts.is_empty() => {
                    "this interaction adds no mount — the Workspace's own are edited with Tab"
                }
                LaunchScope::Interaction => "no added mount to remove",
            });
        }
        let index = self.cursor(scope);
        let removed = match scope {
            LaunchScope::Workspace => self.workspace.mounts.remove(index),
            LaunchScope::Interaction => self.interaction.mounts.remove(index),
        };
        let remaining = self.policy(scope).mounts.len();
        *self.cursor_mut(scope) = index.min(remaining.saturating_sub(1));
        Ok(removed)
    }
}

// --- Keys ------------------------------------------------------------------
//
// Each of these pairs a decision above with the parts that are the client's
// rather than the policy's: whether an edit is allowed at all, what to say
// about it, and — for the Workspace's layer — getting it to the server.

/// Move the editing keys to the other layer. Not itself an edit, so it works on
/// a live interaction too: there the two panes are a record, and which of them a
/// grant came from is still worth reading.
pub fn toggle_scope(app: &mut App) {
    app.launch.scope = app.launch.scope.other();
    app.show_action_message(match app.launch.scope {
        LaunchScope::Workspace => "editing the Workspace policy — every launch here starts from it",
        LaunchScope::Interaction => "editing this interaction's own settings",
    });
}

/// Permit or forbid agent networking, for whichever layer is being edited.
pub fn cycle_network(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.scope == LaunchScope::Workspace {
        let on = !app.launch.workspace.grants_network();
        workspace_change(app, WorkspaceLaunchChange::SetNetwork(Some(on)));
        return app.show_action_message(if on {
            "network on for every launch in this Workspace"
        } else {
            "network off for every launch here — a profile or template may still permit it"
        });
    }
    let message = app.launch.cycle_network();
    app.show_action_message(message);
}

/// Say whether this interaction inherits the Workspace's templates and mounts or
/// carries the whole policy itself. Bound to `I`, for inheriting.
///
/// This one is only ever the interaction's own answer: there is nothing for the
/// Workspace's own policy to stand apart from.
pub fn toggle_standalone(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.scope == LaunchScope::Workspace {
        return app
            .show_action_message("inheriting is this interaction's answer — press Tab to edit it");
    }
    app.launch.interaction.standalone = !app.launch.interaction.standalone;
    app.show_action_message(if app.launch.interaction.standalone {
        "standalone — the Workspace policy does not apply to this interaction"
    } else {
        "this interaction adds to the Workspace policy again"
    });
}

/// Adopt the templates the picker came back with.
pub fn set_templates(app: &mut App, chosen: Vec<String>) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.scope == LaunchScope::Workspace {
        workspace_change(app, WorkspaceLaunchChange::SetTemplates(chosen));
        return;
    }
    let note = app.launch.set_templates(chosen);
    if let Some(note) = note {
        app.show_action_message(note);
    }
}

/// Open the prompt that adds an extra mount.
pub fn open_prompt(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    app.launch.prompt = Some(String::new());
}

pub fn cancel_prompt(app: &mut App) {
    app.launch.prompt = None;
}

/// Accept what the prompt holds as an extra mount, or explain why it is not one
/// and leave the prompt open with the text still in it.
pub fn confirm_prompt(app: &mut App) {
    let Some(text) = app.launch.prompt.clone() else {
        return;
    };
    if !app.allow_launch_edit() {
        app.launch.prompt = None;
        return;
    }
    let parsed = match mount::parse(&text) {
        Ok(mount) => mount,
        Err(problem) => return app.show_action_message(problem),
    };
    app.launch.prompt = None;
    let label = mount::label(&parsed);
    if app.launch.scope == LaunchScope::Workspace {
        if app.launch.workspace.mounts.contains(&parsed) {
            return app.show_action_message("the Workspace policy already grants that mount");
        }
        workspace_change(app, WorkspaceLaunchChange::AddMounts(vec![parsed]));
        return app.show_action_message(format!("added {label} to every launch in this Workspace"));
    }
    match app.launch.add_mount(parsed) {
        Ok(()) => {
            let scope = app.launch.scope;
            app.show_action_message(format!("added {label} to {}", scope.subject()));
        }
        Err(reason) => app.show_action_message(reason),
    }
}

/// Drop the selected mount from the layer being edited.
pub fn remove_selected_mount(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    let scope = app.launch.scope;
    if scope == LaunchScope::Workspace {
        let Some(removed) = app
            .launch
            .workspace
            .mounts
            .get(app.launch.cursor(scope))
            .cloned()
        else {
            return app.show_action_message("the Workspace policy has no mounts to remove");
        };
        workspace_change(app, WorkspaceLaunchChange::RemoveMount(removed.clone()));
        return app.show_action_message(format!(
            "removed {} from every launch in this Workspace",
            mount::label(&removed)
        ));
    }
    match app.launch.remove_mount() {
        Ok(removed) => {
            app.show_action_message(format!(
                "removed {} from {}",
                mount::label(&removed),
                scope.subject()
            ));
        }
        Err(reason) => app.show_action_message(reason),
    }
}

pub fn select_next_mount(app: &mut App) {
    app.launch.select_next_mount();
}

pub fn select_prev_mount(app: &mut App) {
    app.launch.select_prev_mount();
}

/// Keep what this interaction added as the Workspace's own standing policy.
/// Bound to `U`, for moving a setting up a layer once it turns out to be a
/// property of the work rather than of one conversation about it.
pub fn promote_to_workspace(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.interaction.is_empty() {
        return app.show_action_message("this interaction adds nothing to the Workspace policy");
    }
    let policy = app.launch.effective();
    app.ask(Request::ChangeWorkspaceLaunch {
        change: WorkspaceLaunchChange::Replace(policy),
        clear_interaction: true,
    });
}

/// Note that the Workspace's standing policy has just been changed here, and ask
/// the event loop to send it: the server owns that layer. A no-op when the edit
/// landed in this interaction's own settings, which are this client's to keep.
///
/// Sent on every edit rather than on request, because each launch path sends
/// only this interaction's half and merges the *stored* Workspace policy on the
/// server. A Workspace edit kept in this client alone would show on screen as
/// part of the effective policy — and the plan, and the launch, would quietly
/// ignore it.
fn workspace_change(app: &mut App, change: WorkspaceLaunchChange) {
    app.ask(Request::ChangeWorkspaceLaunch {
        change,
        clear_interaction: false,
    });
}

/// While nothing is running there is no sandbox to describe, but there is one
/// decided. Whether the client should go ask the server for it.
pub fn wants_plan(app: &App) -> bool {
    app.can_edit_launch() && app.launch.needs_plan(&app.selection)
}

/// Whether the launch policy can still be edited. Only before an interaction
/// exists, or after one has finished: while one is running or idle, what the
/// Driva view shows is a record of the sandbox it is confined to, and changing
/// that would mean a new session. Once nothing is running there is no live
/// sandbox to contradict, so editing reopens just as if the interaction had
/// never started — and the resume the next message triggers launches under the
/// edited policy.
///
/// `Ended` counts as much as `Stopped`. An agent that exited on its own, and a
/// stored session replayed from its journal (which is marked ended when it is
/// opened), are resumed by exactly the same path as one the operator stopped, so
/// refusing edits there only made the policy look frozen while the resume
/// happily accepted a new one.
pub fn editable(status: &Status) -> bool {
    matches!(
        status,
        Status::Pending | Status::Stopped | Status::Ended { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace_policy() -> LaunchPolicy {
        LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            mounts: vec![LaunchMount {
                source: PathBuf::from("/srv/corpus"),
                destination: None,
                writable: false,
            }],
            standalone: false,
        }
    }

    fn pending() -> App {
        App::pending(Selection::parse("codex").unwrap())
    }

    /// An App with an interaction already running, where the policy is a record
    /// rather than a choice.
    fn running() -> App {
        App::new(Selection::parse("codex").unwrap(), "s1")
    }

    fn options(backend: &str) -> DrivaOptions {
        DrivaOptions {
            isolation_backend: backend.into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: Vec::new(),
        }
    }

    fn pending_in_a_workspace_with_a_policy() -> App {
        let mut app = pending();
        app.launch.set_workspace(workspace_policy());
        app
    }

    fn add_mount(app: &mut App, text: &str) {
        app.launch.prompt = Some(text.into());
        confirm_prompt(app);
    }

    /// The two layers are held apart, and an edit lands in one of them: what the
    /// Workspace grants is added to, not replaced.
    #[test]
    fn an_edit_adds_to_the_workspace_policy_rather_than_replacing_it() {
        let mut app = pending_in_a_workspace_with_a_policy();
        assert_eq!(app.launch.effective(), workspace_policy());

        set_templates(&mut app, vec!["rust".into(), "browser".into()]);
        // Only the addition is this interaction's; the Workspace keeps its own.
        assert_eq!(app.launch.interaction.templates, vec!["browser"]);
        assert!(!app.launch.interaction.standalone);
        assert_eq!(
            app.launch.effective().templates,
            vec!["rust".to_owned(), "browser".to_owned()]
        );

        add_mount(&mut app, "/srv/scratch:rw");
        assert_eq!(app.launch.interaction.mounts.len(), 1);
        assert_eq!(app.launch.effective().mounts.len(), 2);
    }

    /// Dropping a template the Workspace grants cannot be said by adding to it,
    /// so the interaction stops inheriting instead of silently doing nothing.
    #[test]
    fn deselecting_a_workspace_template_makes_the_launch_standalone() {
        let mut app = pending_in_a_workspace_with_a_policy();
        set_templates(&mut app, vec!["browser".into()]);

        assert!(app.launch.interaction.standalone);
        assert_eq!(app.launch.interaction.templates, vec!["browser"]);
        let effective = app.launch.effective();
        assert_eq!(effective.templates, vec!["browser"]);
        // Standalone is the whole policy, so the Workspace's mounts and its
        // network grant go with its templates.
        assert!(effective.mounts.is_empty());
        assert!(!effective.grants_network());
    }

    /// `w` states the opposite of what would otherwise be inherited, and then
    /// returns to inheriting — there is no third state worth stopping on.
    #[test]
    fn network_toggles_against_the_workspace_policy_and_back_to_inheriting() {
        let mut app = pending_in_a_workspace_with_a_policy();
        assert!(app.launch.effective().grants_network());

        cycle_network(&mut app);
        assert_eq!(app.launch.interaction.network, Some(false));
        assert!(!app.launch.effective().grants_network());

        cycle_network(&mut app);
        assert_eq!(app.launch.interaction.network, None);
        assert!(app.launch.effective().grants_network());
    }

    /// A mount the Workspace already grants identically needs no overlay, and
    /// one of its mounts is not this interaction's to remove.
    #[test]
    fn the_workspace_policys_mounts_are_not_this_interactions_to_add_or_remove() {
        let mut app = pending_in_a_workspace_with_a_policy();
        add_mount(&mut app, "/srv/corpus:ro");
        assert!(app.launch.interaction.mounts.is_empty());

        remove_selected_mount(&mut app);
        assert_eq!(app.launch.effective().mounts, workspace_policy().mounts);

        // Standalone is the way out, and it leaves the interaction with nothing
        // but its own inputs.
        toggle_standalone(&mut app);
        assert!(app.launch.effective().mounts.is_empty());
    }

    /// The Workspace's own layer is edited here too, by the same keys, once
    /// `Tab` has moved them onto it — and, since the server owns that layer and
    /// every launch path merges the stored copy, each edit is on its way there.
    #[test]
    fn the_workspace_layer_is_edited_by_the_same_keys_and_sent_to_the_server() {
        let mut app = pending_in_a_workspace_with_a_policy();
        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);
        assert!(!wants_plan(&app));

        toggle_scope(&mut app);
        assert_eq!(app.launch.scope, LaunchScope::Workspace);

        // The UI sends intent and does not optimistically mutate its server
        // snapshot.
        add_mount(&mut app, "/srv/models:ro");
        assert_eq!(app.launch.workspace.mounts.len(), 1);
        assert!(app.launch.interaction.mounts.is_empty());
        let request = app.take_request().unwrap();
        let Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::AddMounts(added),
            clear_interaction: false,
        } = request
        else {
            panic!("unexpected request: {request:?}");
        };
        assert!(!wants_plan(&app));
        let mut stored = app.launch.workspace.clone();
        stored.mounts.extend(added);
        app.launch.sync_workspace(stored);
        assert_eq!(app.launch.workspace.mounts.len(), 2);
        assert!(wants_plan(&app));

        // One the Workspace already grants changes nothing, and says so.
        add_mount(&mut app, "/srv/models:ro");
        assert_eq!(app.launch.workspace.mounts.len(), 2);

        // `x` reaches the Workspace's own mounts here, which is what the
        // interaction layer could never do.
        remove_selected_mount(&mut app);
        assert_eq!(app.launch.workspace.mounts.len(), 2);
        assert!(matches!(
            app.take_request(),
            Some(Request::ChangeWorkspaceLaunch {
                change: WorkspaceLaunchChange::RemoveMount(_),
                clear_interaction: false,
            })
        ));

        // Templates replace that layer's list rather than being turned into an
        // overlay: there is nothing under it for them to add to.
        set_templates(&mut app, vec!["browser".into()]);
        assert_eq!(app.launch.workspace.templates, vec!["rust"]);
        assert_eq!(
            app.take_request(),
            Some(Request::ChangeWorkspaceLaunch {
                change: WorkspaceLaunchChange::SetTemplates(vec!["browser".into()]),
                clear_interaction: false,
            })
        );
        assert!(app.launch.interaction.templates.is_empty());

        // And network is the plain toggle it can be with nothing underneath.
        cycle_network(&mut app);
        assert_eq!(app.launch.workspace.network, Some(true));
        assert_eq!(
            app.take_request(),
            Some(Request::ChangeWorkspaceLaunch {
                change: WorkspaceLaunchChange::SetNetwork(Some(false)),
                clear_interaction: false,
            })
        );
        assert_eq!(app.launch.interaction.network, None);
    }

    /// Whether this interaction inherits the Workspace's policy is only ever
    /// this interaction's answer, so the key says so instead of quietly editing
    /// the layer it cannot mean anything for.
    #[test]
    fn inheriting_stays_this_interactions_own_answer() {
        let mut app = pending_in_a_workspace_with_a_policy();
        toggle_scope(&mut app);
        toggle_standalone(&mut app);
        assert!(!app.launch.interaction.standalone);
        assert!(!app.launch.workspace.standalone);

        toggle_scope(&mut app);
        toggle_standalone(&mut app);
        assert!(app.launch.interaction.standalone);
    }

    /// The mount cursor is per layer: moving between the panes returns to where
    /// each was left rather than pointing at whatever row shares its index.
    #[test]
    fn each_layer_keeps_its_own_mount_cursor() {
        let mut app = pending_in_a_workspace_with_a_policy();
        add_mount(&mut app, "/srv/one");
        add_mount(&mut app, "/srv/two");
        assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);
        // The Workspace's single mount, whatever this interaction's cursor is.
        assert_eq!(app.launch.cursor(LaunchScope::Workspace), 0);

        toggle_scope(&mut app);
        select_next_mount(&mut app);
        assert_eq!(app.launch.cursor(LaunchScope::Workspace), 0);
        assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);

        // `x` on the Workspace's pane leaves this interaction's mounts alone.
        remove_selected_mount(&mut app);
        assert_eq!(app.launch.workspace.mounts.len(), 1);
        assert!(matches!(
            app.take_request(),
            Some(Request::ChangeWorkspaceLaunch {
                change: WorkspaceLaunchChange::RemoveMount(_),
                clear_interaction: false,
            })
        ));
        assert_eq!(app.launch.interaction.mounts.len(), 2);
    }

    /// Storing the policy with the Workspace must not change what the next
    /// launch runs under: this interaction's half is folded in, not layered
    /// onto itself.
    #[test]
    fn promoting_a_policy_to_the_workspace_leaves_the_effective_one_unchanged() {
        let mut app = pending_in_a_workspace_with_a_policy();
        set_templates(&mut app, vec!["rust".into(), "browser".into()]);
        let before = app.launch.effective();

        app.launch.adopt_workspace(before.clone());
        assert!(app.launch.interaction.is_empty());
        assert_eq!(app.launch.effective(), before);
    }

    /// `U` is for a setting that turns out to belong to the work rather than to
    /// this conversation, so with nothing added here there is nothing to move.
    #[test]
    fn moving_this_interactions_settings_up_needs_some_to_move() {
        let mut app = pending_in_a_workspace_with_a_policy();
        promote_to_workspace(&mut app);
        assert_eq!(app.take_request(), None);

        add_mount(&mut app, "/srv/scratch:rw");
        promote_to_workspace(&mut app);
        assert_eq!(
            app.take_request(),
            Some(Request::ChangeWorkspaceLaunch {
                change: WorkspaceLaunchChange::Replace(app.launch.effective()),
                clear_interaction: true,
            })
        );
    }

    #[test]
    fn added_mounts_are_selected_and_removed_one_at_a_time() {
        let mut app = pending();
        for text in ["/srv/one", "/srv/two:rw", "/srv/three:/mnt/three:ro"] {
            add_mount(&mut app, text);
        }
        assert_eq!(app.launch.interaction.mounts.len(), 3);
        // Adding leaves the cursor on what was just added.
        assert_eq!(app.launch.cursor(LaunchScope::Interaction), 2);

        select_prev_mount(&mut app);
        remove_selected_mount(&mut app);
        assert_eq!(
            app.launch
                .interaction
                .mounts
                .iter()
                .map(|mount| mount.source.display().to_string())
                .collect::<Vec<_>>(),
            ["/srv/one", "/srv/three"]
        );
        // The cursor stays in range as the list shrinks under it.
        select_next_mount(&mut app);
        select_next_mount(&mut app);
        assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);
        remove_selected_mount(&mut app);
        remove_selected_mount(&mut app);
        assert!(app.launch.interaction.mounts.is_empty());
        assert_eq!(app.launch.cursor(LaunchScope::Interaction), 0);
        // Removing from an empty list says so rather than panicking.
        remove_selected_mount(&mut app);
    }

    /// A duplicate is refused rather than silently layered twice, since the
    /// server would then reject the whole policy for a conflicting destination.
    #[test]
    fn the_same_mount_is_not_added_twice() {
        let mut app = pending();
        add_mount(&mut app, "/srv/data");
        add_mount(&mut app, "/srv/data:ro");
        assert_eq!(app.launch.interaction.mounts.len(), 1);
    }

    /// Text that is not a mount leaves the prompt open with what was typed, so a
    /// typo is corrected rather than retyped.
    #[test]
    fn an_unparseable_mount_keeps_the_prompt_open() {
        let mut app = pending();
        add_mount(&mut app, "relative/path");
        assert_eq!(app.launch.prompt.as_deref(), Some("relative/path"));
        assert!(app.launch.interaction.mounts.is_empty());
    }

    /// Editing the launch inputs changes what would be launched, so the plan on
    /// screen has to be re-asked for — otherwise the view would keep describing
    /// a sandbox the operator has just changed.
    #[test]
    fn editing_the_launch_inputs_re_asks_for_the_plan() {
        let mut app = pending();
        let effective = app.launch.effective();
        app.launch.plan(
            app.selection.clone(),
            effective,
            Some(DrivaOptions {
                isolation_backend: "bwrap".into(),
                command: vec!["codex".into()],
                working_directory: PathBuf::from("/tmp/styra/workspace"),
                network: false,
                mounts: Vec::new(),
            }),
        );
        assert!(!wants_plan(&app));

        cycle_network(&mut app);
        assert_eq!(app.launch.interaction.network, Some(true));
        assert!(wants_plan(&app));

        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);
        assert!(!wants_plan(&app));

        set_templates(&mut app, vec!["rust".into()]);
        assert!(wants_plan(&app));
        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);

        add_mount(&mut app, "/srv/data:rw");
        assert!(wants_plan(&app));
    }

    /// The plan is asked for once per distinct set of inputs, and a live
    /// interaction's own policy replaces it outright.
    #[test]
    fn a_planned_policy_is_asked_for_once_per_selection_and_yields_to_a_live_one() {
        let mut app = pending();
        assert!(wants_plan(&app));
        let effective = app.launch.effective();
        app.launch
            .plan(app.selection.clone(), effective, Some(options("planned")));
        assert!(app.launch.planned);
        assert!(!wants_plan(&app));

        // Switching model before launch changes the policy, so it is re-asked.
        app.selection = Selection::parse("claude").unwrap();
        assert!(wants_plan(&app));
        // A failed plan still counts as asked: the server is not polled per frame.
        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);
        assert!(!wants_plan(&app));
        assert_eq!(
            app.launch.driva.as_ref().unwrap().isolation_backend,
            "planned"
        );

        // Once something is running, what it runs under replaces the plan and
        // no further planning happens.
        app.status = Status::Running;
        app.launch.record(options("live"));
        assert!(!app.launch.planned);
        assert!(!wants_plan(&app));
        app.selection = Selection::parse("codex").unwrap();
        assert!(!wants_plan(&app));
        assert_eq!(app.launch.driva.as_ref().unwrap().isolation_backend, "live");
    }

    /// The reported flow: stop a session, add a mount, send a new message. The
    /// mount has to reach the resume, and the view has to stop describing the
    /// sandbox the stopped interaction ran in — otherwise the edit reads as
    /// having been discarded and the old policy as having been restored.
    #[test]
    fn a_stopped_or_ended_session_re_plans_its_policy_before_the_resume() {
        for status in [
            Status::Stopped,
            Status::Ended {
                exit_code: Some(0),
                error: None,
            },
        ] {
            let mut app = running();
            app.launch.record(options("live"));
            // While it runs, the record stands and nothing is asked.
            assert!(!app.can_edit_launch());
            assert!(!wants_plan(&app));

            app.status = status;
            assert!(app.can_edit_launch());
            // Nothing has changed yet, but the record is the previous
            // interaction's, so the sandbox a resume would use is asked for.
            assert!(wants_plan(&app));
            let effective = app.launch.effective();
            app.launch
                .plan(app.selection.clone(), effective, Some(options("live")));
            assert!(!wants_plan(&app));

            // The edit lands in the half the resume sends...
            add_mount(&mut app, "/srv/data");
            assert_eq!(app.launch.effective().mounts.len(), 1);
            // ...and is re-planned, so the view answers for it.
            assert!(wants_plan(&app));
        }
    }

    /// Entering another Workspace re-plans: the policy a launch would run under
    /// changed even though nothing the operator typed did.
    #[test]
    fn a_workspace_policy_change_re_asks_for_the_plan() {
        let mut app = pending();
        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);
        assert!(!wants_plan(&app));

        app.launch.set_workspace(workspace_policy());
        assert!(wants_plan(&app));
    }

    /// The launch policy is only editable while nothing has started: once an
    /// interaction is running, the driva view is a record of the sandbox it is
    /// confined to and changing it would mean a new session.
    #[test]
    fn the_launch_policy_cannot_be_edited_once_an_interaction_is_running() {
        let mut app = App::new(Selection::parse("codex").unwrap(), "s1");
        assert!(!app.can_edit_launch());

        cycle_network(&mut app);
        assert_eq!(app.launch.interaction.network, None);
        set_templates(&mut app, vec!["rust".into()]);
        assert!(app.launch.interaction.templates.is_empty());
        open_prompt(&mut app);
        assert!(app.launch.prompt.is_none());
        toggle_standalone(&mut app);
        assert!(!app.launch.interaction.standalone);

        let mut pending = pending();
        assert!(pending.can_edit_launch());
        cycle_network(&mut pending);
        assert_eq!(pending.launch.interaction.network, Some(true));
    }
}

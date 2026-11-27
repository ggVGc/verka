//! The sandbox policy a launch runs under: the two layers it is made of, the
//! keys that edit them, and the plan the server resolves them into.
//!
//! Everything launch-specific outside of rendering lives here, so the rest of
//! the client only has to know that [`App`] carries a [`Launch`], that the
//! event loop resolves [`crate::keybindings`] and calls the free functions
//! below, and that [`App::can_edit_launch`] says whether any of it applies.
//! Rendering is the matching [`crate::presentation::driva`] module.
//!
//! The two layers are the whole shape of this module. The Workspace's standing
//! policy applies to every launch there and outlives every interaction in it;
//! this interaction's own settings are layered over it and go when it does.
//! [`Launch`] holds both, plus which one the keys are on ([`LaunchScope`]), and
//! the same key does the same thing to whichever that is. The details view's
//! up/down arrows move the focus between those panes.
//!
//! The split of responsibility here is deliberate: [`Launch`] holds the data
//! and makes the decisions that are only about policy (does this mount change
//! anything, what does `w` mean on this layer, is the plan still current), and
//! the free functions below pair those with the three things that belong to the
//! wider client — refusing an edit while an interaction is running, reporting
//! what happened, and asking the event loop to reach the server.

use crate::activity::Status;
use crate::app::{App, Request};
use crate::mount::{self, PathList};
use styra_protocol::agent::Selection;
use styra_protocol::{
    DrivaOptions, LaunchMount, LaunchPolicy, Mount, MountAccess, MountOrigin, ProjectAction,
    WorkspaceLaunchChange,
};
use styra_ui::fuzzy_list::FuzzyList;

/// Which of the two policy layers the Driva view's keys are editing.
///
/// The layers were only ever *shown* apart, while every key edited the second —
/// so changing what a body of work always needs meant tuning one interaction
/// and promoting it. The view now names one of them as the one being edited,
/// and the up/down arrows move between them.
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
    /// The paths listed under that prompt, narrowed by the name being typed.
    pub paths: Option<mount::PathList>,
    /// The open prompt editing a project action's command in the focused
    /// layer, and what has been typed into it.
    pub command_prompt: Option<(ProjectAction, String)>,
    /// The Driva policy of the live session, or — before one has launched — the
    /// policy the next interaction would be launched under. `None` until either
    /// is known, as on a replayed journal, which has no sandbox to describe and
    /// none to plan.
    pub driva: Option<DrivaOptions>,
    /// Whether `driva` describes a launch that has not happened yet. The two are
    /// worth telling apart on screen: one is what the agent runs under, the
    /// other is what it would run under.
    pub planned: bool,
    /// How far the sandbox account is scrolled. The policy is now longer than
    /// a terminal — mounts, the backend's own floor, the whole environment,
    /// and the private root — and everything in it is worth being able to
    /// reach, so the part that does not fit is scrolled to rather than lost.
    pub scroll: styra_ui::scroll::Scroll,
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

    /// What the workspace mount is when no layer says: writable, except where
    /// the server has planned a launch in its repository's main worktree.
    pub fn writable_workspace_by_default(&self) -> bool {
        !self
            .driva
            .as_ref()
            .is_some_and(|options| options.workspace_read_only_by_default)
    }

    /// Whether the sandbox `driva` describes binds the workspace read-only.
    /// Read from the resolved mount list rather than from the policy layers,
    /// since that is what the agent actually runs (or, while `planned`, would
    /// run) under; a repository mount over the same directory may have widened
    /// it. `false` while no policy is known.
    pub fn workspace_read_only(&self) -> bool {
        self.driva.as_ref().is_some_and(|options| {
            options.mounts.iter().any(|attributed| {
                attributed.origin == MountOrigin::Workspace
                    && matches!(
                        attributed.mount,
                        Mount::Bind {
                            access: MountAccess::ReadOnly,
                            ..
                        }
                    )
            })
        })
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
        // A different sandbox is a different account of one, so it is read
        // from the top rather than from wherever the last one was left.
        self.scroll.reset();
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

    /// What `R` means on the layer being edited, as the new state and the words
    /// for it.
    ///
    /// The same two-state shape as [`Self::cycle_network`], around a default
    /// that is usually the opposite one: nothing stated means the workspace is
    /// bound writable, except in a repository's main worktree. Either way the
    /// first press always changes the effective answer and the second always
    /// returns to inheriting.
    fn cycle_writable_workspace(&mut self) -> String {
        let inherited = self
            .workspace
            .grants_writable_workspace(self.writable_workspace_by_default());
        self.interaction.writable_workspace = match self.interaction.writable_workspace {
            None => Some(!inherited),
            Some(_) => None,
        };
        match self.interaction.writable_workspace {
            // Unlike the network answer this is the whole of the question: the
            // workspace mount is assembled from it alone, so it can be promised.
            Some(false) => "the workspace is mounted read-only for this interaction".to_owned(),
            Some(true) => "the workspace is mounted read-write for this interaction".to_owned(),
            None => format!(
                "the workspace mount follows the Workspace policy again ({})",
                access_word(inherited)
            ),
        }
    }

    /// Adopt the effective templates chosen for this interaction.
    ///
    /// The picker shows and returns the *effective* list,
    /// so the choice has to be turned back into an overlay. Keeping everything
    /// the Workspace grants means the overlay is just the additions. Dropping one
    /// of them cannot be said by adding, so the interaction stops inheriting and
    /// carries the list itself — otherwise deselecting a Workspace template would
    /// silently do nothing. Whether that happened is what comes back.
    fn set_interaction_templates(&mut self, chosen: Vec<String>) -> Option<&'static str> {
        let base = &self.workspace.templates;
        if !self.interaction.ignore_workspace && base.iter().all(|name| chosen.contains(name)) {
            self.interaction.templates = chosen
                .into_iter()
                .filter(|name| !base.contains(name))
                .collect();
            return None;
        }
        let newly_ignoring_workspace = !self.interaction.ignore_workspace;
        self.interaction.ignore_workspace = true;
        self.interaction.templates = chosen;
        newly_ignoring_workspace.then_some(
            "ignoring the Workspace — dropping one of its templates means this interaction carries its own list",
        )
    }

    /// Add `mount` to the `scope` layer, or say why it would change nothing
    /// there.
    ///
    /// The two layers refuse a mount for different reasons. On the Workspace's,
    /// only a duplicate of its own grants is pointless. On this interaction's, a
    /// mount the Workspace already grants identically is too — an overlay row
    /// that changes nothing about the sandbox is worse than being told so.
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
                if !self.interaction.ignore_workspace && self.workspace.mounts.contains(&mount) {
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

/// How a mount's access reads, in the words the mount rows already use.
pub fn access_word(writable: bool) -> &'static str {
    if writable {
        "rw"
    } else {
        "ro"
    }
}

/// Bind the workspace — the directory the agent is launched in — writable or
/// read-only, for whichever layer is being edited.
///
/// It is the one mount an operator cannot reach with `m` and `x`: the server
/// assembles it from the Workspace's own host path rather than from any policy
/// row, so `rw` was the only thing it could ever be. Read-only is what a review
/// or an exploration of someone else's checkout wants, and it is a stronger
/// claim than a withdrawn network permission — nothing ORs write access back in.
pub fn cycle_workspace_access(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.scope == LaunchScope::Workspace {
        let writable = !app
            .launch
            .workspace
            .grants_writable_workspace(app.launch.writable_workspace_by_default());
        workspace_change(
            app,
            WorkspaceLaunchChange::SetWritableWorkspace(Some(writable)),
        );
        return app.show_action_message(if writable {
            "the workspace is mounted read-write for every launch in this Workspace"
        } else {
            "the workspace is mounted read-only for every launch in this Workspace"
        });
    }
    let message = app.launch.cycle_writable_workspace();
    app.show_action_message(message);
}

/// Say whether this interaction inherits the Workspace's templates and mounts or
/// carries the whole policy itself. Bound to `I`, for inheriting.
///
/// This one is only ever the interaction's own answer: there is nothing for the
/// Workspace's own policy to stand apart from.
pub fn toggle_ignore_workspace(app: &mut App) {
    if !app.allow_launch_edit() {
        return;
    }
    if app.launch.scope == LaunchScope::Workspace {
        return app
            .show_action_message("inheriting is this interaction's answer — press Tab to edit it");
    }
    app.launch.interaction.ignore_workspace = !app.launch.interaction.ignore_workspace;
    app.show_action_message(if app.launch.interaction.ignore_workspace {
        "ignoring the Workspace policy for this interaction"
    } else {
        "this interaction adds to the Workspace policy again"
    });
}

/// Open the prompt that edits `action`'s command in the focused layer,
/// starting from what that layer states now.
///
/// Unlike the rest of the policy this is editable on a live interaction: a
/// command changes nothing about the sandbox, and is resolved when it is run.
pub fn open_command_prompt(app: &mut App, action: ProjectAction) {
    if app.workspace_launch_pending > 0 {
        return app.show_warning("the Workspace launch policy is still saving");
    }
    let current = app
        .launch
        .policy(app.launch.scope)
        .command(action)
        .unwrap_or_default()
        .to_owned();
    app.launch.command_prompt = Some((action, current));
}

pub fn cancel_command_prompt(app: &mut App) {
    app.launch.command_prompt = None;
}

/// Store what the command prompt holds in the focused layer. Blank clears the
/// layer's command, so it inherits again.
pub fn confirm_command_prompt(app: &mut App) {
    let Some((action, text)) = app.launch.command_prompt.take() else {
        return;
    };
    let command = Some(text.trim().to_owned()).filter(|command| !command.is_empty());
    let scope = app.launch.scope;
    let message = match &command {
        Some(command) => format!("{} runs {command} for {}", action.name(), scope.subject()),
        None => format!(
            "{} inherits its command again for {}",
            action.name(),
            scope.subject()
        ),
    };
    match scope {
        LaunchScope::Workspace => {
            workspace_change(app, WorkspaceLaunchChange::SetCommand { action, command })
        }
        LaunchScope::Interaction => *app.launch.interaction.command_mut(action) = command,
    }
    app.show_action_message(message);
}

/// Adopt the templates the picker came back with.
#[cfg(test)]
pub fn set_templates(app: &mut App, chosen: Vec<String>) {
    set_templates_for(app, app.launch.scope, chosen);
}

/// Adopt a picker result for the layer it was opened from. The explicit scope
/// matters now that template discovery is asynchronous: changing the visible
/// pane while the server answers must not redirect the eventual edit.
pub fn set_templates_for(app: &mut App, scope: LaunchScope, chosen: Vec<String>) {
    if !app.allow_launch_edit() {
        return;
    }
    if scope == LaunchScope::Workspace {
        workspace_change(app, WorkspaceLaunchChange::SetTemplates(chosen));
        return;
    }
    let note = app.launch.set_interaction_templates(chosen);
    if let Some(note) = note {
        app.show_action_message(note);
    }
}

/// Open the prompt that adds an extra mount.
pub fn open_prompt(app: &mut App) {
    if !app.allow_mount_edit() {
        return;
    }
    if app.mount_change_restarts() {
        app.show_action_message(
            "adding a mount restarts the interaction; the conversation resumes",
        );
    }
    // Mounts are normally adjacent to the Workspace already on screen. Start
    // there so adding one only needs the part of the path that differs.
    app.launch.prompt = Some(
        app.workspace
            .root_or_current_directory()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    );
    refresh_paths(app);
}

pub fn cancel_prompt(app: &mut App) {
    app.launch.prompt = None;
    app.launch.paths = None;
}

/// Bring the list under the mount prompt up to date with what it holds, or
/// drop it with the prompt.
pub fn refresh_paths(app: &mut App) {
    match &app.launch.prompt {
        Some(text) => app
            .launch
            .paths
            .get_or_insert_with(Default::default)
            .update(text),
        None => app.launch.paths = None,
    }
}

/// Take the highlighted path into the prompt. A directory keeps its trailing
/// `/`, so the list goes straight on to what is under it.
pub fn take_highlighted_path(app: &mut App) {
    let highlighted = app.launch.paths.as_ref().and_then(PathList::highlighted);
    match highlighted {
        Some(path) => {
            app.launch.prompt = Some(path);
            refresh_paths(app);
        }
        None => app.show_warning("no path to take"),
    }
}

/// Move the cursor in the list under the mount prompt.
pub fn move_path_cursor(app: &mut App, step: fn(&mut FuzzyList, &[String])) {
    if let Some(paths) = app.launch.paths.as_mut() {
        step(&mut paths.list, &paths.rows);
    }
}

/// Accept what the prompt holds as an extra mount, or explain why it is not one
/// and leave the prompt open with the text still in it.
///
/// A source still being narrowed down — a name typed that is not a path that
/// exists — means the row the list has highlighted for it, as it would in any
/// fuzzy finder, so that row is what gets mounted.
pub fn confirm_prompt(app: &mut App) {
    let Some(mut text) = app.launch.prompt.clone() else {
        return;
    };
    if let Some(paths) = &app.launch.paths {
        let source = !text.contains(':');
        if source && paths.narrowing() && !mount::expand_home(&text).exists() {
            if let Some(highlighted) = paths.highlighted() {
                text = highlighted;
                app.launch.prompt = Some(text.clone());
                refresh_paths(app);
            }
        }
    }
    if !app.allow_mount_edit() {
        return cancel_prompt(app);
    }
    let parsed = match mount::parse(&text) {
        Ok(mount) => mount,
        Err(problem) => return app.show_error(problem),
    };
    cancel_prompt(app);
    let label = mount::label(&parsed);
    if app.launch.scope == LaunchScope::Workspace {
        if app.launch.workspace.mounts.contains(&parsed) {
            return app.show_warning("the Workspace policy already grants that mount");
        }
        workspace_change(app, WorkspaceLaunchChange::AddMounts(vec![parsed]));
        app.note_mount_change();
        return show_mount_change(
            app,
            format!("added {label} to every launch in this Workspace"),
        );
    }
    add_mount_to(app, app.launch.scope, parsed);
}

/// Add `mount` to this interaction's own layer from outside the Driva view —
/// the message editor's path prompt, a link's menu — by the same path the
/// view's own prompt takes: refused while the agent is working, and an idle
/// interaction restarted to take it. Returns whether the mount was added.
///
/// The layer is not the operator's choice of pane here but a property of what
/// they are doing: a path that came up in one message is this interaction's
/// business, and `U` moves it up a layer if it turns out to be the Workspace's.
pub fn grant_interaction_mount(app: &mut App, mount: LaunchMount) -> bool {
    if !app.allow_mount_edit() {
        return false;
    }
    add_mount_to(app, LaunchScope::Interaction, mount)
}

/// Add `mount` to the client-held `scope` layer, ask for the restart that
/// applies it to an idle interaction, and say so.
fn add_mount_to(app: &mut App, scope: LaunchScope, mount: LaunchMount) -> bool {
    let label = mount::label(&mount);
    match app.launch.add_mount_to(scope, mount) {
        Ok(()) => {
            app.note_mount_change();
            show_mount_change(app, format!("added {label} to {}", scope.subject()));
            true
        }
        Err(reason) => {
            app.show_error(reason);
            false
        }
    }
}

/// Drop the selected mount from the layer being edited.
pub fn remove_selected_mount(app: &mut App) {
    if !app.allow_mount_edit() {
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
            return app.show_warning("the Workspace policy has no mounts to remove");
        };
        workspace_change(app, WorkspaceLaunchChange::RemoveMount(removed.clone()));
        app.note_mount_change();
        return show_mount_change(
            app,
            format!(
                "removed {} from every launch in this Workspace",
                mount::label(&removed)
            ),
        );
    }
    match app.launch.remove_mount() {
        Ok(removed) => {
            app.note_mount_change();
            show_mount_change(
                app,
                format!(
                    "removed {} from {}",
                    mount::label(&removed),
                    scope.subject()
                ),
            );
        }
        Err(reason) => app.show_error(reason),
    }
}

/// Report a mount change, adding that the interaction is about to restart for
/// it when it is: the busy notice that follows is easy to miss, and the change
/// is why it happened.
fn show_mount_change(app: &mut App, message: String) {
    app.show_action_message(if app.restart_for_mounts {
        format!("{message} — restarting the interaction to apply it")
    } else {
        message
    });
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
        return app.show_warning("this interaction adds nothing to the Workspace policy");
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
        Status::Pending | Status::Stopped(_) | Status::Ended { .. }
    )
}

/// Whether mounts can be added or removed: wherever the rest of the policy can
/// be edited, and also while the interaction is idle.
///
/// A live sandbox's mounts are still fixed for its lifetime, but an idle agent
/// is not in the middle of anything, so the edit is applied by restarting it
/// under the new policy (see [`App::note_mount_change`]). The conversation
/// resumes natively, so the restart costs a moment rather than any context.
/// Mounts are the one part of the policy this is offered for: they are what a
/// conversation turns out to need mid-way, when a file it is asked about lives
/// somewhere the sandbox cannot see.
pub fn mounts_editable(status: &Status) -> bool {
    editable(status) || status.is_idle()
}

#[cfg(test)]
mod tests;

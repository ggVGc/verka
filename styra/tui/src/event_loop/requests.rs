//! Carrying out the [`Request`]s key handlers queue on the [`App`]: the work
//! that talks to the server, opens a picker of its own, or ends the loop.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::navigation::{
    ensure_listed, narrow_navigator_to_jump, open_interactions_in_workspace, preview_workspace_jump,
};
use super::screen::blocked_on;
use super::sync::{refresh_processes, refresh_quota};
use super::{Loop, RunOutcome};
use crate::app::{App, Focus, Request, View};
use crate::config::Configuration;
use crate::launch::LaunchScope;
use crate::picker;
use crate::session::{self, Attachment};
use crate::workspace_jump::WorkspaceJump;
use styra_protocol::LogEntry;
use styra_server::Client;

/// What the screen says while a Git branch and linked workspace are made,
/// whichever key asked for them.
const CREATING_WORKTREE: &str = "creating a Git branch and workspace…";

/// The restart that follows making a linked workspace.
const RESTARTING_IN_WORKTREE: &str = "restarting the interaction in the linked workspace…";

impl Loop<'_> {
    /// Every request the last round queued is carried out here, at the top of
    /// the next, before anything is drawn. A key handler that ends its round
    /// early — and most of them do — still restarts the loop, so nothing it
    /// asked for can be left sitting in the queue behind later keys, as the
    /// diff's keys once were. A picker that the operator backs out of leaves
    /// the session as it was, so those requests go on to the next one rather
    /// than returning.
    pub(super) fn carry_out_requests(&mut self, workspace_id: &str) -> Result<Option<RunOutcome>> {
        while let Some(request) = self.app.take_request() {
            self.dirty = true;
            if let Some(outcome) = self.carry_out(request, workspace_id)? {
                return Ok(Some(outcome));
            }
        }
        Ok(None)
    }

    fn carry_out(&mut self, request: Request, workspace_id: &str) -> Result<Option<RunOutcome>> {
        let app = &mut *self.app;
        let client = self.client;
        match request {
            Request::Rebase => start_rebase(app, client, None),
            Request::Retarget(target) => start_rebase(app, client, Some(&target)),
            Request::Quit => return Ok(Some(RunOutcome::Quit)),
            Request::Integrate(_) | Request::GenerateIntegrationMessage => {}
            Request::Workspace => return self.pick_workspace(workspace_id),
            Request::OpenSession(id) => return Ok(Some(RunOutcome::OpenSession(id))),
            Request::OpenBranch(id) => return Ok(Some(RunOutcome::OpenBranch(id))),
            Request::JumpBack => return Ok(Some(RunOutcome::JumpBack)),
            Request::JumpForward => return Ok(Some(RunOutcome::JumpForward)),
            Request::InteractionHistory => return self.pick_from_history(),
            Request::Sessions => return self.pick_session(workspace_id),
            Request::Worktrees => return self.pick_worktree(workspace_id),
            Request::Interactions => {
                ensure_listed(app, client)?;
                if app.interactions.items.is_empty() {
                    app.push_log(LogEntry::warn("no live interactions on the server"));
                    return Ok(None);
                }
                let workspaces = client.list_workspaces()?;
                app.set_view(View::Events);
                app.focus = Focus::List;
                app.interactions.show(workspaces);
            }
            Request::WorkspaceJump { listed } => self.open_workspace_jump(listed)?,
            // The overview reads the same snapshot the loop refreshes on its
            // timer, but the tiles name their Workspaces, and that list is
            // only fetched when asked for. It is best-effort: a grid a moment
            // stale is still the grid.
            Request::Overview => {
                if let Ok(workspaces) = client.list_workspaces() {
                    app.interactions.workspaces = workspaces;
                }
                app.interactions.close();
                app.overview.open();
                app.set_view(View::Overview);
                app.focus = Focus::List;
            }
            Request::ShowInteraction(id) => self.show_interaction(id),
            Request::MessageInteraction(id) => self.message_interaction(id),
            Request::NextLiveInteraction => self.next_live_interaction(),
            Request::NextWorkingInteraction => {
                let Some(next) = app
                    .interactions
                    .next_active(&app.session_id, app.workspace.id.as_deref())
                else {
                    app.show_warning("no other interaction is actively working");
                    return Ok(None);
                };
                self.make_interaction_current(next);
            }
            Request::NewSession => return Ok(Some(RunOutcome::NewSession)),
            Request::CreateWorktree { message } => {
                return self.create_worktree(message, workspace_id)
            }
            Request::ApplySelection => {
                let Attachment::Attached { .. } = self.live else {
                    return Ok(None);
                };
                let selection = app.selection.clone();
                match client.set_session_selection(&app.session_id, &selection) {
                    Ok(()) => app.show_action_message(format!("model set to {}", selection.model)),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not switch to {}: {error:#}",
                        selection.model
                    ))),
                }
            }
            Request::ConvertProvider(selection) => {
                let previous = app.selection.clone();
                app.set_selection(selection);
                let live = &mut *self.live;
                let restarted = blocked_on(
                    self.terminal,
                    self.app,
                    "switching provider and restarting the interaction…",
                    |app| session::restart(app, client, live),
                )?;
                match restarted {
                    Ok(_) => return Ok(Some(RunOutcome::OpenSession(self.app.session_id.clone()))),
                    Err(error) => {
                        self.app.set_selection(previous);
                        self.app
                            .show_error(format!("could not switch provider: {error:#}"));
                    }
                }
            }
            Request::Templates => {
                // Which templates the picker starts from is the layer being
                // edited. For the Workspace's own that is exactly its list. For
                // this interaction's, the picker offers and returns the whole
                // layering a launch would apply — the Workspace's templates
                // included, since those are as much part of what the operator is
                // choosing as their own. Turning that choice back into an
                // overlay is `App`'s job.
                let current = match app.launch.scope {
                    LaunchScope::Workspace => app.launch.workspace.templates.clone(),
                    LaunchScope::Interaction => app.launch.effective().templates,
                };
                let request_id = self
                    .launch_effects
                    .submit_templates(workspace_id.to_owned());
                app.template_picker = Some(picker::TemplatePicker::loading(
                    request_id,
                    workspace_id.to_owned(),
                    app.launch.scope,
                    current,
                ));
            }
            Request::ChangeWorkspaceLaunch {
                change,
                clear_interaction,
            } => self.launch_effects.submit_workspace_launch(
                app,
                workspace_id.to_owned(),
                change,
                clear_interaction,
            ),
            // Parsing is the server's, since it holds the session's recorded
            // contract and the journal the answer is read from; this client
            // only asks and renders.
            Request::Answer { contract } => {
                let id = app.session_id.clone();
                if id.is_empty() {
                    app.answer.set(Err("no session to answer from yet".into()));
                    return Ok(None);
                }
                let answer = match contract {
                    Some(contract) => client.turn_answer_as(&id, contract),
                    None => client.turn_answer(&id),
                };
                app.answer.set(answer.map_err(|error| format!("{error:#}")));
            }
            Request::Quota => refresh_quota(app, client),
            Request::Processes => {
                refresh_processes(app, client);
            }
            // The setting is the server's to keep — it is stored with the
            // Session and acted on long after this client has gone — so what
            // is shown is what the server accepted, not what was pressed.
            Request::SetAutoRetry(enabled) => {
                match client.set_interaction_auto_retry(&app.session_id, enabled) {
                    Ok(()) => {
                        app.auto_retry = enabled;
                        app.show_action_message(if enabled {
                            "rate-limit retry on — this session will be asked again after a reset"
                        } else {
                            "rate-limit retry off"
                        });
                    }
                    Err(error) => report_failure(
                        app,
                        format!("could not change the rate-limit retry: {error:#}"),
                    ),
                }
            }
            Request::SetAutoCommit(enabled) => {
                match client.set_interaction_auto_commit(&app.session_id, enabled) {
                    Ok(()) => {
                        app.auto_commit = enabled;
                        app.show_action_message(if enabled {
                            "auto-commit on — each turn's work is committed when it goes idle"
                        } else {
                            "auto-commit off"
                        });
                    }
                    Err(error) => {
                        report_failure(app, format!("could not change auto-commit: {error:#}"))
                    }
                }
            }
            Request::EditFile => {
                if let Some(path) = app.selected_file_path() {
                    open_path(app, self.config, &path, None);
                }
            }
            Request::OpenPath(path) => open_path(app, self.config, &path, None),
            Request::OpenPathAt(path, line) => open_path(app, self.config, &path, line),
            Request::OpenUrl(url) => open_url(app, self.config, &url),
            Request::OpenDirectory => open_directory(app, self.config),
            Request::ShowDiffAt {
                id,
                path,
                line,
                fullscreen,
            } => self.show_diff(&id, fullscreen, Some((path, line))),
            Request::ShowDiff(id) => self.show_diff(&id, false, None),
            Request::ShowDiffFullscreen(id) => self.show_diff(&id, true, None),
            Request::ProjectAction(action) => run_project_action(app, client, self.config, action),
            Request::GenerateEditReport(only) => self.edit_reports.start(app, client, only),
            Request::OpenShell => {
                match crate::terminal::open_shell(client, &app.session_id, self.config) {
                    Ok(program) => app.show_action_message(format!("opened shell in {program}")),
                    Err(error) => app.push_log(LogEntry::error(format!(
                        "could not open session shell: {error:#}"
                    ))),
                }
            }
        }
        Ok(None)
    }

    fn pick_workspace(&mut self, workspace_id: &str) -> Result<Option<RunOutcome>> {
        let app = &mut *self.app;
        let client = self.client;
        let mut workspaces = client.list_workspaces()?;
        // The picker keeps the listing it is handed fresh while it is
        // open, during which this loop is not polling, so what it
        // ends on is the newest listing there is.
        ensure_listed(app, client)?;
        let mut interactions = app.interactions.snapshot();
        let query = app.interactions_query();
        let choice = picker::run_workspace_picker(
            self.terminal,
            client,
            &mut workspaces,
            &mut interactions,
            &query,
            None,
        );
        app.interactions.refresh(interactions);
        // Backing out never leaves Styra, even with no Workspace (or a
        // deleted one) behind the list: it lands on the interaction
        // view, where `q` is still the way out.
        let Some(choice) = choice? else {
            app.set_view(View::Events);
            app.focus = Focus::List;
            return Ok(None);
        };
        let workspace = match choice {
            // Looking the Workspace up again records the access, which
            // is what floats it to the top of the picker next time. The
            // summary the picker already holds stands in if the server
            // cannot answer.
            picker::WorkspaceChoice::Existing(workspace) => {
                client.workspace(&workspace.id).unwrap_or(workspace)
            }
            // A fresh interaction is asked for outright, so neither
            // live work nor the Session list stands in the way.
            picker::WorkspaceChoice::New(workspace) => {
                return Ok(Some(RunOutcome::OpenWorkspace {
                    workspace: Box::new(client.workspace(&workspace.id).unwrap_or(workspace)),
                    session_id: None,
                    open_interactions: false,
                }));
            }
            picker::WorkspaceChoice::CreateCurrentDirectory => {
                let host_path = session::resolve_workspace(None)?;
                session::workspace_for_host(client, &host_path)?
            }
        };
        // A Workspace with work in flight is entered at that work, the
        // way `v` enters it: the live list opens with the cursor on the
        // Workspace's first Interaction, so the operator sees the rest
        // of the live list without being asked which Session they meant.
        if crate::interactions::first_live_in_workspace(&app.interactions.items, &workspace.id)
            .is_some()
        {
            open_interactions_in_workspace(app, client, &workspace.id)?;
            return Ok(None);
        }
        let mut sessions = client.list_sessions(&workspace.id)?;
        if sessions.is_empty() {
            return Ok(Some(RunOutcome::OpenWorkspace {
                workspace: Box::new(workspace),
                session_id: None,
                open_interactions: false,
            }));
        }
        // Starting fresh from this list enters the Workspace that was
        // just chosen with no Session loaded, which is the same place
        // entering a Workspace that has no history at all lands.
        if let Some(choice) = picker::run_session_picker(
            self.terminal,
            client,
            &crate::workspace::display_name(&workspace),
            &mut sessions,
            &app.interactions.items,
            None,
            true,
        )? {
            return Ok(Some(RunOutcome::OpenWorkspace {
                workspace: Box::new(workspace),
                session_id: match choice {
                    picker::SessionChoice::Open(id) => Some(id),
                    picker::SessionChoice::New => None,
                },
                open_interactions: false,
            }));
        }
        if !client
            .list_workspaces()?
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return Ok(Some(RunOutcome::Quit));
        }
        Ok(None)
    }

    fn pick_from_history(&mut self) -> Result<Option<RunOutcome>> {
        let app = &mut *self.app;
        if self.history.entries().is_empty() {
            app.show_warning("no interactions in history");
            return Ok(None);
        }
        let workspaces = self.client.list_workspaces().unwrap_or_default();
        let mut sessions = Vec::new();
        // Live rows supply their latest names. Stored Sessions fill
        // in history entries that have since stopped or completed.
        if self
            .history
            .entries()
            .iter()
            .any(|id| app.interactions.current(id).is_none())
        {
            for workspace in &workspaces {
                if let Ok(stored) = self.client.list_sessions(&workspace.id) {
                    sessions.extend(stored);
                }
            }
        }
        let rows =
            crate::history_picker::rows(self.history, &app.interactions, &workspaces, &sessions);
        Ok(crate::history_picker::run(self.terminal, &rows)?.map(RunOutcome::JumpTo))
    }

    fn pick_session(&mut self, workspace_id: &str) -> Result<Option<RunOutcome>> {
        let app = &mut *self.app;
        let mut sessions = self.client.list_sessions(workspace_id)?;
        if sessions.is_empty() {
            app.push_log(LogEntry::warn("no sessions found in the current Workspace"));
            return Ok(None);
        }
        Ok(
            match picker::run_session_picker(
                self.terminal,
                self.client,
                app.workspace.name.as_deref().unwrap_or("styra"),
                &mut sessions,
                &app.interactions.items,
                Some(&app.session_id),
                true,
            )? {
                Some(picker::SessionChoice::Open(id)) => Some(RunOutcome::OpenSession(id)),
                // The same new interaction `n` starts from the main view:
                // it inherits the context being viewed, in this Workspace.
                Some(picker::SessionChoice::New) => Some(RunOutcome::NewSession),
                None => None,
            },
        )
    }

    /// Listed here rather than inside the picker so that a server that
    /// cannot answer — one older than the request — is a logged line and not
    /// the end of the client.
    fn pick_worktree(&mut self, workspace_id: &str) -> Result<Option<RunOutcome>> {
        let client = self.client;
        let worktrees = match client.list_worktrees(Some(workspace_id)) {
            Ok(worktrees) => worktrees,
            Err(error) => {
                self.app.push_log(LogEntry::error(format!(
                    "could not list worktrees: {error:#}"
                )));
                return Ok(None);
            }
        };
        let Some(choice) = crate::worktree_picker::run_worktree_picker(
            self.terminal,
            client,
            worktrees,
            workspace_id,
            &self.app.session_id,
        )?
        else {
            return Ok(None);
        };
        // With every Workspace listed the Session may be in another
        // one, which has to be entered for the view to follow it.
        if choice.workspace_id != workspace_id {
            if let Some(workspace) = client
                .list_workspaces()?
                .into_iter()
                .find(|workspace| workspace.id == choice.workspace_id)
            {
                return Ok(Some(RunOutcome::OpenWorkspace {
                    workspace: Box::new(workspace),
                    session_id: Some(choice.session_id),
                    open_interactions: false,
                }));
            }
        }
        Ok(Some(RunOutcome::OpenSession(choice.session_id)))
    }

    /// The listing the loop keeps, as `a` takes, so the choice offered is of
    /// the work running at the last refresh. `V` offers what the navigator
    /// would list rather than only live work, so it opens on the list itself:
    /// the grid tiles live work alone and could not show most of what it
    /// offers.
    fn open_workspace_jump(&mut self, listed: bool) -> Result<()> {
        let app = &mut *self.app;
        ensure_listed(app, self.client)?;
        let workspaces = self.client.list_workspaces()?;
        let interactions = &app.interactions.items;
        let mut jump = if listed {
            WorkspaceJump::listed(&workspaces, interactions, |interaction| {
                app.interactions.lists(interaction)
            })
        } else {
            WorkspaceJump::new(&workspaces, interactions)
        };
        if jump.is_empty() {
            app.show_warning(if listed {
                "no Workspace has interactions the list shows"
            } else {
                "no Workspace has live interactions"
            });
            return Ok(());
        }
        if !listed && app.view == View::Overview {
            jump.grid = Some((
                app.overview.clone(),
                app.interactions.only_current_workspace,
            ));
        }
        app.interactions.workspaces = workspaces;
        app.workspace_jump = Some(jump);
        preview_workspace_jump(app);
        narrow_navigator_to_jump(app);
        Ok(())
    }

    /// Loaded outright, as `n` does. A load that fails leaves the overview
    /// up, and says so where the operator is looking rather than only in the
    /// log.
    fn show_interaction(&mut self, id: String) {
        if id == self.app.session_id {
            self.app.set_view(View::Events);
            self.history.record(&id);
            return;
        }
        let Some(interaction) = self.app.interactions.current(&id).cloned() else {
            self.app
                .show_warning("that interaction is no longer running");
            return;
        };
        self.make_interaction_current(interaction);
        if self.app.session_id == id {
            self.history.record(&id);
        } else {
            self.app
                .show_error("could not open that interaction; the log (ctrl-l) says why");
        }
    }

    /// As [`Self::show_interaction`], but the grid stays up behind the
    /// message box, so the operator can go on to the next tile once this one
    /// is answered.
    fn message_interaction(&mut self, id: String) {
        let Some(interaction) = self.app.interactions.current(&id).cloned() else {
            self.app
                .show_warning("that interaction is no longer running");
            return;
        };
        self.make_interaction_current(interaction);
        self.app.set_view(View::Overview);
        if self.app.session_id == id {
            self.app.enter_input();
        } else {
            self.app
                .show_error("could not open that interaction; the log (ctrl-l) says why");
        }
    }

    /// The interaction is loaded outright rather than cursored, and the
    /// navigator is left as it was: open or closed. Newly idle work takes
    /// priority; otherwise this walks every live interaction.
    fn next_live_interaction(&mut self) {
        let app = &mut *self.app;
        // The client's snapshot is at most a refresh old, which is
        // fresh enough to step by: one stopped since then still loads.
        let overview = (app.view == View::Overview).then(|| app.overview.clone());
        let next = if let Some(overview) = &overview {
            overview.next_attention_or_live(
                &app.interactions,
                &app.session_id,
                app.workspace.id.as_deref(),
            )
        } else {
            app.interactions
                .next_attention_or_live(&app.session_id, app.workspace.id.as_deref())
        };
        let Some(next) = next else {
            app.show_warning("no other interaction is running");
            return;
        };
        let target_id = next.id.clone();
        self.make_interaction_current(next);
        let Some(mut overview) = overview else {
            return;
        };
        let app = &mut *self.app;
        if app.session_id == target_id {
            if overview.running_only
                && overview
                    .indices(&app.interactions, app.workspace.id.as_deref())
                    .iter()
                    .all(|index| app.interactions.items[*index].id != app.session_id)
            {
                overview.toggle_running_only();
            }
            overview.open();
        }
        app.overview = overview;
        app.set_view(View::Overview);
    }

    /// Naming the branch asks the model for a topic and the checkout copies
    /// the repository out, which together take long enough to look like a
    /// hang. `Ctrl+W` and `Ctrl-Enter` differ only in what the checkout is
    /// made for: a Session already running, which has to be restarted into
    /// it — carrying the message, if `Ctrl-Enter` sent one — or one that the
    /// first prompt launches there.
    fn create_worktree(
        &mut self,
        message: Option<String>,
        workspace_id: &str,
    ) -> Result<Option<RunOutcome>> {
        let client = self.client;
        let live = &mut *self.live;
        if let Some(message) = message.as_ref().filter(|_| self.app.session_id.is_empty()) {
            // The launch branches and checks out before it sends, and
            // reports its own failure — restoring the message box.
            let message = message.clone();
            blocked_on(self.terminal, self.app, CREATING_WORKTREE, |app| {
                session::submit_message(app, client, workspace_id, live, message, true)
            })?;
            return Ok(None);
        }
        let session_id = self.app.session_id.clone();
        let created = blocked_on(self.terminal, self.app, CREATING_WORKTREE, |_| {
            client.create_session_worktree(&session_id)
        })?;
        if let Err(error) = created {
            self.app
                .show_error(format!("could not create a linked workspace: {error}"));
            // Nothing was sent: the message waits in the box for a
            // plain Enter, or for another try.
            if let Some(message) = message {
                self.app.set_input(message);
                self.app.enter_input();
            }
            return Ok(None);
        }
        if let Some(message) = message {
            let sent = blocked_on(self.terminal, self.app, RESTARTING_IN_WORKTREE, |app| {
                session::restart_and_send(app, client, live, message)
            })?;
            if sent {
                return Ok(Some(RunOutcome::OpenSession(session_id)));
            }
            self.app.show_error(
                "created the linked workspace, but could not restart in it; the message is back in the box",
            );
            self.app.enter_input();
            return Ok(None);
        }
        // The checkout is read when the agent launches, so the
        // interaction is restarted into it rather than left running in
        // the directory it started in. Reopening the Session afterwards
        // is what puts the checkout on screen: the view then reads its
        // directory from the revived interaction.
        let restarted = blocked_on(self.terminal, self.app, RESTARTING_IN_WORKTREE, |app| {
            session::restart(app, client, live)
        })?;
        match restarted {
            Ok(_) => return Ok(Some(RunOutcome::OpenSession(session_id))),
            Err(error) => self.app.show_error(format!(
                "created the linked workspace, but could not restart in it ({error}); it will be used when this Session next launches"
            )),
        }
        Ok(None)
    }

    /// Read interaction `id`'s checkout diff into the preview pane, or the
    /// fullscreen view. Reading it runs `git`, which is why it is a request.
    /// With `at`, the diff is read afresh and opened at that file and line,
    /// and left as it was if the file is not among its changes; without, the
    /// diff on screen is reopened against the checkout.
    fn show_diff(&mut self, id: &str, fullscreen: bool, at: Option<(PathBuf, Option<u32>)>) {
        let app = &mut *self.app;
        let Some(interaction) = app.interactions.items.iter().find(|row| row.id == id) else {
            app.show_error("no interaction to diff");
            return;
        };
        let target = match crate::checkout_diff::target(self.client, interaction) {
            Ok(target) => target,
            Err(error) => {
                app.show_error(format!("could not open snapshot diff: {error:#}"));
                return;
            }
        };
        // Reopening the pane already open, as a switch of interaction does,
        // leaves the keys with whichever side had them.
        let reopened = !fullscreen && at.is_none() && app.diff_preview;
        let diff = match at {
            Some((path, line)) => {
                let mut diff = styra_diff::CheckoutDiff::read(target);
                if let Err(reason) = diff.jump_to(&path, line) {
                    app.show_warning(reason);
                    return;
                }
                diff
            }
            None => crate::checkout_diff::reopen(app.checkout_diff.take(), target),
        };
        app.checkout_diff = Some(diff);
        self.edit_reports.load(app, self.client);
        self.diff_refreshed = Instant::now();
        if !reopened {
            app.diff_interaction_focus = !fullscreen && app.interactions.open;
        }
        app.diff_preview = !fullscreen;
        app.diff_turn = false;
        if fullscreen {
            app.set_view(View::CheckoutDiff);
        } else if !matches!(app.view, View::Events | View::Files) {
            app.set_view(View::Events);
        }
    }
}

fn start_rebase(app: &mut App, client: &Client, target: Option<&str>) {
    let id = app.session_id.clone();
    let started = match target {
        Some(target) => client.retarget_session(&id, target),
        None => client.rebase_session(&id),
    };
    app.rebase_status = Some(styra_protocol::RebaseStatus {
        finished: started.is_err(),
        succeeded: false,
        output: String::new(),
        source_commit: String::new(),
        message: match &started {
            Ok(()) => "Starting rebase…".into(),
            Err(error) => format!("Could not rebase: {error:#}"),
        },
    });
    if started.is_ok() {
        app.pending_rebase = Some(id);
    }
}

/// Shown and logged both: the notice is cut to the terminal's width.
fn report_failure(app: &mut App, message: String) {
    app.show_error(message.clone());
    app.push_log(LogEntry::error(message));
}

/// Use the interaction's checkout for project commands, including stored Sessions.
fn project_action_directory(app: &App, client: &Client) -> Result<PathBuf> {
    if let Some(root) = app.workspace.root() {
        return Ok(root.to_path_buf());
    }
    // Stored Sessions have no live sandbox root, but can still own a checkout.
    if let Some(workspace_id) = app.workspace.id.as_deref() {
        for worktree in client.list_worktrees(Some(workspace_id))? {
            if worktree.exists
                && worktree
                    .sessions
                    .iter()
                    .any(|session| session.id == app.session_id)
            {
                if let Some(path) = worktree.worktree {
                    return Ok(path);
                }
            }
        }
    }
    app.workspace
        .host_path
        .clone()
        .or_else(|| app.workspace.root_or_current_directory())
        .ok_or_else(|| anyhow::anyhow!("could not determine the Workspace directory"))
}

fn run_project_action(
    app: &mut App,
    client: &Client,
    config: &dyn Configuration,
    action: styra_protocol::ProjectAction,
) {
    let directory = match project_action_directory(app, client) {
        Ok(directory) => directory,
        Err(error) => {
            app.show_error(format!("could not locate the project: {error:#}"));
            return;
        }
    };
    let result = crate::terminal::project_command(&directory, &app.launch.effective(), action)
        .and_then(|command| {
            let program = crate::terminal::open_project_action(&directory, &command, config)?;
            Ok((program, command))
        });
    match result {
        Ok((program, command)) => app.show_action_message(format!(
            "{}: {command} — opened in {program}",
            action.name()
        )),
        Err(error) => app.show_error(format!(
            "could not {} the project: {error:#}",
            action.name()
        )),
    }
}

/// Hand one host path to the configured opener, at `line` when one is known,
/// reporting the outcome the way
/// [`terminal::open_shell`](crate::terminal::open_shell) does.
///
/// The one place a file is opened, so the Files view, a typed `files` answer,
/// a selected Markdown link, and the diff view obey the same configuration
/// and say the same thing afterwards.
fn open_path(app: &mut App, config: &dyn Configuration, path: &Path, line: Option<u32>) {
    let mut command = match line {
        Some(line) => config.open_file_at(path, line),
        None => config.open_file(path),
    };
    // Named in both messages, because what opens a file is configuration: an
    // operator who set it needs to see which command Styra actually ran.
    let described = crate::terminal::describe(&command);
    match crate::terminal::spawn_detached(&mut command) {
        Ok(()) => app.show_action_message(format!("opened {described}")),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not open {}: {error:#}",
            path.display()
        ))),
    }
}

/// Hand a web address to the configured browser, reporting the outcome as
/// [`open_path`] does.
fn open_url(app: &mut App, config: &dyn Configuration, url: &str) {
    let mut command = config.open_url(url);
    let described = crate::terminal::describe(&command);
    match crate::terminal::spawn_detached(&mut command) {
        Ok(()) => app.show_action_message(format!("opened {described}")),
        Err(error) => app.push_log(LogEntry::error(format!("could not open {url}: {error:#}"))),
    }
}

/// Open a terminal in the directory the interaction works in.
fn open_directory(app: &mut App, config: &dyn Configuration) {
    let Some(directory) = app.workspace.working_directory_or_current() else {
        app.show_warning("no directory to open a terminal in");
        return;
    };
    match crate::terminal::open_directory(&directory, config) {
        Ok(program) => {
            app.show_action_message(format!("opened {program} in {}", directory.display()))
        }
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not open a terminal in {}: {error:#}",
            directory.display()
        ))),
    }
}

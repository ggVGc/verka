use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::activity::Status;
use crate::app::{App, LaunchPolicy};
use crate::launch;
use styra_server::agent::Selection;
use styra_server::protocol::{
    CreateSession, CreateWorkspace, PlanSession, ResumeSession, SendMessage, SessionInfo,
};
use styra_server::{
    Client, Contract, InteractionSnapshot, InteractionSnapshotScope, InteractionSummary,
    InteractionUpdate, LogEntry, SessionSummary, WorkspaceSummary,
};

/// The live-agent side of the interactive loop: no process yet (awaiting the
/// operator's first message), a spawned agent, or a replayed journal with no
/// live agent to send to.
#[derive(Debug, PartialEq, Eq)]
pub enum Live {
    /// Nothing has been launched; the event loop spawns the session itself
    /// the moment the operator submits a message from `Focus::Input`.
    Pending,
    /// A server-owned agent process, addressed by id. `cursor` makes polling
    /// incremental and preserves the server's update order.
    Running { session_id: String, cursor: u64 },
    /// A replayed journal (`--view`, a reopened Session, or one this client
    /// lost its connection to); no live agent is attached. Sending a message
    /// resumes it through the provider's native mechanism.
    Viewing,
}

pub fn resolve_workspace(workspace: Option<&Path>) -> Result<PathBuf> {
    let raw = match workspace {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("determining the current directory")?,
    };
    raw.canonicalize()
        .with_context(|| format!("workspace directory {} must exist", raw.display()))
}

pub fn workspace_for_host(client: &Client, host_path: &Path) -> Result<WorkspaceSummary> {
    let canonical = host_path.canonicalize()?;
    if let Some(workspace) = find_workspace_for_host(&client.list_workspaces()?, &canonical) {
        return client.workspace(&workspace.id);
    }
    create_workspace(client, canonical, None)
}

/// Create a Workspace and associate the nearest enclosing Git checkout when
/// there is one. Repository discovery belongs to the creating client because
/// an omitted repository in the wire request deliberately means "none".
pub fn create_workspace(
    client: &Client,
    host_path: PathBuf,
    name: Option<String>,
) -> Result<WorkspaceSummary> {
    let git_repository = enclosing_git_repository(&host_path);
    client.create_workspace(&CreateWorkspace {
        host_path,
        name,
        git_repository,
    })
}

fn enclosing_git_repository(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Find the durable Workspace associated with an already-canonical host path.
pub fn find_workspace_for_host(
    workspaces: &[WorkspaceSummary],
    host_path: &Path,
) -> Option<WorkspaceSummary> {
    workspaces
        .iter()
        .find(|workspace| workspace.host_path == host_path)
        .cloned()
}

pub fn all_sessions(client: &Client) -> Result<Vec<SessionSummary>> {
    let mut sessions = Vec::new();
    for workspace in client.list_workspaces()? {
        sessions.extend(client.list_sessions(&workspace.id)?);
    }
    sessions.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(sessions)
}

/// Ask the server for a session. `launch` is this launch's own policy only:
/// the Workspace's standing one is added on the server, so the client cannot
/// launch under a policy it never showed.
/// Contracts in the order `Ctrl-T` walks them, taken from the server's own
/// list so a contract added there is offered here without a second edit. The
/// cycle ends back at an untyped turn, so the operator can always get out of
/// one without leaving the message box.
pub const CONTRACTS: [Contract; 4] = styra_server::contract::CONTRACTS;

/// The next return contract after `current`, wrapping back to an untyped turn.
/// Shared by both message boxes, so `Ctrl-T` walks the same cycle in each.
pub fn next_contract(current: Option<Contract>) -> Option<Contract> {
    match current {
        None => Some(CONTRACTS[0]),
        Some(current) => CONTRACTS
            .iter()
            .position(|contract| *contract == current)
            .and_then(|index| CONTRACTS.get(index + 1))
            .copied(),
    }
}

/// One turn as this client describes it: the operator's text, the selection it
/// should run under, and the shape it asks its reply to take.
///
/// Takes the selection rather than the whole [`App`] so launch and resume paths
/// can build the same wire request without borrowing unrelated display state.
pub fn turn(message: &str, selection: &Selection, contract: Option<Contract>) -> SendMessage {
    let turn = SendMessage::new(message).under(selection.clone());
    match contract {
        Some(contract) => turn.asking_for(contract),
        None => turn,
    }
}

/// `contract` types the seed message, when the operator asked the very first
/// turn for a shape. A session is not typed as a whole — every later turn
/// chooses for itself.
pub fn create_session(
    client: &Client,
    launch: &LaunchPolicy,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
    contract: Option<Contract>,
) -> Result<SessionInfo> {
    client.create_session(&CreateSession {
        workspace_id: workspace_id.to_owned(),
        selection: selection.clone(),
        launch: launch.clone(),
        message: seed.map(str::to_owned),
        name: None,
        contract,
    })
}

/// While nothing is running there is no sandbox to describe, but there is one
/// decided: ask the server what an interaction started under the current
/// selection and policy would run in, so the Driva view answers "what will this
/// agent be able to touch" rather than waiting for the next message.
pub fn ensure_driva_plan(app: &mut App, client: &Client, workspace_id: &str) {
    // Not just the blank screen: a stopped or ended Session is resumed under
    // whatever the policy says when the next message is sent, so it is planned
    // the same way. Gated on the policy being editable rather than on `live`,
    // so the two questions ("can this be changed" and "is what is shown still
    // true") cannot drift apart.
    if !launch::wants_plan(app) {
        return;
    }
    let selection = app.selection.clone();
    // Sent as this interaction's own half alone, but remembered as the merge:
    // the server answers for the whole policy, having its own copy of the
    // Workspace's, so the merge is what the plan is keyed on.
    let overlay = app.launch.interaction.clone();
    let effective = app.launch.effective();
    let planned = client.plan_session(&PlanSession {
        workspace_id: workspace_id.to_owned(),
        selection: selection.clone(),
        launch: overlay,
    });
    match planned {
        Ok(options) => app.launch.plan(selection, effective, Some(options)),
        Err(error) => {
            app.launch.plan(selection, effective, None);
            // An edit the server rejects (an unknown template, a path that is
            // not there) lands here. The retained plan is the last one that
            // did resolve, so say so where the operator is looking rather than
            // only in the log, otherwise a rejected edit reads as an accepted
            // one that simply changed nothing.
            app.show_action_message(format!("launch policy not applied: {error}"));
            app.push_log(LogEntry::warn(format!(
                "could not describe the sandbox a new interaction would launch in: {error:#}"
            )));
        }
    }
}

/// Spawn a session and wrap it in a fresh `App`. Used for the CLI's trailing
/// prompt on first launch, the only case where the agent starts before the
/// event loop takes over.
pub fn launch_live_session(
    client: &Client,
    launch: &LaunchPolicy,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
) -> Result<(App, SessionInfo)> {
    // The CLI's trailing prompt opens a conversation, not a typed question;
    // asking for a shape is a per-turn choice made in the interface.
    let info = create_session(client, launch, workspace_id, selection, seed, None)?;
    let mut app = App::new(info.selection.clone(), info.id.clone());
    app.launch.interaction = launch.clone();
    app.session_name = info.name.clone();
    app.workspace.id = Some(info.workspace_id.clone());
    app.workspace.enter(info.workspace.clone());
    app.launch.record(info.driva.clone());
    app.push_log(LogEntry::info(format!(
        "journal: {}",
        info.journal_path.display()
    )));
    for message in &info.queued {
        app.outbox.queue(message.clone());
    }
    Ok((app, info))
}

/// Attach to a live interaction: rebuild an `App` from its summary and replay the
/// updates the server has accumulated for it, so the view matches what the interaction
/// has done so far and the event loop can continue polling from the cursor.
pub fn attach_live_interaction(
    client: &Client,
    interaction: InteractionSummary,
) -> Result<(App, Live)> {
    let snapshot = client.interaction_snapshot(&interaction.id, InteractionSnapshotScope::Full)?;
    Ok(app_from_interaction_snapshot(snapshot))
}

/// Turn a server event into the ordinary main interaction model. Keeping this
/// independent of the navigator lets preview and full payloads populate the
/// same view through the event loop's single incoming-event path.
pub fn app_from_interaction_snapshot(snapshot: InteractionSnapshot) -> (App, Live) {
    let InteractionSnapshot {
        request_id: _,
        interaction,
        background_work,
        updates,
        queued,
        scope,
    } = snapshot;
    let mut app = App::new(interaction.selection.clone(), interaction.id.clone());
    app.raw
        .set_loaded(matches!(scope, InteractionSnapshotScope::Full));
    app.session_name = interaction.name.clone();
    app.workspace.id = Some(interaction.workspace_id.clone());
    app.workspace.enter(interaction.workspace.clone());
    app.launch.record(interaction.driva.clone());
    let cursor = updates.next;
    for sequenced in updates.updates {
        apply_update(&mut app, sequenced.update);
    }
    app.select_last();
    for message in queued {
        app.outbox.queue(message);
    }
    let accepting = interaction.accepting;
    let activity = interaction.activity;
    let live = attached_live(interaction.id, cursor, accepting);
    if accepting {
        // Preview snapshots contain only conversation rows, not the
        // TurnCompleted/background lifecycle events needed to reconstruct the
        // status. The summary was captured by the server with the snapshot and
        // is authoritative for both preview and full-load races.
        app.activity.sync_to_interaction(activity, background_work);
    } else if app.activity.status.is_active() {
        // Stopped interactions remain in the server's interaction list until
        // another interaction replaces them. Treat that stale record like a
        // stored journal, otherwise input can be queued against a process that
        // cannot receive it instead of taking the native-resume path.
        app.activity.status = Status::Stopped;
    }
    (app, live)
}

/// Replay a stored journal into a fresh `App`, with no live agent attached.
/// This is what `--view` opens directly, and what [`open_session`] falls back
/// to once it finds no interaction serving the Session.
pub fn open_stored(client: &Client, session_id: &str) -> Result<(App, Live)> {
    let stored = client.stored_session(session_id)?;
    let mut app = App::new(stored.summary.selection, stored.summary.id);
    app.session_name = stored.summary.name;
    app.workspace.id = Some(stored.summary.workspace_id);
    // `stored.events[i]` and `stored.raw[i]` are decoded from the same journal
    // record (see `journal::replay`/`replay_raw`), so pushing them in lockstep
    // — raw line first, as a live session receives it — gives each kept entry
    // a `raw_index` that actually points at its own wire line instead of
    // leaving it unset.
    for (event, line) in stored.events.into_iter().zip(stored.raw) {
        app.raw.push(line);
        // Skip carried-but-viewless traffic (e.g. app-server control lines),
        // matching what a live session shows; it stays available in the raw
        // view above.
        if !matches!(event, styra_server::event::AgentEvent::Unknown { .. }) {
            app.push_event(event);
        }
    }
    // A replayed session has no live agent to end; mark it stopped.
    app.on_ended(styra_server::InteractionEnd {
        exit_code: None,
        error: None,
    });
    Ok((app, Live::Viewing))
}

pub fn open_session(client: &Client, session_id: &str) -> Result<(App, Live)> {
    if let Some(interaction) = client
        .list_interactions()?
        .into_iter()
        .find(|interaction| interaction.id == session_id)
    {
        return attach_live_interaction(client, interaction);
    }
    open_stored(client, session_id)
}

pub fn session_id_from_target(target: &Path) -> Result<String> {
    target
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .with_context(|| format!("invalid session target {}", target.display()))
}

/// Resume `app.session_id` through its provider's native mechanism, then
/// deliver `message` to the freshly revived agent, under `contract` when the
/// operator asked this turn for a shape.
pub fn resume_and_send(
    app: &mut App,
    client: &Client,
    live: &mut Live,
    message: String,
    contract: Option<Contract>,
) {
    if app.session_id.is_empty() {
        app.push_log(LogEntry::warn("not sent: no Session to resume"));
        return;
    }
    match client.resume_session(&ResumeSession {
        id: app.session_id.clone(),
        launch: app.launch.interaction.clone(),
    }) {
        Ok(info) => {
            app.session_name = info.name.clone();
            app.workspace.enter(info.workspace);
            app.launch.record(info.driva);
            app.push_log(LogEntry::info("resumed with provider-native context"));
            for message in &info.queued {
                app.outbox.queue(message.clone());
            }
            let session_id = info.id;
            app.session_id = session_id.clone();
            app.activity.status = Status::Running;
            *live = Live::Running {
                session_id: session_id.clone(),
                cursor: info.updates_after,
            };
            if let Err(error) =
                client.send_turn(&session_id, turn(&message, &app.selection, contract))
            {
                app.push_log(LogEntry::error(format!("send failed: {error:#}")));
            }
        }
        Err(error) => {
            app.push_log(LogEntry::error(format!(
                "could not resume Session {}: {error:#}",
                app.session_id
            )));
            app.set_input(message);
        }
    }
}

pub fn pause_interaction(app: &mut App, client: &Client, live: &mut Live) {
    if let Live::Running { session_id, .. } = live {
        if let Err(error) = client.stop_interaction(session_id) {
            app.push_log(LogEntry::error(format!("pause failed: {error:#}")));
        } else {
            if let Err(error) = client.clear_queued_messages(session_id) {
                app.push_log(LogEntry::error(format!(
                    "could not clear the durable message queue: {error:#}"
                )));
            }
            let cleared = app.outbox.clear_queued();
            app.push_log(LogEntry::info(if cleared == 0 {
                "interaction paused; send a new message to start again".into()
            } else {
                format!(
                    "interaction paused; cleared {cleared} queued message(s); send a new message to start again"
                )
            }));
            mark_stopped(app, live);
        }
    } else {
        app.enter_list();
    }
}

/// Branch the current Session into a new sibling, seeded with its history up
/// to the selected entry (or the whole history, while the list is following
/// the newest entry or the selection has no known wire line). The result is
/// opened immediately, so the operator sees what they branched rather than
/// having to find it again in the picker.
pub fn branch_session(app: &mut App, client: &Client) {
    let at_ms = (!app.timeline.follow)
        .then(|| {
            app.timeline
                .entries
                .get(app.timeline.selected)
                .and_then(|entry| entry.raw_index)
        })
        .flatten()
        .and_then(|index| app.raw.get(index))
        .map(|line| line.at_ms);
    match client.branch_session(&app.session_id, at_ms, None) {
        Ok(branched) => {
            app.push_log(LogEntry::info(format!(
                "branched to session {}",
                branched.name.as_deref().unwrap_or(&branched.id)
            )));
            app.ask(crate::app::Request::OpenSession(branched.id));
        }
        Err(error) => app.push_log(LogEntry::error(format!("branch failed: {error:#}"))),
    }
}

pub fn interrupt_interaction(app: &mut App, client: &Client, live: &Live) {
    let Live::Running { session_id, .. } = live else {
        return app.push_log(LogEntry::warn("no live interaction to interrupt"));
    };
    match client.interrupt_interaction(session_id) {
        Ok(()) => app.push_log(LogEntry::info("interrupt requested")),
        Err(error) => app.push_log(LogEntry::error(format!("interrupt failed: {error:#}"))),
    }
}

/// Apply one session update to the app. Shared by the live event loop and by
/// [`attach_live_interaction`], which replays an interaction's accumulated updates the same way.
pub fn apply_update(app: &mut App, update: InteractionUpdate) {
    match update {
        InteractionUpdate::Event(event) => app.push_event(event),
        InteractionUpdate::Raw(line) => app.raw.push(line),
        InteractionUpdate::Log(entry) => app.push_log(entry),
        InteractionUpdate::Quota(reading) => app.note_quota(reading),
        InteractionUpdate::WorkingDirectoryChanged(directory) => {
            app.workspace.change_directory(directory);
        }
        InteractionUpdate::Ended(end) => app.on_ended(end),
    }
}

fn mark_stopped(app: &mut App, live: &mut Live) {
    app.activity.status = Status::Stopped;
    // This App still represents the same durable Session. `Pending` is
    // reserved for a blank screen with no Session id; marking a stopped
    // Session pending makes the next message create an unrelated session and
    // therefore lose the conversation context.
    *live = Live::Viewing;
}

fn attached_live(session_id: String, cursor: u64, accepting: bool) -> Live {
    if accepting {
        Live::Running { session_id, cursor }
    } else {
        Live::Viewing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use styra_server::agent::{Provider, Selection};
    use styra_server::event::AgentEvent;
    use styra_server::protocol::{SequencedUpdate, Updates};
    use styra_server::{DrivaOptions, InteractionActivity};

    fn workspace(id: &str, host_path: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: None,
            host_path: host_path.into(),
            git_repository: None,
            worktrees_enabled: false,
            path: format!("/state/workspaces/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
            launch: Default::default(),
        }
    }

    fn app() -> App {
        App::new(
            Selection {
                provider: Provider::Codex,
                model: Provider::Codex.default_model().to_owned(),
                effort: Provider::Codex.default_effort(),
            },
            "session-1",
        )
    }

    fn interaction_snapshot(
        activity: InteractionActivity,
        accepting: bool,
        updates: Vec<InteractionUpdate>,
    ) -> InteractionSnapshot {
        InteractionSnapshot {
            request_id: "preview-1".into(),
            interaction: InteractionSummary {
                id: "session-1".into(),
                name: None,
                workspace_id: "workspace-1".into(),
                selection: Selection::parse("codex").unwrap(),
                workspace: PathBuf::from("/workspace"),
                driva: DrivaOptions {
                    isolation_backend: "none".into(),
                    command: vec![],
                    working_directory: PathBuf::from("/workspace"),
                    network: false,
                    mounts: vec![],
                },
                accepting,
                activity,
                last_message: None,
            },
            background_work: activity == InteractionActivity::Background,
            updates: Updates {
                next: updates.len() as u64,
                updates: updates
                    .into_iter()
                    .enumerate()
                    .map(|(index, update)| SequencedUpdate {
                        sequence: index as u64 + 1,
                        update,
                    })
                    .collect(),
            },
            queued: vec![],
            scope: InteractionSnapshotScope::Preview { limit: 5 },
        }
    }

    fn trailing_agent_message() -> Vec<InteractionUpdate> {
        vec![InteractionUpdate::Event(AgentEvent::AgentMessage {
            text: "finished".into(),
        })]
    }

    #[test]
    fn idle_preview_uses_the_snapshot_activity_after_conversation_replay() {
        let (app, live) = app_from_interaction_snapshot(interaction_snapshot(
            InteractionActivity::Pending,
            true,
            trailing_agent_message(),
        ));

        assert_eq!(app.activity.status, Status::Idle);
        assert_eq!(
            live,
            Live::Running {
                session_id: "session-1".into(),
                cursor: 1,
            }
        );
    }

    #[test]
    fn background_preview_keeps_background_state_after_conversation_replay() {
        let (mut app, _) = app_from_interaction_snapshot(interaction_snapshot(
            InteractionActivity::Background,
            true,
            trailing_agent_message(),
        ));

        assert_eq!(app.activity.status, Status::Background);
        // The synchronized backing flag must also keep a later completion in
        // Background rather than incorrectly changing it to Idle.
        app.push_event(AgentEvent::TurnCompleted {
            usage: Default::default(),
        });
        assert_eq!(app.activity.status, Status::Background);
    }

    #[test]
    fn running_preview_remembers_coexisting_background_work() {
        let mut snapshot =
            interaction_snapshot(InteractionActivity::Running, true, trailing_agent_message());
        snapshot.background_work = true;
        let (mut app, _) = app_from_interaction_snapshot(snapshot);

        assert_eq!(app.activity.status, Status::Running);
        app.push_event(AgentEvent::TurnCompleted {
            usage: Default::default(),
        });
        assert_eq!(app.activity.status, Status::Background);
    }

    #[test]
    fn stopped_preview_overrides_every_active_replayed_status() {
        let updates = vec![
            InteractionUpdate::Event(AgentEvent::BackgroundTasks { running: 1 }),
            InteractionUpdate::Event(AgentEvent::TurnCompleted {
                usage: Default::default(),
            }),
        ];
        let (app, live) = app_from_interaction_snapshot(interaction_snapshot(
            InteractionActivity::Background,
            false,
            updates,
        ));

        assert_eq!(app.activity.status, Status::Stopped);
        assert_eq!(live, Live::Viewing);
    }

    #[test]
    fn stopped_session_is_viewed_until_its_next_message_resumes_it() {
        let mut app = app();
        let mut live = Live::Running {
            session_id: "session-1".into(),
            cursor: 7,
        };

        mark_stopped(&mut app, &mut live);

        assert_eq!(app.session_id, "session-1");
        assert_eq!(app.activity.status, Status::Stopped);
        assert_eq!(live, Live::Viewing);
    }

    #[test]
    fn stopped_server_interaction_is_opened_as_resumable_history() {
        assert_eq!(attached_live("session-1".into(), 7, false), Live::Viewing);
        assert_eq!(
            attached_live("session-1".into(), 7, true),
            Live::Running {
                session_id: "session-1".into(),
                cursor: 7,
            }
        );
    }

    #[test]
    fn finds_the_workspace_associated_with_the_host_directory() {
        let workspaces = vec![
            workspace("w-other", "/home/op/other"),
            workspace("w-project", "/home/op/project"),
        ];

        let found = find_workspace_for_host(&workspaces, Path::new("/home/op/project")).unwrap();

        assert_eq!(found.id, "w-project");
    }

    #[test]
    fn does_not_associate_a_different_directory() {
        let workspaces = vec![workspace("w-project", "/home/op/project")];

        assert!(find_workspace_for_host(&workspaces, Path::new("/home/op/elsewhere")).is_none());
    }
}

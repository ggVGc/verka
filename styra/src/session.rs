use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::app::{App, Status};
use crate::cli::Cli;
use styra_server::agent::Selection;
use styra_server::protocol::{CreateSession, CreateWorkspace, ResumeSession, SessionInfo};
use styra_server::{
    Client, InteractionSummary, InteractionUpdate, LogEntry, SessionSummary, WorkspaceSummary,
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
    client.create_workspace(&CreateWorkspace {
        host_path: canonical,
        name: None,
    })
}

/// What to call a Workspace on screen: its given name, or the last component
/// of the host directory it stands for when it has none.
pub fn workspace_display_name(workspace: &WorkspaceSummary) -> String {
    workspace.name.clone().unwrap_or_else(|| {
        workspace
            .host_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned()
    })
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

pub fn create_session(
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
) -> Result<SessionInfo> {
    client.create_session(&CreateSession {
        workspace_id: workspace_id.to_owned(),
        selection: selection.clone(),
        network: cli.network,
        templates: cli.template.clone(),
        message: seed.map(str::to_owned),
        name: None,
    })
}

/// Spawn a session and wrap it in a fresh `App`. Used for the CLI's trailing
/// prompt on first launch, the only case where the agent starts before the
/// event loop takes over.
pub fn launch_live_session(
    client: &Client,
    cli: &Cli,
    workspace_id: &str,
    selection: &Selection,
    seed: Option<&str>,
) -> Result<(App, SessionInfo)> {
    let info = create_session(client, cli, workspace_id, selection, seed)?;
    let mut app = App::new(info.selection.clone(), info.id.clone());
    app.session_name = info.name.clone();
    app.workspace_id = Some(info.workspace_id.clone());
    app.set_workspace_root(info.workspace.clone());
    app.set_driva_options(info.driva.clone());
    app.push_log(LogEntry::info(format!(
        "journal: {}",
        info.journal_path.display()
    )));
    for message in &info.queued {
        app.queue_message(message.clone());
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
    let mut app = App::new(interaction.selection.clone(), interaction.id.clone());
    app.session_name = interaction.name.clone();
    app.workspace_id = Some(interaction.workspace_id.clone());
    app.set_workspace_root(interaction.workspace.clone());
    app.set_driva_options(interaction.driva.clone());
    let batch = client.updates(&interaction.id, 0)?;
    let cursor = batch.next;
    for sequenced in batch.updates {
        apply_update(&mut app, sequenced.update);
    }
    for message in client.queued_messages(&interaction.id)? {
        app.queue_message(message);
    }
    let accepting = interaction.accepting;
    let live = attached_live(interaction.id, cursor, accepting);
    if !accepting {
        // Stopped interactions remain in the server's interaction list until
        // another interaction replaces them. Treat that stale record like a
        // stored journal, otherwise input can be queued against a process that
        // cannot receive it instead of taking the native-resume path.
        if matches!(app.status, Status::Running | Status::Idle) {
            app.status = Status::Stopped;
        }
    }
    Ok((app, live))
}

/// Replay a stored journal into a fresh `App`, with no live agent attached.
/// This is what `--view` opens directly, and what [`open_session`] falls back
/// to once it finds no interaction serving the Session.
pub fn open_stored(client: &Client, session_id: &str) -> Result<(App, Live)> {
    let stored = client.stored_session(session_id)?;
    let mut app = App::new(stored.summary.selection, stored.summary.id);
    app.session_name = stored.summary.name;
    app.workspace_id = Some(stored.summary.workspace_id);
    // `stored.events[i]` and `stored.raw[i]` are decoded from the same journal
    // record (see `journal::replay`/`replay_raw`), so pushing them in lockstep
    // — raw line first, as a live session receives it — gives each kept entry
    // a `raw_index` that actually points at its own wire line instead of
    // leaving it unset.
    for (event, line) in stored.events.into_iter().zip(stored.raw) {
        app.push_raw(line);
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
/// deliver `message` to the freshly revived agent.
pub fn resume_and_send(
    app: &mut App,
    client: &Client,
    cli: &Cli,
    live: &mut Live,
    message: String,
) {
    if app.session_id.is_empty() {
        app.push_log(LogEntry::warn("not sent: no Session to resume"));
        return;
    }
    match client.resume_session(&ResumeSession {
        id: app.session_id.clone(),
        network: cli.network,
        templates: cli.template.clone(),
    }) {
        Ok(info) => {
            app.session_name = info.name.clone();
            app.set_workspace_root(info.workspace);
            app.set_driva_options(info.driva);
            app.push_log(LogEntry::info("resumed with provider-native context"));
            for message in &info.queued {
                app.queue_message(message.clone());
            }
            let session_id = info.id;
            app.session_id = session_id.clone();
            app.status = Status::Running;
            *live = Live::Running {
                session_id: session_id.clone(),
                cursor: info.updates_after,
            };
            if let Err(error) = client.send_message(&session_id, &message) {
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
            let cleared = app.clear_queued_messages();
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
        InteractionUpdate::Raw(line) => app.push_raw(line),
        InteractionUpdate::Log(entry) => app.push_log(entry),
        InteractionUpdate::Ended(end) => app.on_ended(end),
    }
}

fn mark_stopped(app: &mut App, live: &mut Live) {
    app.status = Status::Stopped;
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
    use styra_server::agent::{Provider, Selection};

    fn workspace(id: &str, host_path: &str) -> WorkspaceSummary {
        WorkspaceSummary {
            id: id.into(),
            name: None,
            host_path: host_path.into(),
            path: format!("/state/workspaces/{id}").into(),
            session_count: 0,
            age: "now".into(),
            created_at_ms: 1,
            last_accessed_at_ms: 1,
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

    #[test]
    fn stopped_session_is_viewed_until_its_next_message_resumes_it() {
        let mut app = app();
        let mut live = Live::Running {
            session_id: "session-1".into(),
            cursor: 7,
        };

        mark_stopped(&mut app, &mut live);

        assert_eq!(app.session_id, "session-1");
        assert_eq!(app.status, Status::Stopped);
        assert_eq!(live, Live::Viewing);
    }

    #[test]
    fn stopped_server_interaction_is_opened_as_resumable_history() {
        assert_eq!(
            attached_live("session-1".into(), 7, false),
            Live::Viewing
        );
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

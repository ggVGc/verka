//! Turning what arrives from the agent into what the list shows.
//!
//! One function's worth of policy, but a dense one: most events append a row,
//! several replace one that is already there (a command completing, a tool
//! finishing, thinking ticking over), and nearly all of them say something
//! about whether the session is still working. [`App`] carries the state; this
//! module is the only place that decides how an [`AgentEvent`] changes it.

use crate::app::{App, Status};
use crate::timeline::{Entry, Step};
use styra_server::contract;
use styra_server::event::{AgentEvent, TokenUsage};
use styra_server::Contract;
use styra_server::InteractionEnd;

/// Append a decoded event, advancing status and, while following, selection.
pub fn push_event(app: &mut App, event: AgentEvent) {
    // A typed turn was framed by the server before it was sent, so the
    // message that comes back carries the operator's text followed by the
    // contract's instructions. Show what they wrote and note what they asked
    // for; the framing is boilerplate the list repeating adds nothing, and
    // the raw view still holds the line exactly as it went out.
    let (event, contract) = unframed(event);
    // Set before the replacement paths below, all of which return early:
    // a command or tool finishing is activity like any other.
    app.note_event_received();
    // A command completion is the final state of the command-start row.
    // Replace the most recent matching start instead of adding a second
    // line, so the list shows one command whose indication changes from
    // running to its result.
    if let AgentEvent::CommandCompleted { command, .. } = &event {
        if let Some(entry) = app.timeline.entries.iter_mut().rev().find(|entry| {
            matches!(&entry.event, AgentEvent::CommandStarted { command: started } if started == command)
        }) {
            entry.event = event;
            follow_tail(app);
            return;
        }
    }
    // Same as above, for tool calls: a `ToolCompleted` is the final state
    // of its matching `ToolStarted` row, correlated by id rather than
    // name — Claude's `tool_result` only ever repeats the `tool_use_id`,
    // never the tool's name or arguments, so the completed event's own
    // `name`/`detail` are placeholders that get replaced with the started
    // row's real ones (e.g. `Bash` and its command), so the finished row
    // still shows what actually ran rather than just the bare tool name.
    if let AgentEvent::ToolCompleted {
        id, status, output, ..
    } = &event
    {
        if let Some(entry) = app.timeline.entries.iter_mut().rev().find(|entry| {
            matches!(&entry.event, AgentEvent::ToolStarted { id: started, .. } if started == id)
        }) {
            let finishes_background = matches!(
                &entry.event,
                AgentEvent::ToolStarted { name, .. }
                    if matches!(name.as_str(), "TaskOutput" | "TaskGet" | "task_output" | "task_get")
                        && event.finishes_background_task()
            );
            if let AgentEvent::ToolStarted { id, name, detail } = &entry.event {
                entry.event = AgentEvent::ToolCompleted {
                    id: id.clone(),
                    name: name.clone(),
                    detail: detail.clone(),
                    status: status.clone(),
                    output: output.clone(),
                };
            }
            if finishes_background {
                app.note_background_finished();
            }
            follow_tail(app);
            return;
        }
        // Claude's Edit/Write/MultiEdit tool calls surface as `FileChanged`
        // at start, not `ToolStarted` (see `claude_tool_started`), so their
        // matching `ToolCompleted` never finds a started row above and
        // would otherwise fall through to a new, id-only line. A clean
        // result just confirms what the `FileChanged` row already showed,
        // so it is dropped rather than appended a second time; a failed
        // one replaces the row with a visible error, since the diff shown
        // there may not have actually landed.
        if let Some(entry) = app.timeline.entries.iter_mut().rev().find(|entry| {
            matches!(&entry.event, AgentEvent::FileChanged { id: changed, .. } if changed == id)
        }) {
            if status == "error" {
                if let AgentEvent::FileChanged { paths, .. } = &entry.event {
                    entry.event = AgentEvent::Error {
                        message: format!("{}: {output}", paths.join(", ")),
                    };
                }
            }
            follow_tail(app);
            return;
        }
    }
    // Claude reports extended thinking as a stream of lines — prose blocks
    // and a running token count — many per turn. They all describe the
    // same ongoing reasoning, so a run of them refreshes one line in place
    // (keeping the last prose seen when only the count moved) instead of
    // filling the list with a line per tick.
    if event.updates_thinking() && refresh_thinking(app, &event) {
        return;
    }
    match &event {
        AgentEvent::TurnCompleted { usage } => {
            // The app-server protocol's `turn/completed` carries no usage
            // figures of its own (a default, empty one); keep whatever the
            // last `UsageUpdated` reported rather than blanking the display.
            if *usage != TokenUsage::default() {
                app.latest_usage = Some(usage.clone());
            }
            if app.status.is_active() {
                app.status = app.idle_or_background();
            }
        }
        AgentEvent::UsageUpdated { usage } => {
            app.latest_usage = Some(usage.clone());
        }
        // The agent naming its own model settles what is running, so it
        // replaces the launch request in the status line. An effort the
        // agent does not report leaves whatever was already known standing
        // (Claude Code names a model but never an effort, so the launch's
        // own `--effort` remains the only word on it).
        AgentEvent::ThreadStarted {
            model: Some(model),
            effort,
            ..
        } => {
            let known = effort
                .clone()
                .or_else(|| app.reported_model.take().and_then(|(_, effort)| effort));
            app.reported_model = Some((model.clone(), known));
        }
        AgentEvent::UserMessage { .. }
        | AgentEvent::TurnStarted
        | AgentEvent::CommandStarted { .. }
        | AgentEvent::ToolStarted { .. }
        | AgentEvent::AgentMessage { .. }
        | AgentEvent::Thinking { .. }
        | AgentEvent::PlanUpdated { .. } => {
            if app.status.is_active() {
                app.status = Status::Running;
            }
        }
        _ => {}
    }
    if let Some(running) = event.background_tasks_running() {
        app.note_background_count(running);
    } else if event.starts_background_task() {
        app.note_background_started();
    }
    let transfer_expansion = app.timeline.follow
        && app.timeline.event_is_visible(&event)
        && app
            .timeline
            .selected_entry()
            .is_some_and(|entry| entry.expanded)
        && app
            .timeline
            .seek_forward(app.timeline.selected + 1, Step::Line)
            .is_none();
    if transfer_expansion {
        app.timeline.entries[app.timeline.selected].expanded = false;
    }
    let raw_index = app.raw.len().checked_sub(1);
    app.timeline.entries.push(Entry {
        event,
        expanded: transfer_expansion,
        raw_index,
        contract,
    });
    // Follow the tail of what is actually rendered. Hidden minor events
    // must not move the selection (and therefore the list viewport).
    follow_visible_tail(app);
}

/// Strip the server's contract framing from an operator message, returning the
/// text as it was written and the shape it asked for. Anything else is passed
/// through untouched — only a message this server framed can be unframed.
fn unframed(event: AgentEvent) -> (AgentEvent, Option<Contract>) {
    let AgentEvent::UserMessage { text } = &event else {
        return (event, None);
    };
    match contract::unframe(text) {
        Some((written, contract)) => (
            AgentEvent::UserMessage {
                text: written.to_owned(),
            },
            Some(contract),
        ),
        None => (event, None),
    }
}

/// Fold a thinking update into the line already showing one, if that is what
/// the last row is. Whether it was is what comes back: if not, the event is an
/// ordinary appended row.
fn refresh_thinking(app: &mut App, event: &AgentEvent) -> bool {
    let raw_index = app.raw.len().checked_sub(1);
    let Some(entry) = app.timeline.entries.last_mut() else {
        return false;
    };
    if !entry.event.updates_thinking() {
        return false;
    }
    if let (
        AgentEvent::Thinking { text, tokens },
        AgentEvent::Thinking {
            text: shown,
            tokens: counted,
        },
    ) = (event, &mut entry.event)
    {
        if !text.is_empty() {
            *shown = text.clone();
        }
        if tokens.is_some() {
            *counted = *tokens;
        }
    }
    // The refreshed line stands for the newest wire message.
    entry.raw_index = raw_index;
    if app.status.is_active() {
        app.status = Status::Running;
    }
    follow_visible_tail(app);
    true
}

/// Move the selection to the last row, for the paths that replaced one: the
/// row they changed is the newest thing the agent has said.
fn follow_tail(app: &mut App) {
    if app.timeline.follow && !app.timeline.entries.is_empty() {
        app.select_tail();
    }
}

/// The same, but only when the last row is one the current filters actually
/// show — a hidden minor event must not move the list's viewport.
fn follow_visible_tail(app: &mut App) {
    if app.timeline.follow
        && !app.timeline.entries.is_empty()
        && app.timeline.is_visible(app.timeline.entries.len() - 1)
    {
        app.select_tail();
    }
}

/// Record that the session ended. This is terminal regardless of `Stopped`.
pub fn on_ended(app: &mut App, end: InteractionEnd) {
    app.status = Status::Ended {
        exit_code: end.exit_code,
        error: end.error,
    };
}

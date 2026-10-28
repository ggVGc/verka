//! Mapping from application interaction state to the UI navigator model.

use crate::activity::{IdleReason, Status};
use crate::app::App;
use std::borrow::Cow;
use styra_protocol::InteractionSummary;
use styra_ui::interactions::{
    BranchLink, InteractionNavigator, InteractionRow, InteractionState, InteractionStatus,
};

pub(crate) fn view(app: &App) -> InteractionNavigator<'_> {
    let ordered = app.interactions.display_rows(app.workspace.id.as_deref());
    let cursor = app.interactions.cursor(&app.session_id);
    let loading = app
        .interactions
        .pending(&app.session_id)
        .map(|item| item.id.as_str());
    let all_workspaces = !app.interactions.only_current_workspace;
    let scope = if all_workspaces {
        Cow::Borrowed("All")
    } else {
        Cow::Borrowed(
            app.workspace
                .name
                .as_deref()
                .or(app.workspace.id.as_deref())
                .unwrap_or("Current Workspace"),
        )
    };
    // Every completed row is stopped, so hiding stopped rows hides those too.
    let completion_filter = if app.interactions.hide_stopped {
        "stopped hidden"
    } else if app.interactions.show_completed {
        "completed shown"
    } else {
        "completed hidden"
    };
    let mut rows = Vec::new();
    let mut heading = None;
    let source_of_selected = ordered
        .iter()
        .find(|row| app.interactions.items[row.index].id == cursor)
        .and_then(|row| row.branched_from.as_deref());
    for row in &ordered {
        let index = row.index;
        let interaction = &app.interactions.items[index];
        let new_workspace = heading.as_deref() != Some(interaction.workspace_id.as_str());
        if all_workspaces && new_workspace {
            let name = app
                .interactions
                .workspaces
                .iter()
                .find(|workspace| workspace.id == interaction.workspace_id)
                .map(crate::workspace::display_name)
                .unwrap_or_else(|| interaction.workspace_id.clone());
            rows.push(InteractionRow::Workspace(Cow::Owned(name)));
        }
        heading = Some(interaction.workspace_id.clone());
        // The order has put a headed worktree's rows, and then the worktrees
        // integrated into it, right after its heading — see `DisplayRow`.
        if let Some(worktree) = &row.heading {
            let name = interaction
                .workspace
                .file_name()
                .unwrap_or(interaction.workspace.as_os_str())
                .to_string_lossy();
            rows.push(InteractionRow::Directory {
                name,
                level: row.level.saturating_sub(1),
                into: worktree.into.clone().map(Cow::Owned),
            });
        }
        let name_of = |id: &str| {
            app.interactions
                .name(id)
                .unwrap_or_else(|| styra_ui::picker::short_id(id))
                .to_owned()
        };
        let state = state(interaction);
        let name = app
            .interactions
            .name(&interaction.id)
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Borrowed(styra_ui::picker::short_id(&interaction.id)));
        rows.push(InteractionRow::Interaction {
            name,
            level: row.level,
            depth: row.depth,
            branched_from: row
                .branched_from
                .as_deref()
                .map(|source| Cow::Owned(name_of(source))),
            branches: ordered
                .iter()
                .filter(|other| other.branched_from.as_deref() == Some(interaction.id.as_str()))
                .count(),
            branch_link: if source_of_selected == Some(interaction.id.as_str()) {
                BranchLink::Source
            } else if row.branched_from.as_deref() == Some(cursor) {
                BranchLink::Branch
            } else {
                BranchLink::None
            },
            model: &interaction.selection.model,
            branch: interaction
                .checkout
                .as_ref()
                .map(|checkout| checkout.branch.as_deref().unwrap_or("detached head")),
            expected_branch: interaction.expected_branch.as_deref().filter(|expected| {
                interaction
                    .checkout
                    .as_ref()
                    .is_some_and(|checkout| checkout.branch.as_deref() != Some(*expected))
            }),
            status: state.status,
            selected: interaction.id == cursor,
            loading: loading == Some(interaction.id.as_str()),
            newly_idle: state.newly_idle,
            flashing: app.interactions.flashing(&interaction.id),
            stop_reason: state.stop_reason.map(Cow::Owned),
            rate_limited: state.rate_limited.map(Cow::Owned),
            uncommitted: interaction.uncommitted_changes,
            completion: state.completion,
            tags: &interaction.tags,
            last_message: interaction
                .last_message
                .as_deref()
                .filter(|_| app.interactions.show_responses),
        });
    }
    InteractionNavigator {
        scope,
        all_workspaces,
        completion_filter: Cow::Borrowed(completion_filter),
        tag_filter: app.interactions.tag_filter(),
        filter: app
            .interactions
            .filter()
            .or(app.interactions.typing_filter().then_some("")),
        typing_filter: app.interactions.typing_filter(),
        requested_offset: app.interactions.scroll_offset,
        rows,
    }
}

/// What `interaction` is doing, as its navigator row draws it — and anywhere
/// else it is mentioned, such as a branch marker leading to it.
pub(crate) fn state(interaction: &InteractionSummary) -> InteractionState {
    // The navigator's gutter has one cell per row, so the reasons the status
    // carries are dropped here rather than rendered: they belong to the status
    // line of the interaction the panes below are showing. The exception is a
    // stopped one — see `InteractionRow::stop_reason`.
    let reported = Status::reported(interaction);
    let stop_reason = match &reported {
        // A completed (or abandoned, or sealed) row already says so with its
        // own badge, and the stop reason next to it would only repeat the
        // badge.
        Status::Stopped(_) if interaction.completed.is_done() => None,
        Status::Stopped(why) => Some(why.label()),
        _ => None,
    };
    // A refused turn leaves the interaction idle, which on its own reads as
    // "your turn": the window that is holding it is named so the row cannot
    // be mistaken for one waiting on the operator.
    let rate_limited = match &reported {
        Status::Idle(IdleReason::RateLimited(limit)) => Some(limit.window.clone()),
        _ => None,
    };
    InteractionState {
        status: status(&reported, interaction),
        stop_reason,
        rate_limited,
        newly_idle: interaction.activity == styra_protocol::InteractionActivity::Pending
            && interaction.idle_unseen,
        completion: interaction.completed,
    }
}

/// The marker an interaction is drawn with, wherever it is listed.
pub(super) fn status(reported: &Status, interaction: &InteractionSummary) -> InteractionStatus {
    match reported {
        Status::Pending => InteractionStatus::Pending,
        // Each row's own event count, not the attached session's: the rows
        // that are working animate whether or not this client's session is.
        Status::Running => InteractionStatus::Running {
            events: interaction.events,
        },
        Status::Idle(IdleReason::Failed { .. }) => InteractionStatus::IdleFailed,
        Status::Idle(_) => InteractionStatus::Idle,
        Status::Background => InteractionStatus::Background,
        Status::Stopped(why) => InteractionStatus::Stopped(super::stop_tone(why)),
        Status::Ended { error: Some(_), .. } => InteractionStatus::Error,
        Status::Ended { .. } => InteractionStatus::Ended,
    }
}

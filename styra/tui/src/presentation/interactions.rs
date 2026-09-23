//! Mapping from application interaction state to the UI navigator model.

use crate::activity::{IdleReason, Status};
use crate::app::App;
use std::borrow::Cow;
use styra_protocol::CompletionState;
use styra_ui::interactions::{InteractionNavigator, InteractionRow, InteractionStatus};

pub(crate) fn view(app: &App) -> InteractionNavigator<'_> {
    let ordered = app
        .interactions
        .display_indices(app.workspace.id.as_deref());
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
    let completion_filter = if app.interactions.show_completed {
        "completed shown"
    } else {
        "completed hidden"
    };
    let mut rows = Vec::new();
    let mut heading = None;
    for index in ordered {
        let interaction = &app.interactions.items[index];
        if all_workspaces && heading.as_deref() != Some(interaction.workspace_id.as_str()) {
            let name = app
                .interactions
                .workspaces
                .iter()
                .find(|workspace| workspace.id == interaction.workspace_id)
                .map(crate::workspace::display_name)
                .unwrap_or_else(|| interaction.workspace_id.clone());
            rows.push(InteractionRow::Workspace(Cow::Owned(name)));
            heading = Some(interaction.workspace_id.clone());
        }
        // The navigator's gutter has one cell per row, so the reasons the
        // status carries are dropped here rather than rendered: they belong to
        // the status line of the interaction the panes below are showing. The
        // exception is a stopped one — see `InteractionRow::stop_reason`.
        let reported = Status::reported(interaction);
        let stop_reason = match &reported {
            // A completed (or sealed) row already says so with its own badge,
            // and "you completed it" next to it would only repeat the badge.
            Status::Stopped(_) if interaction.completed != CompletionState::Active => None,
            Status::Stopped(why) => Some(Cow::Owned(why.label())),
            _ => None,
        };
        // A refused turn leaves the interaction idle, which on its own reads
        // as "your turn": the window that is holding it is named so the row
        // cannot be mistaken for one waiting on the operator.
        let rate_limited = match &reported {
            Status::Idle(IdleReason::RateLimited(limit)) => Some(Cow::Owned(limit.window.clone())),
            _ => None,
        };
        let status = match reported {
            Status::Pending => InteractionStatus::Pending,
            // Each row's own event count, not the attached session's: the rows
            // that are working animate whether or not this client's session is.
            Status::Running => InteractionStatus::Running {
                events: interaction.events,
            },
            Status::Idle(_) => InteractionStatus::Idle,
            Status::Background => InteractionStatus::Background,
            Status::Stopped(_) => InteractionStatus::Stopped,
            Status::Ended { error: Some(_), .. } => InteractionStatus::Error,
            Status::Ended { .. } => InteractionStatus::Ended,
        };
        let name = interaction
            .name
            .as_deref()
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Borrowed(styra_ui::picker::short_id(&interaction.id)));
        rows.push(InteractionRow::Interaction {
            name,
            provider: interaction.selection.provider.as_str(),
            status,
            current: interaction.id == app.session_id,
            selected: interaction.id == cursor,
            loading: loading == Some(interaction.id.as_str()),
            newly_idle: interaction.activity == styra_protocol::InteractionActivity::Pending
                && interaction.idle_unseen,
            stop_reason,
            rate_limited,
            uncommitted: interaction.uncommitted_changes,
            completed: interaction.completed != CompletionState::Active,
            sealed: interaction.completed == CompletionState::Sealed,
            tags: &interaction.tags,
            last_message: interaction.last_message.as_deref(),
        });
    }
    InteractionNavigator {
        scope,
        all_workspaces,
        completion_filter: Cow::Borrowed(completion_filter),
        rows,
    }
}

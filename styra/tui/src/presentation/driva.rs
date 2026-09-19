//! Mapping from launch, workspace, and interaction state to the Driva model.

use crate::app::App;
use crate::launch::LaunchScope;

pub(crate) fn view(app: &App) -> styra_ui::driva::DrivaView<'_> {
    use styra_ui::driva::{
        DrivaActivity, DrivaLaunch, DrivaStatus, DrivaView, DrivaWorkspace,
        LaunchScope as UiLaunchScope,
    };
    let status = match app.activity.status {
        crate::activity::Status::Pending => DrivaStatus::Pending,
        crate::activity::Status::Running => DrivaStatus::Running,
        crate::activity::Status::Idle(_) => DrivaStatus::Idle,
        crate::activity::Status::Background => DrivaStatus::Background,
        crate::activity::Status::Stopped(_) => DrivaStatus::Stopped,
        crate::activity::Status::Ended { .. } => DrivaStatus::Ended,
    };
    let scope = match app.launch.scope {
        LaunchScope::Workspace => UiLaunchScope::Workspace,
        LaunchScope::Interaction => UiLaunchScope::Interaction,
    };
    let last_message = app.timeline.entries.iter().rev().find_map(|entry| {
        matches!(
            entry.event,
            styra_protocol::event::AgentEvent::AgentMessage { .. }
        )
        .then(|| entry.event.summary())
    });
    // Current-directory fallback is resolved before presentation so the UI
    // implementation never performs filesystem access.
    let workspace = DrivaWorkspace {
        id: app.workspace.id.clone(),
        name: app.workspace.name.clone(),
        given_name: app.workspace.given_name.clone(),
        worktrees_enabled: app.workspace.worktrees_enabled,
        git_repository: app.workspace.git_repository.clone(),
        host_path: app.workspace.host_path.clone(),
        server_path: app.workspace.server_path.clone(),
        session_count: app.workspace.session_count,
        age: app.workspace.age.clone(),
        created_at_ms: app.workspace.created_at_ms,
        last_accessed_at_ms: app.workspace.last_accessed_at_ms,
        root: app.workspace.root().map(ToOwned::to_owned),
        working_directory: app.workspace.working_directory_or_current(),
    };
    DrivaView {
        chrome: super::panel_chrome(app, Some("details")),
        editable: app.can_edit_launch(),
        launch: DrivaLaunch {
            workspace: &app.launch.workspace,
            interaction: &app.launch.interaction,
            scope,
            workspace_cursor: app.launch.cursor(LaunchScope::Workspace),
            interaction_cursor: app.launch.cursor(LaunchScope::Interaction),
            prompt: app.launch.prompt.as_deref(),
            driva: app.launch.driva.as_ref(),
            planned: app.launch.planned,
            requested_scroll: app.launch.scroll.offset,
        },
        workspace,
        activity: DrivaActivity { status },
        selection_name: app.selection.name(),
        session_id: &app.session_id,
        session_name: app.session_name.as_deref(),
        queued_count: app.outbox.queued_count(),
        last_message,
        workspace_launch_pending: app.workspace_launch_pending,
        git_repository_prompt: app.git_repository_prompt.as_deref(),
    }
}

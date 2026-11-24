//! Moving between interactions: which one is current, and the navigator,
//! grid and Workspace jump that choose it.

use anyhow::Result;

use crate::app::{App, Focus, LaunchPolicy, View};
use crate::session::{self, Attachment};
use styra_protocol::InteractionSummary;
use styra_server::Client;

/// Narrow an open navigator to the Workspace highlighted in the jump — none,
/// when the query leaves nothing to highlight — or lift that narrowing once
/// the jump has closed.
pub(super) fn narrow_navigator_to_jump(app: &mut App) {
    app.interactions.jump_workspaces = app
        .workspace_jump
        .as_ref()
        .filter(|_| app.interactions.open)
        .map(|jump| jump.selected().into_iter().map(str::to_owned).collect());
}

/// Open the live interactions with the cursor on the first of `target`'s, the
/// way `v` lands. Run where the Workspace is chosen rather than asked for: a
/// request is only taken once a key reaches the dispatch, so the cursor would
/// not settle and load until the operator pressed another.
pub(super) fn open_interactions_in_workspace(
    app: &mut App,
    client: &Client,
    target: &str,
) -> Result<()> {
    let workspaces = client.list_workspaces()?;
    app.set_view(View::Events);
    app.focus = Focus::List;
    app.interactions.show(workspaces);
    let session_id = app.session_id.clone();
    if !app
        .interactions
        .cursor_to_workspace(target, &session_id, app.workspace.id.as_deref())
    {
        app.show_warning("the list's filters hide that Workspace's interactions");
    }
    Ok(())
}

/// Over the grid, show the Workspace highlighted in the jump by moving the
/// grid's cursor onto its first tile. Only the grid's own cursor moves, so
/// nothing loads until the choice is made, and Esc has nothing to undo but
/// the cursor and scope the jump remembered.
pub(super) fn preview_workspace_jump(app: &mut App) {
    let Some(target) = app
        .workspace_jump
        .as_ref()
        .filter(|jump| jump.grid.is_some())
        .and_then(|jump| jump.selected())
        .map(str::to_owned)
    else {
        return;
    };
    select_workspace_in_grid(app, &target);
}

/// Narrow the grid to `target` and move its cursor to `target`'s first tile.
/// The grid shows `target` in place of the Workspace on screen until a tile of
/// it has loaded.
pub(super) fn select_workspace_in_grid(app: &mut App, target: &str) -> bool {
    app.interactions.only_current_workspace = true;
    app.overview.workspace =
        (app.workspace.id.as_deref() != Some(target)).then(|| target.to_owned());
    app.overview.select_workspace(
        &app.interactions,
        target,
        &app.session_id,
        app.workspace.id.as_deref(),
    )
}

/// Load the interaction the list or grid cursor has moved onto, if it is not
/// the one already on screen. Called when the cursor settles, and ahead of any
/// key that acts on the row under it, so an outstanding move never leaves a
/// command addressing the interaction the operator has already moved off.
pub(super) fn load_cursored_interaction(
    app: &mut App,
    live: &mut Attachment,
    client: &Client,
    standing_launch: &LaunchPolicy,
) {
    let Some(interaction) = app.interactions.pending(&app.session_id).cloned() else {
        return;
    };
    let overview = (app.view == View::Overview).then(|| app.overview.clone());
    make_interaction_current(app, live, client, standing_launch, interaction);
    if let Some(overview) = overview {
        app.overview = overview;
        app.set_view(View::Overview);
        // The Workspace the jump chose is now the one on screen.
        if app.overview.workspace.as_deref() == app.workspace.id.as_deref() {
            app.overview.workspace = None;
        }
    }
}

pub(super) fn cursored_interaction_is_loadable(app: &App) -> bool {
    app.tag_picker.is_none()
        && app.diff_branch_picker.is_none()
        && app.branch_picker.is_none()
        && app.interactions.due(&app.session_id).is_some()
}

/// Make `interaction` current. Its complete state is loaded on the event-loop
/// thread, then replaces the previous interaction's state in one step. There
/// is no separate loader-owned target: the cursor is the only pending state,
/// and it rests here whether the load lands or fails.
pub(super) fn make_interaction_current(
    app: &mut App,
    live: &mut Attachment,
    client: &Client,
    standing_launch: &LaunchPolicy,
    interaction: InteractionSummary,
) {
    if interaction.id == app.session_id {
        return;
    }
    let id = interaction.id.clone();
    session::save_composer(app, client, true);
    match session::attach_live_interaction(client, &id, &app.quota_noticed) {
        Ok(opened) => {
            session::switch_to(app, live, opened, standing_launch);
            // The Workspace list is only filled when the navigator is opened,
            // so a switch made with it closed — `n` — would otherwise land on
            // a screen that cannot name the Workspace it is in, and the title
            // would lose its opening name. Ask the server for the list it is
            // missing, and keep the answer for the switches after this one.
            if !app
                .interactions
                .workspaces
                .iter()
                .any(|workspace| Some(workspace.id.as_str()) == app.workspace.id.as_deref())
            {
                if let Ok(workspaces) = client.list_workspaces() {
                    app.interactions.workspaces = workspaces;
                }
            }
            if let Some(workspace) = app
                .interactions
                .workspaces
                .iter()
                .find(|workspace| Some(workspace.id.as_str()) == app.workspace.id.as_deref())
                .cloned()
            {
                app.show_workspace(&workspace);
            }
        }
        Err(error) => {
            // The cursor comes home on a failed load too, so a server that
            // cannot answer for one interaction is reported once rather than
            // re-asked on every frame.
            app.interactions.rest();
            app.report_error(format!(
                "could not make interaction {id} current: {error:#}"
            ));
        }
    }
}

/// Open the live Interactions navigator over the main Interaction view, as `a`
/// does.
///
/// Best-effort: nothing live to list leaves the navigator closed rather than
/// failing the transition that asked for it. The root loop uses this when
/// entering a Workspace it already knows holds live work, with the listing it
/// found that out from.
pub fn open_interaction_navigator(
    app: &mut App,
    client: &Client,
    interactions: Vec<InteractionSummary>,
) {
    if interactions.is_empty() {
        return;
    }
    let workspaces = client.list_workspaces().unwrap_or_default();
    app.set_view(View::Events);
    app.focus = Focus::List;
    app.interactions.open(interactions, workspaces);
}

/// Make sure the App holds an interactions listing, asking the server only
/// when it has none yet — the startup listing and the loop's poll both having
/// failed. Every request that reads the fleet goes through this rather than
/// asking for itself: the poll keeps the listing at most a refresh old, and
/// the server takes a long time to answer it.
pub(super) fn ensure_listed(app: &mut App, client: &Client) -> Result<()> {
    if !app.interactions.listed() {
        let query = app.interactions_query();
        app.interactions.refresh(client.list_interactions(&query)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

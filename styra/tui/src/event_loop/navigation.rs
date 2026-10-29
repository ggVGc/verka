//! Moving between interactions: which one is current, and the navigator,
//! grid and Workspace jump that choose it.

use anyhow::Result;

use crate::app::{App, Focus, LaunchPolicy, View};
use crate::session::{self, Attachment};
use crate::workspace_jump::WorkspaceJump;
use styra_protocol::{InteractionSummary, LogEntry};
use styra_server::Client;

/// Narrow an open navigator to the Workspaces the jump's query leaves, or
/// lift that narrowing once the jump has closed.
pub(super) fn narrow_navigator_to_jump(app: &mut App) {
    app.interactions.jump_workspaces = app
        .workspace_jump
        .as_ref()
        .filter(|_| app.interactions.open)
        .map(WorkspaceJump::matching);
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
    app.overview
        .select_workspace(&app.interactions, target, app.workspace.id.as_deref())
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
            app.push_log(LogEntry::error(format!(
                "could not make interaction {id} current: {error:#}"
            )));
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
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use styra_protocol::{InteractionActivity, WorkspaceSummary};

    fn workspaces(ids: &[&str]) -> Vec<WorkspaceSummary> {
        ids.iter()
            .map(|id| WorkspaceSummary {
                id: (*id).into(),
                name: Some((*id).into()),
                host_path: (*id).into(),
                git_repository: None,
                path: (*id).into(),
                session_count: 0,
                age: String::new(),
                created_at_ms: 0,
                last_accessed_at_ms: 0,
                launch: Default::default(),
            })
            .collect()
    }

    /// Opening the jump over the navigator keeps it open, listing only the
    /// Workspaces the jump's query leaves; closing the jump lists them all.
    #[test]
    fn the_workspace_jump_narrows_the_open_navigator() {
        let press = |character| crossterm::event::KeyEvent::from(KeyCode::Char(character));
        assert!(super::super::keys::interaction_navigator_passthrough(
            press('v')
        ));
        assert!(super::super::keys::interaction_navigator_passthrough(
            press('V')
        ));

        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        let mut other =
            crate::interactions::tests::interaction("two", InteractionActivity::Running);
        other.workspace_id = "other".into();
        let interactions = vec![
            crate::interactions::tests::interaction("one", InteractionActivity::Running),
            other,
        ];
        let workspaces = workspaces(&["workspace", "other"]);
        let jump = WorkspaceJump::new(&workspaces, &interactions);
        app.interactions.open(interactions, workspaces);
        app.workspace_jump = Some(jump);
        narrow_navigator_to_jump(&mut app);
        assert_eq!(app.interactions.visible_indices("", None), vec![0, 1]);

        for character in "oth".chars() {
            app.workspace_jump
                .as_mut()
                .unwrap()
                .handle_key(press(character));
        }
        narrow_navigator_to_jump(&mut app);
        assert_eq!(app.interactions.visible_indices("", None), vec![1]);

        app.workspace_jump = None;
        narrow_navigator_to_jump(&mut app);
        assert!(app.interactions.open);
        assert_eq!(app.interactions.visible_indices("", None), vec![0, 1]);
    }

    /// Over the grid, the jump narrows the grid to its highlighted Workspace
    /// and moves only the grid's cursor; nothing is scheduled to load.
    #[test]
    fn the_workspace_jump_previews_its_highlight_on_the_grid() {
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        app.workspace.id = Some("workspace".into());
        let mut other =
            crate::interactions::tests::interaction("two", InteractionActivity::Running);
        other.workspace_id = "other".into();
        let interactions = vec![
            crate::interactions::tests::interaction("one", InteractionActivity::Running),
            other,
        ];
        let workspaces = workspaces(&["workspace", "other"]);
        let mut jump = WorkspaceJump::new(&workspaces, &interactions);
        app.interactions.open(interactions, workspaces);
        app.view = View::Overview;
        jump.grid = Some((app.overview.clone(), false));
        app.workspace_jump = Some(jump);

        app.workspace_jump
            .as_mut()
            .unwrap()
            .handle_key(crossterm::event::KeyEvent::from(KeyCode::Down));
        preview_workspace_jump(&mut app);

        assert!(app.interactions.only_current_workspace);
        assert_eq!(
            app.overview
                .indices(&app.interactions, Some("workspace"))
                .len(),
            1,
            "only the highlighted Workspace's tiles are shown"
        );
        assert_eq!(
            app.overview
                .selected_id(&app.interactions, "one", Some("workspace")),
            Some("two")
        );
        assert!(app.interactions.pending("one").is_none());
    }

    #[test]
    fn a_failed_grid_selection_load_rests_the_shared_cursor_and_keeps_the_grid() {
        let root = tempfile::tempdir().unwrap();
        let client = Client::new(root.path().join("missing.sock"));
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
            "one",
        );
        app.interactions.refresh(
            ["one", "two"]
                .into_iter()
                .map(|id| crate::interactions::tests::interaction(id, InteractionActivity::Running))
                .collect(),
        );
        app.view = View::Overview;
        app.overview.running_only = true;
        app.overview.note_rows(&[2]);
        app.overview.right(&app.interactions, "one", None);
        app.interactions.move_cursor_to("two".into(), "one");
        let mut live = Attachment::Detached;

        load_cursored_interaction(&mut app, &mut live, &client, &LaunchPolicy::default());

        assert_eq!(app.session_id, "one");
        assert_eq!(app.view, View::Overview);
        assert!(app.overview.running_only);
        assert_eq!(
            app.overview.selected_id(&app.interactions, "one", None),
            Some("two")
        );
        assert!(app.interactions.pending("one").is_none());
        assert!(!cursored_interaction_is_loadable(&app));
    }
}

use super::*;
use crate::activity::{IdleReason, StopReason};
use std::path::PathBuf;
use styra_protocol::AttributedMount;

fn workspace_policy() -> LaunchPolicy {
    LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![LaunchMount {
            source: PathBuf::from("/srv/corpus"),
            destination: None,
            writable: false,
        }],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    }
}

fn pending() -> App {
    App::pending(Selection::parse("codex").unwrap())
}

/// An App with an interaction already running, where the policy is a record
/// rather than a choice.
fn running() -> App {
    App::new(Selection::parse("codex").unwrap(), "s1")
}

fn options(backend: &str) -> DrivaOptions {
    DrivaOptions {
        isolation_backend: backend.into(),
        command: vec!["codex".into()],
        working_directory: PathBuf::from("/tmp/styra/workspace"),
        network: false,
        base: Vec::new(),
        mounts: Vec::new(),
        ..Default::default()
    }
}

fn pending_in_a_workspace_with_a_policy() -> App {
    let mut app = pending();
    app.launch.set_workspace(workspace_policy());
    app
}

fn add_mount(app: &mut App, text: &str) {
    app.launch.prompt = Some(text.into());
    confirm_prompt(app);
}

/// The two layers are held apart, and an edit lands in one of them: what the
/// Workspace grants is added to, not replaced.
#[test]
fn an_edit_adds_to_the_workspace_policy_rather_than_replacing_it() {
    let mut app = pending_in_a_workspace_with_a_policy();
    assert_eq!(app.launch.effective(), workspace_policy());

    set_templates(&mut app, vec!["rust".into(), "browser".into()]);
    // Only the addition is this interaction's; the Workspace keeps its own.
    assert_eq!(app.launch.interaction.templates, vec!["browser"]);
    assert!(!app.launch.interaction.ignore_workspace);
    assert_eq!(
        app.launch.effective().templates,
        vec!["rust".to_owned(), "browser".to_owned()]
    );

    add_mount(&mut app, "/srv/scratch:rw");
    assert_eq!(app.launch.interaction.mounts.len(), 1);
    assert_eq!(app.launch.effective().mounts.len(), 2);
}

/// Dropping a template the Workspace grants cannot be said by adding to it,
/// so the interaction stops inheriting instead of silently doing nothing.
#[test]
fn deselecting_a_workspace_template_makes_the_launch_ignore_workspace() {
    let mut app = pending_in_a_workspace_with_a_policy();
    set_templates(&mut app, vec!["browser".into()]);

    assert!(app.launch.interaction.ignore_workspace);
    assert_eq!(app.launch.interaction.templates, vec!["browser"]);
    let effective = app.launch.effective();
    assert_eq!(effective.templates, vec!["browser"]);
    // Standalone is the whole policy, so the Workspace's mounts and its
    // network grant go with its templates.
    assert!(effective.mounts.is_empty());
    assert!(!effective.grants_network());
}

/// `w` states the opposite of what would otherwise be inherited, and then
/// returns to inheriting — there is no third state worth stopping on.
#[test]
fn network_toggles_against_the_workspace_policy_and_back_to_inheriting() {
    let mut app = pending_in_a_workspace_with_a_policy();
    assert!(app.launch.effective().grants_network());

    cycle_network(&mut app);
    assert_eq!(app.launch.interaction.network, Some(false));
    assert!(!app.launch.effective().grants_network());

    cycle_network(&mut app);
    assert_eq!(app.launch.interaction.network, None);
    assert!(app.launch.effective().grants_network());
}

/// The workspace mount is `rw` unless a layer says otherwise, and `R` is
/// the only way to say it: it is the one mount `m` and `x` cannot reach.
/// Like `w`, the first press changes the effective answer and the second
/// returns to inheriting.
#[test]
fn the_workspace_mount_is_made_read_only_against_what_would_be_inherited() {
    let mut app = pending();
    assert!(app.launch.effective().grants_writable_workspace(true));

    cycle_workspace_access(&mut app);
    assert_eq!(app.launch.interaction.writable_workspace, Some(false));
    assert!(!app.launch.effective().grants_writable_workspace(true));
    // Changing it changes what would be launched, so the plan is re-asked.
    assert!(wants_plan(&app));

    cycle_workspace_access(&mut app);
    assert_eq!(app.launch.interaction.writable_workspace, None);
    assert!(app.launch.effective().grants_writable_workspace(true));

    // Against a Workspace that has already withdrawn it, the first press is
    // the one that grants it back.
    app.launch.sync_workspace(LaunchPolicy {
        writable_workspace: Some(false),
        ..LaunchPolicy::default()
    });
    assert!(!app.launch.effective().grants_writable_workspace(true));
    cycle_workspace_access(&mut app);
    assert_eq!(app.launch.interaction.writable_workspace, Some(true));
    assert!(app.launch.effective().grants_writable_workspace(true));
}

/// In the repository's main worktree the server plans the unstated answer
/// as read-only, so there the first press is the one that grants write
/// access, on either layer.
#[test]
fn in_the_main_worktree_the_first_press_grants_write_access() {
    let mut app = pending();
    let effective = app.launch.effective();
    app.launch.plan(
        app.selection.clone(),
        effective,
        Some(DrivaOptions {
            workspace_read_only_by_default: true,
            ..options("planned")
        }),
    );
    assert!(!app.launch.writable_workspace_by_default());

    let message = app.launch.cycle_writable_workspace();
    assert_eq!(app.launch.interaction.writable_workspace, Some(true));
    assert!(message.contains("read-write"), "{message}");
    let message = app.launch.cycle_writable_workspace();
    assert_eq!(app.launch.interaction.writable_workspace, None);
    assert!(message.ends_with("(ro)"), "{message}");

    toggle_scope(&mut app);
    cycle_workspace_access(&mut app);
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::SetWritableWorkspace(Some(true)),
            clear_interaction: false,
        })
    );
}

/// The Workspace's own answer is the server's to keep, so `R` on that pane
/// sends the edit rather than mutating the client's snapshot.
#[test]
fn the_workspace_layers_mount_access_is_sent_to_the_server() {
    let mut app = pending_in_a_workspace_with_a_policy();
    toggle_scope(&mut app);

    cycle_workspace_access(&mut app);
    assert_eq!(app.launch.workspace.writable_workspace, None);
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::SetWritableWorkspace(Some(false)),
            clear_interaction: false,
        })
    );
    assert_eq!(app.launch.interaction.writable_workspace, None);
}

#[test]
fn moving_between_policy_layers_is_silent() {
    let mut app = pending_in_a_workspace_with_a_policy();

    toggle_scope(&mut app);
    assert_eq!(app.launch.scope, LaunchScope::Workspace);
    toggle_scope(&mut app);
    assert_eq!(app.launch.scope, LaunchScope::Interaction);
    assert!(app.notices.is_empty());
}

/// Commands are editable on a live interaction, since they change nothing
/// about its sandbox: this interaction's is kept by the client, the
/// Workspace's goes to the server, and a blank one inherits again.
#[test]
fn the_command_prompt_edits_like_every_other_field() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let press = |app: &mut App, code, modifiers| {
        crate::input::handle_command_prompt_key(app, KeyEvent::new(code, modifiers));
    };
    let mut app = running();
    open_command_prompt(&mut app, ProjectAction::Run);
    app.launch.command_prompt.as_mut().unwrap().1 = "cargo run\t--bin".into();

    // A chord is a command, not a letter typed into the command.
    press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('x'), KeyModifiers::ALT);
    // A shell word ends at whitespace, so `--bin` goes in one press.
    press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(app.launch.command_prompt.as_ref().unwrap().1, "cargo run\t");
    press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('b'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('B'), KeyModifiers::SHIFT);
    assert_eq!(app.launch.command_prompt.as_ref().unwrap().1, "cargo B");
}

#[test]
fn a_project_command_is_set_in_the_focused_layer_even_while_running() {
    let type_command = |app: &mut App, action, text: &str| {
        open_command_prompt(app, action);
        app.launch.command_prompt.as_mut().unwrap().1 = text.into();
        confirm_command_prompt(app);
        assert!(app.launch.command_prompt.is_none());
    };
    let mut app = running();

    type_command(&mut app, ProjectAction::Run, " cargo run --bin styra ");
    assert_eq!(
        app.launch.interaction.run.as_deref(),
        Some("cargo run --bin styra")
    );
    assert_eq!(app.take_request(), None);

    // Reopening starts from what the layer states.
    open_command_prompt(&mut app, ProjectAction::Run);
    assert_eq!(
        app.launch.command_prompt,
        Some((ProjectAction::Run, "cargo run --bin styra".into()))
    );
    cancel_command_prompt(&mut app);

    type_command(&mut app, ProjectAction::Run, "  ");
    assert_eq!(app.launch.interaction.run, None);

    toggle_scope(&mut app);
    type_command(&mut app, ProjectAction::Test, "cargo nextest run");
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::SetCommand {
                action: ProjectAction::Test,
                command: Some("cargo nextest run".into()),
            },
            clear_interaction: false,
        })
    );
    assert_eq!(app.launch.interaction.test, None);
}

/// A mount the Workspace already grants identically needs no overlay, and
/// one of its mounts is not this interaction's to remove.
#[test]
fn the_workspace_policys_mounts_are_not_this_interactions_to_add_or_remove() {
    let mut app = pending_in_a_workspace_with_a_policy();
    add_mount(&mut app, "/srv/corpus:ro");
    assert!(app.launch.interaction.mounts.is_empty());

    remove_selected_mount(&mut app);
    assert_eq!(app.launch.effective().mounts, workspace_policy().mounts);

    // Standalone is the way out, and it leaves the interaction with nothing
    // but its own inputs.
    toggle_ignore_workspace(&mut app);
    assert!(app.launch.effective().mounts.is_empty());
}

/// The Workspace's own layer is edited here too, by the same keys, once
/// `Tab` has moved them onto it — and, since the server owns that layer and
/// every launch path merges the stored copy, each edit is on its way there.
#[test]
fn the_workspace_layer_is_edited_by_the_same_keys_and_sent_to_the_server() {
    let mut app = pending_in_a_workspace_with_a_policy();
    let effective = app.launch.effective();
    app.launch.plan(app.selection.clone(), effective, None);
    assert!(!wants_plan(&app));

    toggle_scope(&mut app);
    assert_eq!(app.launch.scope, LaunchScope::Workspace);

    // The UI sends intent and does not optimistically mutate its server
    // snapshot.
    add_mount(&mut app, "/srv/models:ro");
    assert_eq!(app.launch.workspace.mounts.len(), 1);
    assert!(app.launch.interaction.mounts.is_empty());
    let request = app.take_request().unwrap();
    let Request::ChangeWorkspaceLaunch {
        change: WorkspaceLaunchChange::AddMounts(added),
        clear_interaction: false,
    } = request
    else {
        panic!("unexpected request: {request:?}");
    };
    assert!(!wants_plan(&app));
    let mut stored = app.launch.workspace.clone();
    stored.mounts.extend(added);
    app.launch.sync_workspace(stored);
    assert_eq!(app.launch.workspace.mounts.len(), 2);
    assert!(wants_plan(&app));

    // One the Workspace already grants changes nothing, and says so.
    add_mount(&mut app, "/srv/models:ro");
    assert_eq!(app.launch.workspace.mounts.len(), 2);

    // `x` reaches the Workspace's own mounts here, which is what the
    // interaction layer could never do.
    remove_selected_mount(&mut app);
    assert_eq!(app.launch.workspace.mounts.len(), 2);
    assert!(matches!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::RemoveMount(_),
            clear_interaction: false,
        })
    ));

    // Templates replace that layer's list rather than being turned into an
    // overlay: there is nothing under it for them to add to.
    set_templates(&mut app, vec!["browser".into()]);
    assert_eq!(app.launch.workspace.templates, vec!["rust"]);
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::SetTemplates(vec!["browser".into()]),
            clear_interaction: false,
        })
    );
    assert!(app.launch.interaction.templates.is_empty());

    // And network is the plain toggle it can be with nothing underneath.
    cycle_network(&mut app);
    assert_eq!(app.launch.workspace.network, Some(true));
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::SetNetwork(Some(false)),
            clear_interaction: false,
        })
    );
    assert_eq!(app.launch.interaction.network, None);
}

/// Whether this interaction inherits the Workspace's policy is only ever
/// this interaction's answer, so the key says so instead of quietly editing
/// the layer it cannot mean anything for.
#[test]
fn inheriting_stays_this_interactions_own_answer() {
    let mut app = pending_in_a_workspace_with_a_policy();
    toggle_scope(&mut app);
    toggle_ignore_workspace(&mut app);
    assert!(!app.launch.interaction.ignore_workspace);
    assert!(!app.launch.workspace.ignore_workspace);

    toggle_scope(&mut app);
    toggle_ignore_workspace(&mut app);
    assert!(app.launch.interaction.ignore_workspace);
}

/// The mount cursor is per layer: moving between the panes returns to where
/// each was left rather than pointing at whatever row shares its index.
#[test]
fn each_layer_keeps_its_own_mount_cursor() {
    let mut app = pending_in_a_workspace_with_a_policy();
    add_mount(&mut app, "/srv/one");
    add_mount(&mut app, "/srv/two");
    assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);
    // The Workspace's single mount, whatever this interaction's cursor is.
    assert_eq!(app.launch.cursor(LaunchScope::Workspace), 0);

    toggle_scope(&mut app);
    select_next_mount(&mut app);
    assert_eq!(app.launch.cursor(LaunchScope::Workspace), 0);
    assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);

    // `x` on the Workspace's pane leaves this interaction's mounts alone.
    remove_selected_mount(&mut app);
    assert_eq!(app.launch.workspace.mounts.len(), 1);
    assert!(matches!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::RemoveMount(_),
            clear_interaction: false,
        })
    ));
    assert_eq!(app.launch.interaction.mounts.len(), 2);
}

/// Storing the policy with the Workspace must not change what the next
/// launch runs under: this interaction's half is folded in, not layered
/// onto itself.
#[test]
fn promoting_a_policy_to_the_workspace_leaves_the_effective_one_unchanged() {
    let mut app = pending_in_a_workspace_with_a_policy();
    set_templates(&mut app, vec!["rust".into(), "browser".into()]);
    let before = app.launch.effective();

    app.launch.adopt_workspace(before.clone());
    assert!(app.launch.interaction.is_empty());
    assert_eq!(app.launch.effective(), before);
}

/// `U` is for a setting that turns out to belong to the work rather than to
/// this conversation, so with nothing added here there is nothing to move.
#[test]
fn moving_this_interactions_settings_up_needs_some_to_move() {
    let mut app = pending_in_a_workspace_with_a_policy();
    promote_to_workspace(&mut app);
    assert_eq!(app.take_request(), None);

    add_mount(&mut app, "/srv/scratch:rw");
    promote_to_workspace(&mut app);
    assert_eq!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::Replace(app.launch.effective()),
            clear_interaction: true,
        })
    );
}

#[test]
fn added_mounts_are_selected_and_removed_one_at_a_time() {
    let mut app = pending();
    for text in ["/srv/one", "/srv/two:rw", "/srv/three:/mnt/three:ro"] {
        add_mount(&mut app, text);
    }
    assert_eq!(app.launch.interaction.mounts.len(), 3);
    // Adding leaves the cursor on what was just added.
    assert_eq!(app.launch.cursor(LaunchScope::Interaction), 2);

    select_prev_mount(&mut app);
    remove_selected_mount(&mut app);
    assert_eq!(
        app.launch
            .interaction
            .mounts
            .iter()
            .map(|mount| mount.source.display().to_string())
            .collect::<Vec<_>>(),
        ["/srv/one", "/srv/three"]
    );
    // The cursor stays in range as the list shrinks under it.
    select_next_mount(&mut app);
    select_next_mount(&mut app);
    assert_eq!(app.launch.cursor(LaunchScope::Interaction), 1);
    remove_selected_mount(&mut app);
    remove_selected_mount(&mut app);
    assert!(app.launch.interaction.mounts.is_empty());
    assert_eq!(app.launch.cursor(LaunchScope::Interaction), 0);
    // Removing from an empty list says so rather than panicking.
    remove_selected_mount(&mut app);
}

/// A duplicate is refused rather than silently layered twice, since the
/// server would then reject the whole policy for a conflicting destination.
#[test]
fn the_same_mount_is_not_added_twice() {
    let mut app = pending();
    add_mount(&mut app, "/srv/data");
    add_mount(&mut app, "/srv/data:ro");
    assert_eq!(app.launch.interaction.mounts.len(), 1);
}

/// Text that is not a mount leaves the prompt open with what was typed, so a
/// typo is corrected rather than retyped.
#[test]
fn an_unparseable_mount_keeps_the_prompt_open() {
    let mut app = pending();
    add_mount(&mut app, "relative/path");
    assert_eq!(app.launch.prompt.as_deref(), Some("relative/path"));
    assert!(app.launch.interaction.mounts.is_empty());
}

#[test]
fn opening_a_mount_prompt_prefills_the_current_workspace_path() {
    let mut app = pending();
    app.workspace.enter(PathBuf::from("/home/op/project"));

    open_prompt(&mut app);

    assert_eq!(app.launch.prompt.as_deref(), Some("/home/op/project"));
}

/// Editing the launch inputs changes what would be launched, so the plan on
/// screen has to be re-asked for — otherwise the view would keep describing
/// a sandbox the operator has just changed.
#[test]
fn editing_the_launch_inputs_re_asks_for_the_plan() {
    let mut app = pending();
    let effective = app.launch.effective();
    app.launch
        .plan(app.selection.clone(), effective, Some(options("bwrap")));
    assert!(!wants_plan(&app));

    cycle_network(&mut app);
    assert_eq!(app.launch.interaction.network, Some(true));
    assert!(wants_plan(&app));

    let effective = app.launch.effective();
    app.launch.plan(app.selection.clone(), effective, None);
    assert!(!wants_plan(&app));

    set_templates(&mut app, vec!["rust".into()]);
    assert!(wants_plan(&app));
    let effective = app.launch.effective();
    app.launch.plan(app.selection.clone(), effective, None);

    add_mount(&mut app, "/srv/data:rw");
    assert!(wants_plan(&app));
    // Entering another Workspace changes the plan without an operator edit.
    {
        let mut app = pending();
        let effective = app.launch.effective();
        app.launch.plan(app.selection.clone(), effective, None);
        assert!(!wants_plan(&app));

        app.launch.set_workspace(workspace_policy());
        assert!(wants_plan(&app));
    }
}

/// The plan is asked for once per distinct set of inputs, and a live
/// interaction's own policy replaces it outright.
#[test]
fn a_planned_policy_is_asked_for_once_per_selection_and_yields_to_a_live_one() {
    let mut app = pending();
    assert!(wants_plan(&app));
    let effective = app.launch.effective();
    app.launch
        .plan(app.selection.clone(), effective, Some(options("planned")));
    assert!(app.launch.planned);
    assert!(!wants_plan(&app));

    // Switching model before launch changes the policy, so it is re-asked.
    app.selection = Selection::parse("claude").unwrap();
    assert!(wants_plan(&app));
    // A failed plan still counts as asked: the server is not polled per frame.
    let effective = app.launch.effective();
    app.launch.plan(app.selection.clone(), effective, None);
    assert!(!wants_plan(&app));
    assert_eq!(
        app.launch.driva.as_ref().unwrap().isolation_backend,
        "planned"
    );

    // Once something is running, what it runs under replaces the plan and
    // no further planning happens.
    app.activity.status = Status::Running;
    app.launch.record(options("live"));
    assert!(!app.launch.planned);
    assert!(!wants_plan(&app));
    app.selection = Selection::parse("codex").unwrap();
    assert!(!wants_plan(&app));
    assert_eq!(app.launch.driva.as_ref().unwrap().isolation_backend, "live");
}

/// The reported flow: stop a session, add a mount, send a new message. The
/// mount has to reach the resume, and the view has to stop describing the
/// sandbox the stopped interaction ran in — otherwise the edit reads as
/// having been discarded and the old policy as having been restored.
#[test]
fn a_stopped_or_ended_session_re_plans_its_policy_before_the_resume() {
    for status in [
        Status::Stopped(StopReason::Paused),
        Status::ended(Some(0), None),
    ] {
        let mut app = running();
        app.launch.record(options("live"));
        // While it runs, the record stands and nothing is asked.
        assert!(!app.can_edit_launch());
        assert!(!wants_plan(&app));

        app.activity.status = status;
        assert!(app.can_edit_launch());
        // Nothing has changed yet, but the record is the previous
        // interaction's, so the sandbox a resume would use is asked for.
        assert!(wants_plan(&app));
        let effective = app.launch.effective();
        app.launch
            .plan(app.selection.clone(), effective, Some(options("live")));
        assert!(!wants_plan(&app));

        // The edit lands in the half the resume sends...
        add_mount(&mut app, "/srv/data");
        assert_eq!(app.launch.effective().mounts.len(), 1);
        // ...and is re-planned, so the view answers for it.
        assert!(wants_plan(&app));
    }
}

/// The launch policy is only editable while nothing has started: once an
/// interaction is running, the driva view is a record of the sandbox it is
/// confined to and changing it would mean a new session.
#[test]
fn the_launch_policy_cannot_be_edited_once_an_interaction_is_running() {
    let mut app = running();
    assert!(!app.can_edit_launch());
    cycle_workspace_access(&mut app);
    assert_eq!(app.launch.interaction.writable_workspace, None);

    cycle_network(&mut app);
    assert_eq!(app.launch.interaction.network, None);
    set_templates(&mut app, vec!["rust".into()]);
    assert!(app.launch.interaction.templates.is_empty());
    open_prompt(&mut app);
    assert!(app.launch.prompt.is_none());
    toggle_ignore_workspace(&mut app);
    assert!(!app.launch.interaction.ignore_workspace);

    let mut pending = pending();
    assert!(pending.can_edit_launch());
    cycle_network(&mut pending);
    assert_eq!(pending.launch.interaction.network, Some(true));
}

/// An idle agent is not in the middle of anything, so a mount is not
/// refused: it is added, and the interaction is marked to be restarted
/// under it. The rest of the policy stays a record.
#[test]
fn an_idle_interaction_takes_a_mount_by_restarting() {
    let mut app = running();
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    assert!(!app.can_edit_launch());

    cycle_network(&mut app);
    assert_eq!(app.launch.interaction.network, None);
    assert!(!app.restart_for_mounts);

    // Said before the mount is typed, and again once it is added.
    open_prompt(&mut app);
    assert!(app.notices.iter().any(|message| message
        .text
        .contains("adding a mount restarts the interaction")));
    add_mount(&mut app, "/srv/data");
    assert_eq!(app.launch.interaction.mounts.len(), 1);
    assert!(app.restart_for_mounts);
    assert!(app.notices.iter().any(|message| message
        .text
        .contains("restarting the interaction to apply it")));

    app.restart_for_mounts = false;
    remove_selected_mount(&mut app);
    assert!(app.launch.interaction.mounts.is_empty());
    assert!(app.restart_for_mounts);
}

/// The Workspace's layer is the server's, so the edit is sent, and the
/// restart waits on it in the event loop rather than being made here.
#[test]
fn an_idle_interactions_workspace_mount_is_sent_and_restarts() {
    let mut app = running();
    app.activity.status = Status::Idle(IdleReason::TurnComplete);
    toggle_scope(&mut app);

    add_mount(&mut app, "/srv/data");

    assert!(matches!(
        app.take_request(),
        Some(Request::ChangeWorkspaceLaunch {
            change: WorkspaceLaunchChange::AddMounts(_),
            ..
        })
    ));
    assert!(app.restart_for_mounts);
}

/// A stopped agent's next launch takes the mount without a restart.
#[test]
fn a_stopped_interaction_takes_a_mount_without_restarting() {
    let mut stopped = running();
    stopped.activity.status = Status::Stopped(StopReason::Paused);
    add_mount(&mut stopped, "/srv/data");
    assert_eq!(stopped.launch.interaction.mounts.len(), 1);
    assert!(!stopped.restart_for_mounts);
    assert!(stopped
        .notices
        .iter()
        .all(|message| !message.text.contains("restart")));
}

#[test]
fn another_launch_edit_waits_for_the_workspace_snapshot_being_saved() {
    let mut app = pending_in_a_workspace_with_a_policy();
    app.workspace_launch_pending = 1;
    let before = app.launch.effective();

    cycle_network(&mut app);
    set_templates(&mut app, vec!["browser".into()]);

    assert_eq!(app.launch.effective(), before);
    assert_eq!(app.take_request(), None);
    assert!(app
        .notices
        .iter()
        .all(|message| message.text == "the Workspace launch policy is still saving"));
}

#[test]
fn the_workspace_reads_read_only_from_the_resolved_mount() {
    let bind = |origin, access| AttributedMount {
        origin,
        mount: Mount::Bind {
            source: PathBuf::from("/tmp/styra/workspace"),
            destination: PathBuf::from("/tmp/styra/workspace"),
            access,
        },
    };
    let mut app = running();
    assert!(!app.launch.workspace_read_only(), "no policy known yet");
    app.launch.record(DrivaOptions {
        mounts: vec![
            bind(MountOrigin::Workspace, MountAccess::ReadOnly),
            bind(MountOrigin::Operator, MountAccess::ReadWrite),
        ],
        ..options("bubblewrap")
    });
    assert!(app.launch.workspace_read_only());
    app.launch.record(DrivaOptions {
        mounts: vec![
            bind(MountOrigin::Workspace, MountAccess::ReadWrite),
            bind(MountOrigin::GitRepository, MountAccess::ReadOnly),
        ],
        ..options("bubblewrap")
    });
    assert!(!app.launch.workspace_read_only());
}

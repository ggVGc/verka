use super::*;

fn view<'a>(
    editable: bool,
    workspace: &'a LaunchPolicy,
    interaction: &'a LaunchPolicy,
) -> DrivaView<'a> {
    DrivaView {
        chrome: PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
            into: None,
            source_branch: None,
            parent: None,
            agent: "codex".into(),
            model: "gpt".into(),
            model_reported: false,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: crate::chrome::StatusTone::Idle,
            elapsed: None,
            suffix: None,
            session: None,
        },
        editable,
        mounts_editable: true,
        tab: DetailsTab::Details,
        details_requested_scroll: 0,
        launch: DrivaLaunch {
            workspace,
            interaction,
            scope: LaunchScope::Interaction,
            workspace_cursor: 0,
            interaction_cursor: 0,
            prompt: None,
            paths: None,
            command_prompt: None,
            driva: None,
            planned: false,
            requested_scroll: 0,
        },
        workspace: DrivaWorkspace {
            id: None,
            name: None,
            given_name: None,
            git_repository: None,
            host_path: None,
            server_path: None,
            session_count: None,
            age: None,
            created_at_ms: None,
            last_accessed_at_ms: None,
            root: None,
            working_directory: None,
            worktrees: None,
        },
        activity: DrivaActivity {
            status: if editable {
                DrivaStatus::Stopped
            } else {
                DrivaStatus::Idle
            },
        },
        project_commands: ProjectAction::ALL.map(|action| {
            LaunchPolicy::merge(workspace, interaction)
                .command(action)
                .unwrap_or("not configured")
                .to_owned()
        }),
        project_action_keys: ["r".into(), "R".into()],
        selection_name: "codex".into(),
        session_id: "s1",
        session_name: None,
        queued_count: 0,
        last_message: None,
        workspace_launch_pending: 0,
        git_repository_prompt: None,
        checkout: None,
        branched_from: None,
    }
}

fn dimmed(span: &Span<'_>) -> bool {
    span.style.add_modifier.contains(Modifier::DIM)
}

/// The span of `lines` whose text contains `needle`.
fn span<'l>(lines: &'l [Line<'static>], needle: &str) -> &'l Span<'static> {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.contains(needle))
        .unwrap_or_else(|| panic!("no span reads {needle:?} in {lines:?}"))
}

#[test]
fn current_commands_stay_visible_on_both_options_tabs_without_a_sandbox() {
    let policy = LaunchPolicy::default();
    let mut app = view(false, &policy, &policy);
    app.mounts_editable = false;
    app.project_commands = [
        "cargo run --release".into(),
        "cargo test --workspace".into(),
    ];
    for tab in [DetailsTab::Details, DetailsTab::Sandbox] {
        app.tab = tab;
        let backend = ratatui::backend::TestBackend::new(90, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &app, frame.area());
            })
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("cargo run --release"));
        assert!(screen.contains("cargo test --workspace"));
    }
}

/// Each layer says what it states, or what an unstated command falls
/// through to; the summary names the layer the effective one comes from.
#[test]
fn a_project_command_names_the_layer_it_comes_from() {
    let workspace = LaunchPolicy {
        test: Some("cargo nextest run".into()),
        ..LaunchPolicy::default()
    };
    let interaction = LaunchPolicy {
        run: Some("cargo run --release".into()),
        ..LaunchPolicy::default()
    };
    let options = DrivaOptions {
        template_run: Some("cargo run".into()),
        template_test: Some("cargo test".into()),
        ..DrivaOptions::default()
    };
    let mut app = view(true, &workspace, &interaction);
    app.launch.driva = Some(&options);

    assert_eq!(
        effective_command_label(&app, ProjectAction::Run),
        "cargo run --release"
    );
    assert_eq!(
        effective_command_label(&app, ProjectAction::Test),
        "cargo nextest run"
    );
    assert_eq!(
        scope_command_label(&app, LaunchScope::Interaction, ProjectAction::Test),
        "not stated — inherits cargo nextest run"
    );
    assert_eq!(
        scope_command_label(&app, LaunchScope::Workspace, ProjectAction::Run),
        "not stated — templates run cargo run"
    );

    app.launch.driva = None;
    assert_eq!(
        scope_command_label(&app, LaunchScope::Workspace, ProjectAction::Run),
        "none — c sets one"
    );
}

/// On an idle interaction the keys that cannot act are still shown —
/// dimmed, with the reason — rather than dropped from the hints.
#[test]
fn an_idle_interactions_fixed_keys_are_shown_disabled() {
    let policy = LaunchPolicy::default();
    let lines = hint_lines(&view(false, &policy, &policy));

    for usable in ["m mount", "x remove"] {
        assert!(!dimmed(span(&lines, usable)), "{usable} should be usable");
    }
    for fixed in [
        "T templates",
        "w network",
        "R workspace",
        "I ignore",
        "U move up",
        "D save",
    ] {
        assert!(dimmed(span(&lines, fixed)), "{fixed} should be disabled");
    }
    assert!(dimmed(span(&lines, "fixed while the interaction is live")));
    span(&lines, "changing a mount restarts the interaction");
}

#[test]
fn nothing_is_disabled_while_the_whole_policy_is_editable() {
    let policy = LaunchPolicy::default();
    let lines = hint_lines(&view(true, &policy, &policy));

    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|s| !dimmed(s)));
    assert!(!text(lines).concat().contains("fixed while"));
}

/// The pane matches its hints: the settings above the mounts are dimmed,
/// the mounts the keys still reach are not.
#[test]
fn an_idle_interactions_pane_dims_all_but_its_mounts() {
    let workspace = LaunchPolicy::default();
    let interaction = LaunchPolicy {
        mounts: vec![styra_protocol::LaunchMount {
            source: PathBuf::from("/srv/data"),
            destination: None,
            writable: false,
        }],
        ..LaunchPolicy::default()
    };
    let idle = view(false, &workspace, &interaction);
    let rows = pane_rows(&idle, LaunchScope::Interaction);

    assert!(dimmed(span(&rows, "network")));
    assert!(dimmed(span(&rows, "templates")));
    assert!(!dimmed(span(&rows, "mounts")));
    assert!(!dimmed(span(&rows, "/srv/data")));
}

fn text(lines: Vec<Line<'static>>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn state(worktree: &str, repository: &str, branch: Option<&str>) -> CheckoutState {
    CheckoutState {
        worktree: PathBuf::from(worktree),
        repository: PathBuf::from(repository),
        branch: branch.map(ToOwned::to_owned),
    }
}

#[test]
fn checkout_rows_name_the_branch_location_and_repository_state() {
    for (name, checkout, present, absent) in [
        (
            "plain",
            Some(state("/work/project", "/work/project", Some("main"))),
            vec![(0, "branch"), (0, "main"), (1, "/work/project")],
            vec![(1, "linked")],
        ),
        // A linked checkout also names the repository it belongs to.
        (
            "linked",
            Some(state(
                "/styra/worktrees/session",
                "/work/project",
                Some("styra/session"),
            )),
            vec![
                (0, "styra/session"),
                (1, "/styra/worktrees/session"),
                (1, "linked from /work/project"),
            ],
            vec![],
        ),
        (
            "detached",
            Some(state("/work", "/work", None)),
            vec![(0, "detached head")],
            vec![],
        ),
        (
            "outside repository",
            None,
            vec![(0, "not a Git checkout")],
            vec![],
        ),
    ] {
        let lines = text(checkout_lines(checkout.as_ref()));
        if checkout.is_none() {
            assert_eq!(lines.len(), 1, "{name}");
        }
        for (index, needle) in present {
            assert!(
                lines[index].contains(needle),
                "{name}: missing {needle}: {lines:?}"
            );
        }
        for (index, needle) in absent {
            assert!(
                !lines[index].contains(needle),
                "{name}: unexpected {needle}: {lines:?}"
            );
        }
    }
}

#[test]
fn branch_points_name_the_branch_or_detached_head_and_shorten_the_commit() {
    for (branch, expected) in [
        (Some("main"), "main @ 3f2a1b4c9d"),
        (None, "detached head @ 3f2a1b4c9d"),
    ] {
        let line = text(vec![branched_from_line(&BranchPoint {
            branch: branch.map(ToOwned::to_owned),
            commit: "3f2a1b4c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a".to_owned(),
        })]);
        assert!(line[0].contains("branched from"), "{branch:?}: {line:?}");
        assert!(line[0].contains(expected), "{branch:?}: {line:?}");
        assert!(!line[0].contains("3f2a1b4c9d8"), "{branch:?}: {line:?}");
    }
}

fn row(lines: &[String], label: &str) -> String {
    lines
        .iter()
        .find(|line| line.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no {label} row in {lines:?}"))
        .clone()
}

#[test]
fn the_workspace_row_spells_out_the_path_the_placeholder_stands_for() {
    let policy = LaunchPolicy::default();
    let mut view = view(true, &policy, &policy);
    view.workspace.root = Some(PathBuf::from("/home/me/proj"));
    view.workspace.working_directory = Some(PathBuf::from("/home/me/proj/src"));
    let lines = text(interaction_lines(&view));

    assert!(
        row(&lines, "workspace")
            .trim_end()
            .ends_with(" /home/me/proj (<ws>)"),
        "{lines:?}"
    );
    assert!(row(&lines, "working dir").contains("<ws>/src"), "{lines:?}");
}

/// An interaction in a linked worktree reads by the worktree's name, with
/// the directory all worktrees share spelled out once in the Workspace.
#[test]
fn a_linked_worktree_is_named_under_the_worktrees_placeholder() {
    let policy = LaunchPolicy::default();
    let mut view = view(true, &policy, &policy);
    view.workspace.host_path = Some(PathBuf::from("/home/me/proj"));
    view.workspace.worktrees = Some(PathBuf::from("/store/w1/worktrees"));
    view.workspace.root = Some(PathBuf::from("/store/w1/worktrees/fix-retries"));
    view.workspace.working_directory = Some(PathBuf::from("/store/w1/worktrees/fix-retries/src"));
    view.checkout = Some(state(
        "/store/w1/worktrees/fix-retries",
        "/home/me/proj",
        Some("styra/fix-retries"),
    ));
    let interaction = text(interaction_lines(&view));
    let workspace = text(workspace_lines(&view));

    assert!(
        row(&interaction, "workspace")
            .contains("/home/me/proj (<ws>) · rooted at <wt>/fix-retries"),
        "{interaction:?}"
    );
    assert!(row(&interaction, "working dir").contains("<wt>/fix-retries/src"));
    assert!(row(&interaction, "worktree").contains("<wt>/fix-retries · linked from <ws>"));
    assert!(row(&workspace, "worktrees").contains("/store/w1/worktrees (<wt>)"));
    assert!(row(&workspace, "host path").contains("<ws>"));
}

#[test]
fn mount_rows_name_the_workspace_by_its_placeholder() {
    let workspace = LaunchPolicy::default();
    let interaction = LaunchPolicy {
        mounts: vec![LaunchMount {
            source: PathBuf::from("/home/me/proj/data"),
            destination: None,
            writable: false,
        }],
        ..LaunchPolicy::default()
    };
    let mut view = view(true, &workspace, &interaction);
    view.workspace.host_path = Some(PathBuf::from("/home/me/proj"));
    let rows = text(pane_rows(&view, LaunchScope::Interaction));

    assert!(
        rows.iter().any(|row| row.contains("<ws>/data (ro)")),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|row| row.contains("/home/me")), "{rows:?}");
}

#[test]
fn sandbox_summary_mounts_are_compact_without_losing_their_target() {
    assert_eq!(
        details_resolved_mount_label(&Mount::Bind {
            source: PathBuf::from("/workspace"),
            destination: PathBuf::from("/workspace"),
            access: MountAccess::ReadWrite,
        }),
        "/workspace (rw)"
    );
    assert_eq!(
        details_resolved_mount_label(&Mount::Bind {
            source: PathBuf::from("/host/cache"),
            destination: PathBuf::from("/cache"),
            access: MountAccess::ReadOnly,
        }),
        "/host/cache → /cache (ro)"
    );
}

#[test]
fn main_details_mount_labels_do_not_repeat_a_same_path_destination() {
    let path = PathBuf::from("/workspace");
    let line = mount_line(&Mount::Bind {
        source: path.clone(),
        destination: path.clone(),
        access: MountAccess::ReadWrite,
    });
    assert_eq!(text(vec![line]), ["    rw /workspace → /workspace"]);
    assert_eq!(
        launch_mount_label(&LaunchMount {
            source: path.clone(),
            destination: Some(path),
            writable: true,
        }),
        "/workspace (rw)"
    );
}

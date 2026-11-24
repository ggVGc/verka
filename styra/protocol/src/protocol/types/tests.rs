use super::*;

/// Not every state has a reason to report — a running turn is its own
/// explanation — so a summary without one has to read as saying nothing
/// about why rather than failing to read at all.
#[test]
fn a_summary_that_states_no_reason_still_decodes() {
    let summary: InteractionSummary = serde_json::from_str(
        r#"{
                "id": "s1",
                "workspace_id": "w1",
                "selection": {"provider": "claude", "model": "sonnet", "effort": "medium"},
                "workspace": "/srv/work",
                "activity": "pending"
            }"#,
    )
    .expect("a summary without a reason is a summary");

    assert_eq!(summary.activity, InteractionActivity::Pending);
    assert_eq!(summary.activity_reason, None);
}

/// The question every reader of a summary asks, answered in one place
/// rather than by pairing a state with a flag beside it.
#[test]
fn only_a_stopped_interaction_takes_no_messages() {
    for activity in [
        InteractionActivity::Pending,
        InteractionActivity::Running,
        InteractionActivity::Background,
    ] {
        assert!(activity.accepting(), "{activity:?}");
    }
    assert!(!InteractionActivity::Stopped.accepting());
}

/// A restart or a fresh branch leaves work dormant, still open; a stop
/// the operator chose, or one that went wrong, does not.
#[test]
fn only_stops_nobody_chose_leave_an_interaction_open() {
    let mut summary: InteractionSummary = serde_json::from_str(
        r#"{
                "id": "s1",
                "workspace_id": "w1",
                "selection": {"provider": "claude", "model": "sonnet", "effort": "medium"},
                "workspace": "/srv/work",
                "activity": "pending"
            }"#,
    )
    .unwrap();
    assert!(summary.open() && !summary.dormant());

    summary.activity = InteractionActivity::Stopped;
    for (reason, dormant) in [
        (Some(InteractionActivityReason::ServerRestarted), true),
        (
            Some(InteractionActivityReason::ServerRestartedMidTurn),
            true,
        ),
        (Some(InteractionActivityReason::Branched), true),
        (Some(InteractionActivityReason::Paused), false),
        (
            Some(InteractionActivityReason::Exited { exit_code: Some(1) }),
            false,
        ),
        (
            Some(InteractionActivityReason::Failed {
                message: "boom".into(),
            }),
            false,
        ),
        (None, false),
    ] {
        summary.activity_reason = reason.clone();
        assert_eq!(summary.dormant(), dormant, "{reason:?}");
        assert_eq!(summary.open(), dormant, "{reason:?}");
    }
}

/// The reason travels as its own tagged object, so a variant that carries
/// detail (which window, which error) is one shape with the ones that
/// carry none.
#[test]
fn a_reason_carries_its_detail_across_the_wire() {
    let reason = InteractionActivityReason::RateLimited {
        window: "five_hour".into(),
        resets_at_ms: Some(1_700_000_000_000),
    };

    let line = serde_json::to_string(&reason).expect("a reason serializes");

    assert_eq!(
        line,
        r#"{"reason":"rate_limited","detail":{"window":"five_hour","resets_at_ms":1700000000000}}"#
    );
    assert_eq!(
        serde_json::from_str::<InteractionActivityReason>(&line).expect("and reads back"),
        reason
    );
}

fn mount(source: &str, destination: Option<&str>, writable: bool) -> LaunchMount {
    LaunchMount {
        source: PathBuf::from(source),
        destination: destination.map(PathBuf::from),
        writable,
    }
}

fn workspace_policy() -> LaunchPolicy {
    LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![mount("/srv/corpus", None, false)],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    }
}

/// The ordinary case: the Workspace says what every launch here needs, the
/// launch says what is particular to it, and the sandbox gets both.
#[test]
fn a_launch_adds_to_the_workspace_policy_by_default() {
    let base = workspace_policy();
    let overlay = LaunchPolicy {
        templates: vec!["browser".into()],
        mounts: vec![mount("/srv/scratch", None, true)],
        ..LaunchPolicy::default()
    };

    let merged = LaunchPolicy::merge(&base, &overlay);
    assert_eq!(merged.templates, vec!["rust", "browser"]);
    assert_eq!(
        merged.mounts,
        vec![
            mount("/srv/corpus", None, false),
            mount("/srv/scratch", None, true)
        ]
    );
    // Inherited, because the overlay said nothing about it.
    assert_eq!(merged.network, Some(true));
    assert!(merged.grants_network());
}

/// Overriding, in the three ways an additive overlay can express it: a
/// stated network answer, a stated workspace access, a template name that
/// moves later in the layering, and a mount on a destination the Workspace
/// already binds.
#[test]
fn an_overlay_overrides_the_workspace_on_the_grants_it_names() {
    let base = LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into(), "browser".into()],
        mounts: vec![mount("/srv/corpus", Some("/mnt/corpus"), false)],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    };
    let overlay = LaunchPolicy {
        network: Some(false),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![mount("/srv/other", Some("/mnt/corpus"), true)],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    };

    let merged = LaunchPolicy::merge(&base, &overlay);
    assert_eq!(merged.network, Some(false));
    // Naming a template the Workspace already layers moves it last, where
    // it wins on conflict, rather than duplicating it.
    assert_eq!(merged.templates, vec!["browser", "rust"]);
    // One destination, bound to what this launch asked for: the two would
    // otherwise collide and be refused outright.
    assert_eq!(
        merged.mounts,
        vec![mount("/srv/other", Some("/mnt/corpus"), true)]
    );
}

/// A mount naming no destination lands on its own source, so that is what
/// an overlay has to match to replace it.
#[test]
fn a_mount_with_no_destination_is_replaced_through_its_source() {
    let base = LaunchPolicy {
        mounts: vec![mount("/srv/corpus", None, false)],
        ..LaunchPolicy::default()
    };
    let overlay = LaunchPolicy {
        mounts: vec![mount("/srv/corpus", None, true)],
        ..LaunchPolicy::default()
    };

    let merged = LaunchPolicy::merge(&base, &overlay);
    assert_eq!(merged.mounts, vec![mount("/srv/corpus", None, true)]);
}

/// Dropping a grant the Workspace makes cannot be said by adding to it, so
/// `ignore_workspace` is the one way a launch says "not that policy, this one".
#[test]
fn an_ignore_workspace_launch_ignores_the_workspace_policy_entirely() {
    let base = workspace_policy();
    let overlay = LaunchPolicy {
        templates: vec!["browser".into()],
        ignore_workspace: true,
        ..LaunchPolicy::default()
    };

    let merged = LaunchPolicy::merge(&base, &overlay);
    assert_eq!(merged.templates, vec!["browser"]);
    assert!(merged.mounts.is_empty());
    assert_eq!(merged.network, None);
    assert!(!merged.grants_network());
    // The result is a resolved policy: there is nothing left below it.
    assert!(!merged.ignore_workspace);
}

#[test]
fn the_old_standalone_spelling_loads_as_ignore_workspace() {
    let policy: LaunchPolicy =
        serde_json::from_value(serde_json::json!({ "standalone": true })).unwrap();
    assert!(policy.ignore_workspace);

    let stored = serde_json::to_value(policy).unwrap();
    assert_eq!(stored["ignore_workspace"], true);
    assert!(stored.get("standalone").is_none());
}

/// A launch that asks for nothing runs under exactly the Workspace's
/// policy — the property that makes a standing policy worth storing.
#[test]
fn an_empty_overlay_leaves_the_workspace_policy_as_it_is() {
    let base = workspace_policy();
    assert!(LaunchPolicy::default().is_empty());
    assert_eq!(LaunchPolicy::merge(&base, &LaunchPolicy::default()), base);
}

/// The workspace mount is writable unless some layer says otherwise, and
/// either layer can say it: this is the one grant whose absent answer is
/// the permissive one, so the default has to survive a merge that states
/// nothing and be overridable from both sides.
#[test]
fn the_workspace_mount_is_writable_until_a_layer_says_otherwise() {
    assert!(LaunchPolicy::default().grants_writable_workspace(true));
    // In a main worktree the unstated answer is read-only instead.
    assert!(!LaunchPolicy::default().grants_writable_workspace(false));
    // Stating it is asking for something, so the Workspace that holds it is
    // not one with no policy at all.
    assert!(!LaunchPolicy {
        writable_workspace: Some(false),
        ..LaunchPolicy::default()
    }
    .is_empty());

    let read_only = LaunchPolicy {
        writable_workspace: Some(false),
        ..LaunchPolicy::default()
    };
    // Stated by the Workspace, inherited by a launch that says nothing.
    let merged = LaunchPolicy::merge(&read_only, &LaunchPolicy::default());
    assert_eq!(merged.writable_workspace, Some(false));
    assert!(!merged.grants_writable_workspace(true));

    // And taken back by one that does say something.
    let merged = LaunchPolicy::merge(
        &read_only,
        &LaunchPolicy {
            writable_workspace: Some(true),
            ..LaunchPolicy::default()
        },
    );
    assert!(merged.grants_writable_workspace(true));
    // Stated, it holds over the main worktree's read-only default too.
    assert!(merged.grants_writable_workspace(false));

    // Stated by the launch alone, over a Workspace with no answer.
    let merged = LaunchPolicy::merge(&LaunchPolicy::default(), &read_only);
    assert!(!merged.grants_writable_workspace(true));

    // An ignore-workspace launch carries its own answer and nothing else's.
    let merged = LaunchPolicy::merge(
        &LaunchPolicy {
            writable_workspace: Some(false),
            ..LaunchPolicy::default()
        },
        &LaunchPolicy {
            ignore_workspace: true,
            ..LaunchPolicy::default()
        },
    );
    assert!(merged.grants_writable_workspace(true));
}

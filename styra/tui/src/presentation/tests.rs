use super::test_support::{self, inspect};
use super::*;
use styra_protocol::event::{AgentEvent, TurnOutcome, TurnUsage};
use styra_protocol::{CheckoutState, InteractionSummary};
use styra_ui::application::MainView;

#[test]
fn branch_uses_the_message_box_with_its_own_draft_and_history_selector() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "source",
    );
    app.composer.set("source draft".into());
    let mut prompt = crate::branch::BranchPrompt::new(42, app.selection.clone());
    prompt.composer.set("branch draft".into());
    app.branch_prompt = Some(prompt);
    let input = modal_input(&app);
    assert_eq!(input.parts, ["branch draft"]);
    assert!(input.cursor);
    assert!(input.preceding[0].contains("All through this entry"));
    assert!(input.notice.as_ref().unwrap().contains("Ctrl+Enter"));
    app.branch_prompt.as_mut().unwrap().change();
    assert!(modal_input(&app).preceding[0].contains("This entry only"));
}

#[test]
fn turn_diff_mapping_passes_semantic_chrome_and_the_original_message() {
    let mut app = test_support::app("s1");
    app.push_event(AgentEvent::UserMessage {
        text: "fix the retry backoff".into(),
    });
    app.push_event(AgentEvent::FileChanged {
        id: String::new(),
        paths: vec!["a.rs".into()],
        diff: Some("@@ -1 +1 @@\n-old\n+new".into()),
        checkpoint: None,
        checkpoint_error: None,
    });
    app.workspace.name = Some("payments".into());
    app.diff_preview = true;
    app.diff_turn = true;
    app.sync_turn_diff();
    // Under the log, the pane repeats neither the selected message nor
    // where the interaction is.
    let view = shown_diff(&app).unwrap();
    assert!(view.intro_message.is_none());
    let chrome = view.chrome.unwrap();
    assert_eq!(chrome.workspace, None);
    assert_eq!(chrome.session, None);
    app.diff_maximized = true;
    let view = shown_diff(&app).unwrap();
    let message = view.intro_message.unwrap();
    assert!(
        matches!(message.event, AgentEvent::UserMessage { text } if text == "fix the retry backoff")
    );
    assert_eq!(message.protocol, app.selection.provider.protocol());
    let chrome = view.chrome.unwrap();
    assert!(chrome.focused);
    assert!(chrome.suffix.unwrap().starts_with("turn diff"));
}

#[test]
fn ui_receives_requested_anchor_and_feedback_is_applied_explicitly() {
    let mut app = test_support::app("s1");
    app.timeline.list_offset = 19;
    let mut ui = crate::test_ui::TestUi::default();
    ui.application = Box::new(|view| {
        assert_eq!(view.session_id, "s1");
        let MainView::Events(events) = &view.main else {
            panic!("events expected")
        };
        assert_eq!(events.list.requested_offset, 19);
        Ok(styra_ui::RenderFeedback {
            list_offset: Some(7),
            list_row_offset: Some(2),
            ..Default::default()
        })
    });
    let feedback = draw_application(&mut ui, &app).unwrap();
    assert_eq!(ui.applications, 1);
    assert_eq!(app.timeline.list_offset, 19);
    apply_feedback(&mut app, &feedback);
    assert_eq!(
        (app.timeline.list_offset, app.timeline.list_row_offset),
        (7, 2)
    );
}

/// What lets the event loop stop drawing an idle Styra: an idle frame has
/// nothing on it read off the clock, so leaving it up is leaving it right.
/// A running one counts seconds and has to keep being repainted.
#[test]
fn only_a_frame_with_a_clock_on_it_needs_repainting_on_its_own() {
    let mut app = test_support::app("s1");

    app.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
    assert_eq!(clock_reading(&app), None, "an idle screen is a still one");

    app.activity.status = Status::Running;
    assert!(
        clock_reading(&app).is_some(),
        "a running turn counts seconds"
    );

    app.activity.status = Status::Background;
    assert!(clock_reading(&app).is_some(), "so does background work");
}

/// The reading is the text, so a figure that is drawn the same is the same
/// reading, and the loop leaves the frame alone. `format_duration` writes
/// an hour or more as `1h04m`: a session stopped this morning holds those
/// six characters for a minute at a time, and repainting it each second
/// paints identical pixels fifty-nine times out of sixty.
#[test]
fn an_elapsed_figure_too_coarse_to_have_moved_reads_the_same() {
    // Dated from the server's moment, which is how a client attaching to
    // an interaction that stopped a while ago gets the age in the first
    // place — rather than by reaching into the clock behind it.
    let stopped_for = |seconds: u64| {
        let mut app = test_support::app("s1");
        app.activity.adopt_server_status(
            crate::activity::Status::Stopped(crate::activity::StopReason::Completed),
            quota::now_ms() - seconds * 1_000,
        );
        clock_reading(&app)
    };

    let hour = stopped_for(3_600 + 4 * 60);
    assert_eq!(
        hour,
        stopped_for(3_600 + 4 * 60 + 30),
        "half a minute later is the same `1h04m` on screen"
    );
    assert_ne!(hour, stopped_for(3_600 + 5 * 60), "a minute later is not");

    // Under the hour the figure carries seconds, so it does move each one.
    assert_ne!(stopped_for(74), stopped_for(75));
}

fn quota(
    status: styra_protocol::QuotaStatus,
    resets_at_ms: Option<u64>,
) -> styra_protocol::QuotaEvent {
    styra_protocol::QuotaEvent {
        at_ms: 1_000,
        session_id: "s1".into(),
        provider: styra_protocol::agent::Provider::Codex,
        window: "5h".into(),
        status,
        utilization: Some(0.91),
        resets_at_ms,
        detail: None,
    }
}

/// The whole point of the narrow reading of "ticking". A utilization figure
/// in the footer is a figure taken at a moment: it stands until the next
/// reading replaces it, and repainting it changes nothing. Counting it as
/// a clock left an otherwise idle Styra repainting once a second forever,
/// which is what this exists to stop.
#[test]
fn a_standing_quota_figure_does_not_keep_an_idle_screen_repainting() {
    use styra_protocol::QuotaStatus;
    let mut app = test_support::app("s1");
    app.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);

    app.note_quota(quota(QuotaStatus::Warning, None));

    assert!(
        !quota::alert(&app).is_empty(),
        "the footer is showing the figure",
    );
    assert_eq!(
        clock_reading(&app),
        None,
        "but the figure is not going to move"
    );
}

/// A refusal quotes the moment work will be taken again, and stops quoting
/// it once that moment lands — with no new reading to mark the change. So
/// it ticks until it falls due, and is still again afterwards.
#[test]
fn a_refusal_ticks_only_until_the_moment_it_names() {
    use styra_protocol::QuotaStatus;
    let mut ahead = test_support::app("s1");
    ahead.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
    ahead.note_quota(quota(QuotaStatus::Exhausted, Some(u64::MAX)));

    assert!(
        clock_reading(&ahead).is_some(),
        "the moment is still ahead of us"
    );

    let mut passed = test_support::app("s1");
    passed.activity.status = Status::Idle(crate::activity::IdleReason::TurnComplete);
    passed.note_quota(quota(QuotaStatus::Exhausted, Some(1_000)));

    assert_eq!(
        clock_reading(&passed),
        None,
        "and that one has already landed"
    );
}

#[test]
fn header_data_contains_selection_and_status() {
    inspect(&test_support::app("s1"), |v| {
        let MainView::Events(events) = &v.main else {
            panic!("events expected")
        };
        assert_eq!(events.list.chrome.agent, "codex");
        assert_eq!(events.list.chrome.status, "running");
    });
}

#[test]
fn header_data_resolves_workspace_name() {
    let mut app = test_support::app("s1");
    app.workspace.name = Some("payments".into());
    inspect(&app, |v| {
        let MainView::Events(events) = &v.main else {
            panic!("events expected")
        };
        assert_eq!(events.list.chrome.workspace.as_deref(), Some("payments"));
    });
}

#[test]
fn header_data_resolves_interaction_title() {
    let mut app = test_support::app("s1");
    app.workspace.name = Some("payments".into());
    app.interactions.note_name("s1", Some("Fix retries".into()));
    inspect(&app, |v| {
        let MainView::Events(events) = &v.main else {
            panic!("events expected")
        };
        assert_eq!(events.list.chrome.session.as_deref(), Some("Fix retries"));
    });
}

/// An interaction branched off another names that parent in its header;
/// one that was not names none.
#[test]
fn header_data_names_the_parent_it_was_branched_off() {
    use styra_protocol::agent::Provider;
    let mut app = test_support::app("s1");
    let parent = |app: &App| {
        let mut parent = None;
        inspect(app, |v| {
            let MainView::Events(events) = &v.main else {
                panic!("events expected")
            };
            parent = events.list.chrome.parent.clone();
        });
        parent
    };
    assert_eq!(parent(&app), None);
    let origin = styra_protocol::SessionOrigin {
        session_id: "source".into(),
        provider: Provider::Claude,
        at_ms: None,
        history: styra_protocol::BranchHistory::ThroughSelected,
    };
    app.interactions
        .note_origin("s1", Provider::Claude, Some(&origin));
    assert_eq!(parent(&app).as_deref(), Some("untitled"));
    app.interactions
        .note_name("source", Some("Retry backoff".into()));
    assert_eq!(parent(&app).as_deref(), Some("Retry backoff"));
}

#[test]
fn lone_event_list_only_requests_focus_when_another_pane_opens() {
    let mut app = test_support::app("s1");
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(!e.list.chrome.focused);
    });
    app.preview.open = true;
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(e.list.chrome.focused);
        assert!(e.preview.is_some());
    });
}

fn interaction_with_checkout(id: &str, checkout: Option<CheckoutState>) -> InteractionSummary {
    InteractionSummary {
        checkout,
        ..crate::interactions::tests::interaction(id, styra_protocol::InteractionActivity::Running)
    }
}

#[test]
fn header_names_only_linked_worktrees() {
    for (worktree, branch, expected) in [
        (
            "/workspace/.worktrees/fix-retries",
            "fix-retries",
            Some("fix-retries"),
        ),
        ("/workspace", "main", None),
    ] {
        let mut app = test_support::app("s1");
        app.interactions.items = vec![interaction_with_checkout(
            "s1",
            Some(CheckoutState {
                worktree: worktree.into(),
                repository: "/workspace".into(),
                branch: Some(branch.into()),
            }),
        )];
        inspect(&app, |v| {
            let MainView::Events(events) = &v.main else {
                panic!("events expected")
            };
            assert_eq!(
                events.list.chrome.worktree.as_deref(),
                expected,
                "{worktree}"
            );
        });
    }
}

#[test]
fn event_list_data_carries_both_filter_flags() {
    let mut app = test_support::app("s1");
    for (all, minor) in [(false, false), (true, false), (true, true)] {
        app.timeline.all_events = all;
        app.timeline.show_minor = minor;
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!((e.list.all_events, e.list.show_minor), (all, minor));
        });
    }
}

#[test]
fn turn_completion_updates_header_status_tone() {
    let mut app = test_support::app("s1");
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.status_tone,
            styra_ui::chrome::StatusTone::Running
        );
    });
    app.push_event(AgentEvent::TurnCompleted {
        outcome: TurnOutcome::Completed,
        usage: TurnUsage::default(),
    });
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.status_tone,
            styra_ui::chrome::StatusTone::Idle
        );
    });
}

/// Completion is a property of the Session, not a fact the update stream
/// reports, so nothing else would put it in the header — the local status
/// set when the operator marks it done is what the operator reads back.
#[test]
fn header_names_completion_in_the_status() {
    let mut app = test_support::app("s1");
    for (reason, expected) in [
        (
            crate::activity::StopReason::Completed,
            "stopped · completed",
        ),
        (
            crate::activity::StopReason::Abandoned,
            "stopped · abandoned",
        ),
    ] {
        app.activity.status = crate::activity::Status::Stopped(reason);
        inspect(&app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            assert_eq!(e.list.chrome.status, expected);
        });
    }
}

/// Integration stops the agent itself, so the update stream only says the
/// process ended. The listing is what says the work landed, and the header
/// reads it from there.
#[test]
fn header_names_integration_once_the_agent_is_gone() {
    let mut app = test_support::app("s1");
    let mut interaction = interaction_with_checkout("s1", None);
    interaction.integration = Some(styra_protocol::Integration {
        branch: Some("main".into()),
        commit: "0123456789abcdef".into(),
    });
    interaction.completed = styra_protocol::CompletionState::Sealed;
    app.interactions.items.push(interaction);
    app.activity.status = crate::activity::Status::ended(Some(0), None);

    assert!(app.adopt_integration());
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(e.list.chrome.status, "integrated into main · 0123456");
    });
    assert!(!app.adopt_integration());
}

#[test]
fn integration_prompt_data_resolves_target_branch() {
    let mut app = test_support::app("s1");
    let mut interaction = interaction_with_checkout("s1", None);
    interaction.branched_from = Some(styra_protocol::BranchPoint {
        branch: Some("release".into()),
        commit: "0123456".into(),
    });
    app.interactions.items.push(interaction);
    app.integration_prompt = Some("Ship it".into());

    inspect(&app, |v| {
        assert_eq!(v.overlays.integration_message, Some("Ship it"));
        assert_eq!(v.overlays.integration_target, Some("release"));
    });
}

#[test]
fn a_new_interaction_header_names_the_source_branch() {
    let mut app = test_support::app("");
    app.checkout_from = Some("source-session".into());
    app.interactions.items.push(interaction_with_checkout(
        "source-session",
        Some(CheckoutState {
            branch: Some("styra/source-work".into()),
            worktree: "/state/worktrees/source-work".into(),
            repository: "/workspace".into(),
        }),
    ));
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.source_branch.as_deref(),
            Some("styra/source-work")
        );
    });
    // A source standing in the Workspace's own checkout lends nothing of
    // its own: the launch starts from the Workspace branch.
    app.workspace_branch = Some("main".into());
    app.interactions.items[0].checkout = Some(CheckoutState {
        branch: Some("main".into()),
        worktree: "/workspace".into(),
        repository: "/workspace".into(),
    });
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.source_branch.as_deref(),
            Some("main (Workspace)")
        );
    });
    // A chosen base branch is what the launch starts from instead.
    app.base_branch = Some("release".into());
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.source_branch.as_deref(),
            Some("release (base)")
        );
    });
    app.base_branch = None;
    app.checkout_from = None;
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert_eq!(
            e.list.chrome.source_branch.as_deref(),
            Some("main (Workspace)")
        );
    });
    app.workspace_branch = None;
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(e.list.chrome.source_branch.is_none());
    });
    app.session_id = "source-session".into();
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(e.list.chrome.source_branch.is_none());
    });
}

#[test]
fn entering_input_supplies_message_box_data() {
    let mut app = test_support::app("s1");
    app.enter_input();

    inspect(&app, |v| {
        assert!(v.overlays.input.is_some());
    });
}

/// A recording takes the message box's place rather than sitting beside
/// it: the box's keys belong to the capture while it runs, so leaving the
/// buffer on screen would offer typing that does not happen.
#[test]
fn a_running_recording_replaces_the_message_box() {
    let mut app = test_support::app("s1");
    app.enter_input();
    app.composer.insert("half a sentence");
    app.recording = Some(crate::audio::Recorded::for_test(0.4, 0.6, 2.0));

    inspect(&app, |v| {
        assert!(v.overlays.input.is_none());
        let recording = v.overlays.recording.expect("capture data");
        assert_eq!(
            (recording.level, recording.loudest, recording.gain),
            (0.4, 0.6, 2.0)
        );
    });
}

/// Creating a Git workspace blocks the loop, so while it runs the message
/// box is replaced by a notice saying so: a box left on screen would offer
/// typing that nothing reads.
#[test]
fn a_blocking_call_replaces_the_message_box_with_its_notice() {
    let mut app = test_support::app("");
    app.enter_input();
    app.composer.insert("start on the picker");
    app.busy = Some("creating a new Git workspace…".into());

    inspect(&app, |v| {
        assert_eq!(
            v.overlays.busy.as_ref().unwrap().message,
            "creating a new Git workspace…"
        );
    });
}

/// Every view's status line must name the model and effort in use, since
/// the agent name alone does not say it and the session may be spending a
/// model nobody typed.
#[test]
fn the_status_line_names_the_model_and_effort_in_use() {
    let mut app = test_support::app("s-1");
    app.push_event(AgentEvent::ThreadStarted {
        thread_id: "t-9".into(),
        model: Some(test_support::MODEL.into()),
        effort: Some(test_support::EFFORT.into()),
    });
    for view in [
        View::Events,
        View::Raw,
        View::Log,
        View::Transcript,
        View::Driva,
    ] {
        app.view = view;
        inspect(&app, |v| {
            let chrome = match &v.main {
                MainView::Events(e) => &e.list.chrome,
                MainView::Raw(r) => &r.chrome,
                MainView::Log { chrome, .. } => chrome,
                MainView::Transcript(t) => &t.chrome,
                MainView::Driva(d) => &d.chrome,
                _ => panic!("unexpected view"),
            };
            assert_eq!(chrome.model, test_support::MODEL);
            assert_eq!(chrome.effort.as_deref(), Some(test_support::EFFORT));
        });
    }
}

#[test]
fn checkout_diff_layout_feedback_reaches_the_host() {
    let mut app = test_support::app("s1");
    let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
        worktree: "/no-such-styra-diff-checkout".into(),
        branch: None,
        base_branch: None,
        base: "HEAD".into(),
        branch_point: false,
    });
    diff.diff = Ok("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".into());
    diff.side_by_side = true;
    app.checkout_diff = Some(diff);
    app.diff_preview = true;
    for split in [true, false, true] {
        let feedback = styra_ui::RenderFeedback {
            diff_layout: Some((split, 7)),
            ..Default::default()
        };
        apply_feedback(&mut app, &feedback);
        let diff = app.checkout_diff.as_ref().unwrap();
        assert_eq!((diff.rendered_side, diff.horizontal_scroll), (split, 7));
        assert!(diff.side_by_side);
    }
}

/// The files sit in a column left of the patch until ctrl-f moves them
/// under it.
#[test]
fn ctrl_f_moves_the_diff_file_list_below_the_patch_and_back() {
    let mut app = test_support::app("s1");
    app.checkout_diff = Some(crate::checkout_diff::read(styra_diff::DiffTarget {
        worktree: "/no-such-styra-diff-checkout".into(),
        branch: None,
        base_branch: None,
        base: "HEAD".into(),
        branch_point: false,
    }));
    app.diff_preview = true;
    let below = |app: &App| shown_diff(app).unwrap().files_below;
    assert!(
        shown_diff(&app).unwrap().per_file,
        "a fresh diff opens on its file list"
    );
    assert!(!below(&app));
    let f = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('f'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    crate::input::handle_diff_key(&mut app, f);
    assert!(below(&app));
    app.view = View::Files;
    assert!(below(&app), "the choice holds in the files view");
    crate::input::handle_diff_key(&mut app, f);
    assert!(!below(&app));
}

/// `D` gives the diff the whole view and takes the keys from the log it
/// hides; Tab cannot hand them back until `D` returns the log.
#[test]
fn d_maximizes_the_diff_pane_and_back() {
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = test_support::app("s1");
    app.checkout_diff = Some(crate::checkout_diff::read(styra_diff::DiffTarget {
        worktree: "/no-such-styra-diff-checkout".into(),
        branch: None,
        base_branch: None,
        base: "HEAD".into(),
        branch_point: false,
    }));
    app.diff_preview = true;
    let maximized = |app: &App| {
        let mut shown = None;
        inspect(app, |v| {
            let MainView::Events(e) = &v.main else {
                panic!()
            };
            shown = Some(e.diff_maximized);
        });
        shown.unwrap()
    };
    assert!(!maximized(&app));
    crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Char('D')));
    assert!(maximized(&app));
    assert!(app.diff_focused());
    crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Tab));
    assert!(app.diff_focused(), "no hidden log to hand the keys to");
    crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Char('D')));
    assert!(!maximized(&app));
    crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Tab));
    assert!(!app.diff_focused());
}

#[test]
fn opening_a_side_diff_report_keeps_the_events_view() {
    let mut app = test_support::app("s1");
    let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
        worktree: "/no-such-styra-diff-checkout".into(),
        branch: None,
        base_branch: None,
        base: "HEAD".into(),
        branch_point: false,
    });
    diff.diff = Ok("diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old\n+new\n".into());
    diff.report.report = Some(styra_diff::Report::default());
    app.checkout_diff = Some(diff);
    app.diff_preview = true;
    crate::input::handle_diff_key(
        &mut app,
        crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('o')),
    );
    assert_eq!(app.view, View::Events, "the diff stays in its pane");
    assert!(app.checkout_diff.as_ref().unwrap().report_page.is_some());
    inspect(&app, |v| {
        let MainView::Events(e) = &v.main else {
            panic!()
        };
        assert!(matches!(
            e.preview,
            Some(styra_ui::application::SidePreview::CheckoutDiff(_))
        ));
    });
}

#[test]
fn checkout_diff_is_dispatched_to_the_side_views() {
    let mut app = test_support::app("s1");
    app.checkout_diff = Some(styra_diff::CheckoutDiff {
        source: styra_diff::Source::Checkout(styra_diff::DiffTarget {
            worktree: "/state/worktrees/styra-7".into(),
            branch: Some("styra/rename".into()),
            base_branch: Some("main".into()),
            base: "4bf5c35d".into(),
            branch_point: true,
        }),
        diff: Ok(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
        ),
        ..Default::default()
    });
    for view in [View::Events, View::Files] {
        app.view = view;
        app.diff_preview = true;
        assert!(panel_chrome(&app, Some("diff")).focused);
        inspect(&app, |v| {
            let preview = match &v.main {
                MainView::Events(e) => e.preview,
                MainView::Files(f) => f.preview,
                _ => panic!(),
            };
            assert!(matches!(
                preview,
                Some(styra_ui::application::SidePreview::CheckoutDiff(_))
            ));
        });
        app.diff_interaction_focus = true;
        assert!(!panel_chrome(&app, Some("diff")).focused);
        assert!(panel_chrome(&app, None).focused);
        app.diff_interaction_focus = false;
        app.enter_input();
        assert!(!panel_chrome(&app, Some("diff")).focused);
        app.enter_list();
    }
}

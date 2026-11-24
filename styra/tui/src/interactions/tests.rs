use super::*;
use std::path::PathBuf;
use styra_protocol::{CompletionState, InteractionActivity};

pub(crate) fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
    InteractionSummary {
        auto_retry: false,
        auto_commit: false,
        id: id.into(),
        name: None,
        tags: Vec::new(),
        workspace_id: "workspace".into(),
        selection: styra_protocol::agent::Selection::parse("codex").unwrap(),
        workspace: PathBuf::from("/workspace"),
        activity,
        activity_reason: None,
        activity_since_ms: 0,
        idle_unseen: false,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: None,
        last_message: None,
        recent_messages: Vec::new(),
        events: 0,
        branched_from: None,
        completed: CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

/// An interaction the operator finished with: stopped, and marked
/// complete on the Session it serves.
fn completed(id: &str) -> InteractionSummary {
    let mut interaction = interaction(id, InteractionActivity::Stopped);
    interaction.completed = CompletionState::Completed;
    interaction
}

/// The ids of [`LiveInteractions::items`], in item order.
fn item_ids(live: &LiveInteractions) -> Vec<&str> {
    live.items.iter().map(|item| item.id.as_str()).collect()
}

/// The ids in the order the navigator draws them.
fn shown<'a>(live: &'a LiveInteractions, workspace_id: Option<&str>) -> Vec<&'a str> {
    live.display_indices("", workspace_id)
        .into_iter()
        .map(|index| live.items[index].id.as_str())
        .collect()
}

/// j/k walk the rows exactly as they are drawn: from each row, `next`
/// lands on the one below it and `previous` on the one above.
fn assert_walk_follows_display(live: &LiveInteractions, workspace_id: Option<&str>) {
    for pair in shown(live, workspace_id).windows(2) {
        assert_eq!(live.next(pair[0], "", workspace_id).unwrap().id, pair[1]);
        assert_eq!(
            live.previous(pair[1], "", workspace_id).unwrap().id,
            pair[0]
        );
    }
}

#[test]
fn paging_follows_filtered_display_order_and_stops_at_the_ends() {
    let mut live = LiveInteractions::default();
    live.open(
        (0..50)
            .map(|index| {
                let mut item =
                    interaction(&format!("item-{index:02}"), InteractionActivity::Pending);
                if index % 2 == 0 {
                    item.tags.push("keep".into());
                }
                item
            })
            .collect(),
        vec![],
    );
    live.set_tag_filter(vec!["keep".into()], "item-00", None);

    live.cursor_page("item-00", None, true);
    assert_eq!(live.cursor("item-00"), "item-20");
    assert!(live.pending("item-00").is_some());
    assert!(live.due("item-00").is_none());
    live.cursor_page("item-00", None, true);
    assert_eq!(live.cursor("item-00"), "item-40");
    for _ in 0..3 {
        live.cursor_page("item-00", None, true);
        assert_eq!(live.cursor("item-00"), "item-48");
    }
    live.cursor_page("item-00", None, false);
    assert_eq!(live.cursor("item-00"), "item-28");
    for _ in 0..4 {
        live.cursor_page("item-00", None, false);
    }
    assert_eq!(live.cursor("item-00"), "item-00");
    assert!(live.pending("item-00").is_none());
}

#[test]
fn paging_handles_an_absent_cursor_and_an_empty_list() {
    let mut live = LiveInteractions::default();
    for down in [true, false] {
        live.cursor_page("missing", None, down);
        assert!(live.pending("missing").is_none());
        live.open(
            vec![interaction("only", InteractionActivity::Pending)],
            vec![],
        );
        live.cursor_page("missing", None, down);
        assert_eq!(live.cursor("missing"), "only");
        live.close();
        live = LiveInteractions::default();
    }
}

#[test]
fn live_interactions_open_newest_first_regardless_of_pause_state() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("0000000000200-1-0", InteractionActivity::Pending),
            interaction("0000000000300-1-0", InteractionActivity::Stopped),
            interaction("0000000000100-1-0", InteractionActivity::Running),
        ],
        vec![],
    );

    assert!(live.open);
    assert_eq!(
        item_ids(&live),
        [
            "0000000000300-1-0",
            "0000000000200-1-0",
            "0000000000100-1-0"
        ]
    );
    assert_eq!(
        live.current("0000000000100-1-0").unwrap().id,
        "0000000000100-1-0"
    );

    // A change of activity does not move a row.
    live.refresh(vec![
        interaction("0000000000100-1-0", InteractionActivity::Pending),
        interaction("0000000000200-1-0", InteractionActivity::Running),
        interaction("0000000000300-1-0", InteractionActivity::Stopped),
    ]);
    assert_eq!(
        item_ids(&live),
        [
            "0000000000300-1-0",
            "0000000000200-1-0",
            "0000000000100-1-0"
        ]
    );

    let mut items = live.items.clone();
    items[0].activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
    let mut newer_paused = interaction("0000000000400-1-0", InteractionActivity::Stopped);
    newer_paused.activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
    items.push(newer_paused);
    live.refresh(items);
    assert_eq!(
        item_ids(&live),
        [
            "0000000000400-1-0",
            "0000000000300-1-0",
            "0000000000200-1-0",
            "0000000000100-1-0"
        ]
    );
}

#[test]
fn live_interactions_sort_by_sent_messages_not_focus_or_agent_activity() {
    let mut older = interaction("0000000000100-old", InteractionActivity::Pending);
    older.last_user_message_at_ms = 500;
    let mut newer = interaction("0000000000200-new", InteractionActivity::Running);
    newer.last_user_message_at_ms = 400;
    newer.interacted_at_ms = 900;
    newer.activity_since_ms = 900;
    let unsent = interaction("0000000000300-unsent", InteractionActivity::Pending);
    let mut paused = interaction("0000000000400-paused", InteractionActivity::Stopped);
    paused.last_user_message_at_ms = 1000;
    paused.activity_reason = Some(styra_protocol::InteractionActivityReason::Paused);
    let mut live = LiveInteractions::default();
    live.open(vec![newer.clone(), unsent, paused, older], vec![]);
    assert_eq!(
        item_ids(&live),
        [
            "0000000000400-paused",
            "0000000000100-old",
            "0000000000200-new",
            "0000000000300-unsent"
        ]
    );

    // A new operator message moves the row on the next listing, even
    // though no other metadata (or agent response) has changed yet.
    newer.last_user_message_at_ms = 1100;
    let mut refreshed = live.items.clone();
    refreshed[2] = newer;
    assert!(live.refresh(refreshed));
    assert_eq!(
        item_ids(&live),
        [
            "0000000000200-new",
            "0000000000400-paused",
            "0000000000100-old",
            "0000000000300-unsent"
        ]
    );
}

#[test]
fn refreshing_live_interactions_keeps_current_lookup_by_identity() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Running),
        ],
        vec![],
    );
    let next = live.next("one", "", Some("workspace")).unwrap();
    assert_eq!(next.id, "two");
    let mut refreshed_two = interaction("two", InteractionActivity::Pending);
    refreshed_two.last_message = Some("new response".into());
    live.refresh(vec![
        refreshed_two,
        interaction("one", InteractionActivity::Running),
    ]);

    assert_eq!(live.current("two").unwrap().id, "two");
    assert_eq!(
        live.current("two").unwrap().last_message.as_deref(),
        Some("new response")
    );
}

#[test]
fn refreshed_names_have_one_lookup_for_the_list_and_current_screen() {
    let mut live = LiveInteractions::default();
    let mut original = interaction("one", InteractionActivity::Pending);
    original.name = Some("Original".into());
    live.open(vec![original], vec![]);

    assert_eq!(live.name("one"), Some("Original"));
    assert_eq!(live.current("one").unwrap().name, None);

    let mut renamed = interaction("one", InteractionActivity::Pending);
    renamed.name = Some("Renamed".into());
    assert!(live.refresh(vec![renamed]));

    assert_eq!(live.name("one"), Some("Renamed"));
    assert_eq!(live.current("one").unwrap().name, None);
}

#[test]
fn a_snapshot_seeds_another_listing_with_its_names() {
    let mut named = interaction("one", InteractionActivity::Pending);
    named.name = Some("Named".into());
    let mut live = LiveInteractions::default();
    assert!(!live.listed());
    live.refresh(vec![
        named,
        interaction("two", InteractionActivity::Running),
    ]);
    assert!(live.listed());

    let mut seeded = LiveInteractions::default();
    seeded.refresh(live.snapshot());

    assert_eq!(seeded.name("one"), Some("Named"));
    assert_eq!(seeded.name("two"), None);
    assert_eq!(seeded.items, live.items);
}

#[test]
fn completed_interactions_are_hidden_until_toggled() {
    let mut abandoned = interaction("abandoned", InteractionActivity::Stopped);
    abandoned.completed = CompletionState::Abandoned;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            completed("completed"),
            interaction("active", InteractionActivity::Pending),
            abandoned,
        ],
        vec![],
    );

    assert_eq!(
        live.visible_indices("", Some("workspace")),
        vec![1],
        "completed and abandoned rows are hidden by default"
    );
    live.toggle_completed();
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
}

/// Finished rows are left on the server until the operator asks to see
/// them, except the one on screen, which the screen still describes.
#[test]
fn the_listing_asks_for_finished_rows_only_when_they_are_shown() {
    let mut live = LiveInteractions::default();
    let query = live.query("current");
    assert!(query.skip_done);
    assert!(!query.skip_stopped, "hiding stopped rows is a local filter");
    assert_eq!(query.keep, vec!["current".to_owned()]);
    assert!(query.admits_summary(&completed("current")));
    assert!(!query.admits_summary(&completed("other")));

    assert!(
        live.query("").keep.is_empty(),
        "a blank screen keeps nothing"
    );

    live.toggle_completed();
    assert!(!live.query("current").skip_done);
}

#[test]
fn stopped_rows_can_be_hidden() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("stopped", InteractionActivity::Stopped),
            interaction("idle", InteractionActivity::Pending),
            completed("completed"),
        ],
        vec![],
    );
    live.show_completed = true;
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);

    live.toggle_stopped();

    assert_eq!(
        live.visible_indices("", Some("workspace")),
        vec![1],
        "stopped rows, completed ones included, are hidden"
    );
}

/// A server restart, or a branch nobody has sent to yet, leaves work
/// dormant rather than stopped: it stays in the live set — listed with
/// stopped rows hidden, tiled, counted as idle, landed on and stepped to —
/// while a pause still takes it out.
#[test]
fn dormant_rows_stay_live() {
    use styra_protocol::InteractionActivityReason;
    let stopped_for = |id: &str, reason| InteractionSummary {
        activity_reason: Some(reason),
        ..interaction(id, InteractionActivity::Stopped)
    };
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            stopped_for("restarted", InteractionActivityReason::ServerRestarted),
            stopped_for("branched", InteractionActivityReason::Branched),
            stopped_for("paused", InteractionActivityReason::Paused),
            interaction("idle", InteractionActivity::Pending),
        ],
        vec![],
    );
    live.toggle_stopped();
    let listed = |indices: Vec<usize>| {
        let mut ids = indices
            .into_iter()
            .map(|index| live.items[index].id.as_str())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };

    assert_eq!(
        listed(live.visible_indices("", Some("workspace"))),
        ["branched", "idle", "restarted"]
    );
    assert_eq!(
        listed(live.overview_indices(None)),
        ["branched", "idle", "restarted"]
    );
    let counts = live.activity_counts();
    assert_eq!((counts.idle, counts.stopped), (3, 1));
    assert!(is_live(&live.items[0]) && !is_live(&live.items[2]));
    assert_ne!(first_live(&live.items).unwrap().id, "paused");
    let mut stepped = std::collections::BTreeSet::new();
    let mut from = "idle".to_owned();
    for _ in 0..4 {
        from = live.next_live(&from, None).unwrap().id;
        stepped.insert(from.clone());
    }
    assert_eq!(
        stepped.into_iter().collect::<Vec<_>>(),
        ["branched", "idle", "restarted"]
    );
}

/// Integrating the Interaction on screen completes it; it stays listed,
/// with its branches nested under it, for as long as it is on screen.
#[test]
fn the_current_row_is_listed_whatever_its_state() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            completed("current"),
            interaction("stopped", InteractionActivity::Stopped),
            completed("other"),
        ],
        vec![],
    );
    assert_eq!(
        live.visible_indices("current", Some("workspace")),
        vec![0, 1]
    );

    live.toggle_stopped();
    assert_eq!(live.visible_indices("current", Some("workspace")), vec![0]);
    assert!(live.visible_indices("", Some("workspace")).is_empty());

    live.set_tag_filter(vec!["missing".into()], "current", Some("workspace"));
    assert!(
        live.visible_indices("current", Some("workspace"))
            .is_empty(),
        "the tag filter still narrows the list"
    );
}

#[test]
fn finishing_selects_visible_local_work_before_other_workspaces() {
    use InteractionActivity::{Pending, Running, Stopped};
    let elsewhere = || {
        let mut item = interaction("elsewhere", Pending);
        item.workspace_id = "other".into();
        item
    };
    for (name, items, show_completed, hide_stopped, scope, expected) in [
        (
            "hidden stopped sibling",
            vec![
                completed("finished"),
                interaction("stopped", Stopped),
                elsewhere(),
            ],
            true,
            true,
            None,
            "elsewhere",
        ),
        (
            "next visible row",
            vec![completed("finished"), interaction("next", Pending)],
            false,
            false,
            Some("workspace"),
            "next",
        ),
        (
            "stay on shown completed row without local work",
            vec![completed("finished"), elsewhere()],
            true,
            false,
            None,
            "finished",
        ),
        (
            "local live work before stopped or elsewhere",
            vec![
                interaction("earlier", Running),
                completed("finished"),
                elsewhere(),
                interaction("stopped", Stopped),
            ],
            false,
            false,
            None,
            "earlier",
        ),
        (
            "local stopped work before elsewhere",
            vec![
                completed("finished"),
                elsewhere(),
                interaction("stopped", Stopped),
            ],
            false,
            false,
            None,
            "stopped",
        ),
        (
            "leave workspace when no local work remains",
            vec![completed("finished"), elsewhere()],
            false,
            false,
            None,
            "elsewhere",
        ),
    ] {
        let count = items.len();
        let mut live = LiveInteractions::default();
        live.open(items, vec![]);
        live.show_completed = show_completed;
        live.hide_stopped = hide_stopped;
        assert_eq!(
            live.select_past_hidden("finished", scope).unwrap().id,
            expected,
            "{name}"
        );
        assert_eq!(
            live.items.len(),
            count,
            "{name}: the completed row is still listed"
        );
    }
}

/// Completion is a property of the Session, cleared by the server when it
/// is resumed, so an interaction started again arrives already not
/// completed — and the listing shows it without anything here having to
/// clear a mark.
#[test]
fn an_interaction_started_again_is_no_longer_completed() {
    let mut live = LiveInteractions::default();
    live.open(vec![completed("finished")], vec![]);
    assert!(live.visible_indices("", Some("workspace")).is_empty());

    live.refresh(vec![interaction("finished", InteractionActivity::Running)]);

    assert_eq!(live.visible_indices("", Some("workspace")), vec![0]);
}

#[test]
fn finishing_prefers_the_same_worktree_before_the_rest_of_the_workspace() {
    for finished_state in [CompletionState::Completed, CompletionState::Abandoned] {
        for show_completed in [false, true] {
            for sibling_before in [false, true] {
                let mut finished = completed("finished");
                finished.completed = finished_state;
                finished.workspace = PathBuf::from("/worktrees/shared");
                let mut sibling = interaction("sibling", InteractionActivity::Pending);
                sibling.workspace = finished.workspace.clone();
                let other_worktree = interaction("other-worktree", InteractionActivity::Running);
                let items = if sibling_before {
                    vec![sibling, finished, other_worktree]
                } else {
                    vec![finished, other_worktree, sibling]
                };
                let mut live = LiveInteractions::default();
                live.open(items, vec![]);
                live.show_completed = show_completed;

                let next = live.select_past_hidden("finished", None).unwrap();

                assert_eq!(next.id, "sibling");
            }
        }
    }
}

#[test]
fn finishing_skips_stopped_and_finished_worktree_siblings() {
    let mut finished = completed("finished");
    finished.workspace = PathBuf::from("/worktrees/shared");
    let mut stopped = interaction("stopped", InteractionActivity::Stopped);
    stopped.workspace = finished.workspace.clone();
    let mut done = interaction("done", InteractionActivity::Pending);
    done.workspace = finished.workspace.clone();
    done.completed = CompletionState::Completed;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            finished,
            stopped,
            done,
            interaction("next", InteractionActivity::Pending),
        ],
        vec![],
    );
    live.show_completed = true;

    assert_eq!(
        live.select_past_hidden("finished", None).unwrap().id,
        "next"
    );
}

/// Nearest is where the navigator draws the rows — branches beneath
/// their source — not the order they were created in.
#[test]
fn completing_an_interaction_moves_to_the_nearest_row_as_drawn() {
    let mut finished = branched("finished", InteractionActivity::Stopped, "source");
    finished.completed = CompletionState::Completed;
    let mut live = LiveInteractions::default();
    // Drawn as: source, finished, other, later; in creation order later
    // would be the row just after the finished one.
    live.open(
        vec![
            interaction("source", InteractionActivity::Pending),
            interaction("other", InteractionActivity::Pending),
            finished,
            interaction("later", InteractionActivity::Pending),
        ],
        vec![],
    );

    let next = live.select_past_hidden("finished", None).unwrap();

    assert_eq!(next.id, "other");
}

/// A focus request is answered when its interaction first arrives, and
/// only then: not for rows already running when the client started, and
/// not again on the refreshes after.
#[test]
fn a_focus_request_is_claimed_once_on_arrival() {
    let mut asking = interaction("asking", InteractionActivity::Running);
    asking.focus_requested = true;
    let mut live = LiveInteractions::default();
    live.refresh(vec![asking.clone()]);
    assert_eq!(live.take_focus_claim(), None, "it was already running");

    let mut arrived = interaction("arrived", InteractionActivity::Running);
    arrived.focus_requested = true;
    live.refresh(vec![
        asking.clone(),
        interaction("quiet", InteractionActivity::Running),
        arrived.clone(),
    ]);
    assert_eq!(
        live.take_focus_claim().map(|claim| claim.id),
        Some("arrived".into())
    );
    assert_eq!(live.take_focus_claim(), None);

    live.refresh(vec![asking, arrived]);
    assert_eq!(
        live.take_focus_claim(),
        None,
        "still listed is not arriving"
    );
}

/// The tally is of everything listed, seen or not, and counts background
/// work as running: it answers "what is the fleet doing", not "what is
/// waiting for me".
#[test]
fn the_activity_tally_covers_every_listed_interaction() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("working", InteractionActivity::Running),
            interaction("background", InteractionActivity::Background),
            interaction("waiting", InteractionActivity::Pending),
            interaction("done", InteractionActivity::Stopped),
            interaction("also-done", InteractionActivity::Stopped),
            // Completed work must not inflate the stopped count.
            completed("signed-off"),
        ],
        Vec::new(),
    );
    assert_eq!(
        live.activity_counts(),
        ActivityCounts {
            running: 2,
            idle: 1,
            stopped: 2,
        }
    );
}

/// The jump exists to answer the footer's count, so it walks the unseen
/// ones in display order and wraps: pressing it repeatedly visits each one
/// rather than stopping at the last of them.
#[test]
fn the_idle_jump_walks_every_unseen_interaction_and_wraps() {
    let mut unseen = interaction("unseen", InteractionActivity::Pending);
    unseen.idle_unseen = true;
    let mut later = interaction("later", InteractionActivity::Pending);
    later.idle_unseen = true;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            unseen,
            interaction("busy", InteractionActivity::Running),
            later,
        ],
        vec![],
    );

    assert_eq!(live.next_idle_unseen("current", None).unwrap().id, "unseen");
    assert_eq!(live.next_idle_unseen("unseen", None).unwrap().id, "later");
    assert_eq!(live.next_idle_unseen("later", None).unwrap().id, "unseen");
}

/// The step between running interactions walks every live one — waiting on
/// the operator or mid-turn alike — in display order, and wraps, so the
/// operator can go round the working set with one key.
#[test]
fn the_live_step_walks_every_running_interaction_and_wraps() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("waiting", InteractionActivity::Pending),
            interaction("background", InteractionActivity::Background),
            completed("finished"),
            interaction("stopped", InteractionActivity::Stopped),
        ],
        vec![],
    );

    // The step follows the list's order, wrapping at its end.
    assert_eq!(live.next_live("current", None).unwrap().id, "waiting");
    assert_eq!(live.next_live("waiting", None).unwrap().id, "background");
    assert_eq!(live.next_live("background", None).unwrap().id, "current");
    // Active navigation skips the idle row in the same fleet.
    assert_eq!(live.next_active("current", None).unwrap().id, "background");
    assert_eq!(live.next_active("background", None).unwrap().id, "current");
}

#[test]
fn the_n_destination_prioritizes_unseen_idle_work_before_live_work() {
    let mut idle = interaction("idle", InteractionActivity::Pending);
    idle.idle_unseen = true;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("next-live", InteractionActivity::Running),
            idle,
        ],
        vec![],
    );

    assert_eq!(
        live.next_attention_or_live("current", None).unwrap().id,
        "idle"
    );

    live.items
        .iter_mut()
        .find(|interaction| interaction.id == "idle")
        .unwrap()
        .idle_unseen = false;
    assert_eq!(
        live.next_attention_or_live("current", None).unwrap().id,
        "next-live"
    );
}

/// With nothing else actively working there is nowhere to step to: an
/// idle interaction is not a destination even though it is still live.
#[test]
fn the_active_step_has_nowhere_to_go_without_another_working_interaction() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("waiting", InteractionActivity::Pending),
            interaction("stopped", InteractionActivity::Stopped),
        ],
        vec![],
    );

    assert!(live.next_active("current", None).is_none());
}

/// With nothing else running there is nowhere to step to: the interaction
/// on screen is not a destination, and a stopped one is not either.
#[test]
fn the_live_step_has_nowhere_to_go_without_another_running_interaction() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("stopped", InteractionActivity::Stopped),
            completed("finished"),
        ],
        vec![],
    );

    assert!(live.next_live("current", None).is_none());
}

/// In Workspace scope the steps stay in the current Workspace: the
/// operator narrowed the list to it, so work elsewhere is not a
/// destination, and the scope is left as it was.
#[test]
fn steps_in_workspace_scope_stay_in_the_current_workspace() {
    let mut elsewhere = interaction("elsewhere", InteractionActivity::Running);
    elsewhere.workspace_id = "other-workspace".into();
    let mut unseen_elsewhere = interaction("unseen-elsewhere", InteractionActivity::Pending);
    unseen_elsewhere.workspace_id = "other-workspace".into();
    unseen_elsewhere.idle_unseen = true;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            elsewhere,
            unseen_elsewhere,
            interaction("here", InteractionActivity::Running),
        ],
        vec![],
    );
    live.toggle_workspace_scope();
    let scope = Some("workspace");

    assert_eq!(
        live.next_attention_or_live("current", scope).unwrap().id,
        "here"
    );
    assert_eq!(
        live.next_attention_or_live("here", scope).unwrap().id,
        "current"
    );
    assert_eq!(live.next_active("current", scope).unwrap().id, "here");
    assert_eq!(live.previous_active("current", scope).unwrap().id, "here");

    let next = live.cursor_to_next_active("current", scope).unwrap();
    assert_eq!(next.id, "here");
    assert!(live.only_current_workspace);
    // Moved like a j/k step, so the load waits for the cursor to rest.
    assert_eq!(live.cursor("current"), "here");
}

/// With the rest of the current Workspace stopped there is nowhere to
/// step in Workspace scope, though there is in All.
#[test]
fn steps_in_workspace_scope_do_not_leave_it_for_lack_of_work() {
    let mut elsewhere = interaction("elsewhere", InteractionActivity::Running);
    elsewhere.workspace_id = "other-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            elsewhere,
        ],
        vec![],
    );

    assert_eq!(
        live.next_live("current", Some("workspace")).unwrap().id,
        "elsewhere"
    );
    live.toggle_workspace_scope();
    assert!(live.next_live("current", Some("workspace")).is_none());
    assert!(live.next_active("current", Some("workspace")).is_none());
}

/// Presses quicker than the settle must keep walking the working set, so
/// each step starts from the cursor rather than the loaded Interaction.
#[test]
fn repeated_working_steps_walk_on_from_the_cursor() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("second", InteractionActivity::Running),
            interaction("third", InteractionActivity::Running),
        ],
        vec![],
    );
    let first = live.cursor_to_next_active("current", None).unwrap().id;
    let second = live.cursor_to_next_active("current", None).unwrap().id;

    assert_ne!(first, second);
    assert_eq!(live.cursor("current"), second);
}

/// The backwards step walks the working set in reverse, skipping idle
/// work just as the forward one does.
#[test]
fn the_backwards_working_step_reverses_the_forward_one() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("idle", InteractionActivity::Pending),
            interaction("second", InteractionActivity::Running),
            interaction("third", InteractionActivity::Running),
        ],
        vec![],
    );
    let forward = live.next_active("current", None).unwrap().id;
    let backward = live.previous_active("current", None).unwrap().id;
    assert_ne!(forward, backward);
    assert_eq!(live.previous_active(&forward, None).unwrap().id, "current");

    let back = live.cursor_to_previous_active("current", None).unwrap().id;
    assert_eq!(back, backward);
    assert_ne!(back, "idle");
    let again = live.cursor_to_previous_active("current", None).unwrap().id;
    assert_eq!(again, forward, "a second step walks on from the cursor");
}

/// An idle Interaction a client has already been shown is not what the
/// jump is for, and neither is one that stopped rather than went idle.
#[test]
fn the_idle_jump_has_nowhere_to_go_without_an_unseen_interaction() {
    let mut stopped = interaction("stopped", InteractionActivity::Stopped);
    stopped.idle_unseen = true;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Running),
            interaction("seen", InteractionActivity::Pending),
            stopped,
        ],
        vec![],
    );

    assert!(live.next_idle_unseen("current", None).is_none());
}

/// Loading an Interaction replaces the whole screen, so a cursor crossing
/// the list must not ask for one row's load per row it passes over.
#[test]
fn a_moving_cursor_defers_its_load_until_it_comes_to_rest() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Pending),
            interaction("three", InteractionActivity::Pending),
        ],
        vec![],
    );

    live.cursor_next("one", Some("workspace"));
    live.cursor_next("one", Some("workspace"));

    // Two rows crossed, and neither of the interactions passed over — nor
    // the one now under the cursor — has been asked for yet.
    assert_eq!(live.cursor("one"), "three");
    assert_eq!(
        live.pending("one")
            .map(|interaction| interaction.id.as_str()),
        Some("three")
    );
    assert!(live.due("one").is_none());

    std::thread::sleep(LOAD_SETTLE + Duration::from_millis(20));
    assert_eq!(
        live.due("one").map(|interaction| interaction.id.as_str()),
        Some("three")
    );

    // Once the load has landed the cursor is home again, so the settled
    // move is not made a second time.
    live.rest();
    assert_eq!(live.cursor("three"), "three");
    assert!(live.due("three").is_none());
}

#[test]
fn cursor_endpoints_follow_visible_rows_and_defer_loading() {
    let mut live = LiveInteractions::default();
    let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
    elsewhere.workspace_id = "other-workspace".into();
    live.open(
        vec![
            completed("finished"),
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Pending),
            interaction("three", InteractionActivity::Pending),
            elsewhere,
        ],
        vec![],
    );
    live.toggle_workspace_scope();
    let scope = Some("workspace");

    live.cursor_last("one", scope);
    assert_eq!(live.cursor("one"), "three");
    assert!(live.pending("one").is_some());
    assert!(live.due("one").is_none());

    live.cursor_first("one", scope);
    assert_eq!(live.cursor("one"), "one");
    assert!(live.pending("one").is_none());

    live.items[1].tags = vec!["match".into()];
    live.items[2].tags = vec!["match".into()];
    live.set_tag_filter(vec!["match".into()], "three", scope);
    live.cursor_last("three", scope);
    assert_eq!(live.cursor("three"), "two");
    live.cursor_first("three", scope);
    assert_eq!(live.cursor("three"), "one");

    live.set_tag_filter(vec!["missing".into()], "three", scope);
    live.rest();
    live.cursor_first("three", scope);
    live.cursor_last("three", scope);
    assert_eq!(live.cursor("three"), "three");
    assert!(live.pending("three").is_none());
}

/// The arrows move the view and leave the cursor, and nothing is due to
/// load; the next cursor move takes the view back to the cursor.
#[test]
fn scrolling_the_view_leaves_the_cursor_until_it_moves() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Pending),
        ],
        vec![],
    );

    live.scroll_view_down();
    assert_eq!(live.scroll_offset, Some(10));
    assert!(live.scrolled_away);
    live.scroll_view_up();
    live.scroll_view_up();
    assert_eq!(live.scroll_offset, Some(0), "stops at the top");
    assert_eq!(live.cursor("one"), "one");
    assert!(live.pending("one").is_none());

    live.cursor_next("one", None);
    assert!(!live.scrolled_away);
    assert_eq!(live.cursor("one"), "two");
}

#[test]
fn a_cursor_walked_back_onto_the_current_interaction_loads_nothing() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Pending),
        ],
        vec![],
    );

    live.cursor_next("one", Some("workspace"));
    live.cursor_previous("one", Some("workspace"));

    assert_eq!(live.cursor("one"), "one");
    assert!(live.pending("one").is_none());
    assert!(live.due("one").is_none());
}

#[test]
fn a_cursored_interaction_that_leaves_the_list_releases_the_cursor() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("one", InteractionActivity::Pending),
            interaction("two", InteractionActivity::Pending),
        ],
        vec![],
    );
    live.cursor_previous("one", Some("workspace"));

    live.refresh(vec![interaction("one", InteractionActivity::Pending)]);

    assert_eq!(live.cursor("one"), "one");
    assert!(live.pending("one").is_none());
}

#[test]
fn workspace_scope_filters_navigation_and_can_return_to_all() {
    let mut other = interaction("other", InteractionActivity::Pending);
    other.workspace_id = "other-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("current", InteractionActivity::Pending),
            other,
            interaction("next", InteractionActivity::Running),
        ],
        vec![],
    );

    live.toggle_workspace_scope();
    assert!(live.only_current_workspace);
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 2]);
    assert_eq!(
        live.next("current", "", Some("workspace")).unwrap().id,
        "next"
    );

    live.toggle_workspace_scope();
    assert!(!live.only_current_workspace);
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
}

/// Interactions are grouped by Workspace only, not by the checkout they
/// work in, so j/k walks them in item order within each Workspace, in
/// either scope.
#[test]
fn navigation_follows_the_workspace_grouped_display_order() {
    let in_worktree = |id: &str| {
        let mut interaction = interaction(id, InteractionActivity::Pending);
        interaction.workspace = PathBuf::from("/worktrees/feature");
        interaction.checkout = Some(styra_protocol::CheckoutState {
            worktree: PathBuf::from("/worktrees/feature"),
            repository: PathBuf::from("/workspace"),
            branch: Some("feature".into()),
        });
        interaction
    };
    let mut other = interaction("other", InteractionActivity::Pending);
    other.workspace_id = "other-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            in_worktree("feature-1"),
            interaction("main-1", InteractionActivity::Pending),
            other,
            in_worktree("feature-2"),
            interaction("main-2", InteractionActivity::Pending),
        ],
        vec![],
    );
    let workspace = Some("workspace");
    assert_eq!(
        shown(&live, workspace),
        ["feature-1", "main-1", "feature-2", "main-2", "other"]
    );
    assert_walk_follows_display(&live, workspace);

    live.toggle_workspace_scope();
    assert_eq!(
        shown(&live, workspace),
        ["feature-1", "main-1", "feature-2", "main-2"]
    );
    assert_walk_follows_display(&live, workspace);
}

pub(crate) fn branched(id: &str, activity: InteractionActivity, from: &str) -> InteractionSummary {
    let mut interaction = interaction(id, activity);
    interaction.origin = Some(styra_protocol::SessionOrigin {
        session_id: from.into(),
        provider: styra_protocol::agent::Provider::Codex,
        at_ms: None,
        history: Default::default(),
    });
    interaction
}

fn display<'a>(live: &'a LiveInteractions, workspace_id: Option<&str>) -> Vec<(&'a str, usize)> {
    live.display_rows("", workspace_id)
        .into_iter()
        .map(|row| (live.items[row.index].id.as_str(), row.depth))
        .collect()
}

/// A branch sits beneath the interaction it came from, as it does in the
/// Session picker, rather than wherever its creation would sort it — a
/// fresh branch is the newest, and would otherwise rise to the head of the
/// list, away from its source.
#[test]
fn a_branch_is_listed_beneath_its_source() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            branched(
                "0000000000400-twig",
                InteractionActivity::Pending,
                "0000000000300-branch",
            ),
            interaction("0000000000200-other", InteractionActivity::Running),
            branched(
                "0000000000300-branch",
                InteractionActivity::Stopped,
                "0000000000100-source",
            ),
            interaction("0000000000100-source", InteractionActivity::Running),
        ],
        vec![],
    );

    for _ in ["all", "workspace scope"] {
        assert_eq!(
            display(&live, Some("workspace")),
            [
                ("0000000000200-other", 0),
                ("0000000000100-source", 0),
                ("0000000000300-branch", 1),
                ("0000000000400-twig", 2),
            ]
        );
        // Walking the list follows the tree the navigator draws.
        assert_walk_follows_display(&live, Some("workspace"));
        live.toggle_workspace_scope();
    }
}

/// Only a direct line of branches counts: a source's source, a branch's
/// branch, but not a sibling or a cousin.
#[test]
fn descent_follows_only_the_direct_line() {
    use InteractionActivity::Pending;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("root", Pending),
            branched("child", Pending, "root"),
            branched("grandchild", Pending, "child"),
            branched("sibling", Pending, "root"),
            branched("cousin", Pending, "sibling"),
        ],
        vec![],
    );
    assert!(live.descends_from("child", "root"));
    assert!(live.descends_from("grandchild", "root"));
    assert!(!live.descends_from("root", "child"));
    assert!(!live.descends_from("sibling", "child"));
    assert!(!live.descends_from("cousin", "child"));
    assert!(!live.descends_from("child", "child"));
}

/// A branch whose source is not shown has nothing to hang from, so it
/// stands at the root rather than disappearing with its source.
#[test]
fn a_branch_of_a_hidden_source_stands_at_the_root() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            completed("source"),
            branched("branch", InteractionActivity::Stopped, "source"),
            branched("orphan", InteractionActivity::Pending, "closed"),
        ],
        vec![],
    );

    assert_eq!(
        display(&live, Some("workspace")),
        [("branch", 0), ("orphan", 0)]
    );
    live.toggle_completed();
    assert_eq!(
        display(&live, Some("workspace")),
        [("source", 0), ("branch", 1), ("orphan", 0)]
    );
}

/// `interaction` working in its own worktree on `branch`, made from `into`.
fn worktree(mut interaction: InteractionSummary, branch: &str, into: &str) -> InteractionSummary {
    let path = PathBuf::from(format!("/worktrees/{branch}"));
    interaction.workspace = path.clone();
    interaction.checkout = Some(styra_protocol::CheckoutState {
        worktree: path,
        repository: PathBuf::from("/workspace"),
        branch: Some(branch.into()),
    });
    interaction.branched_from = Some(styra_protocol::BranchPoint {
        branch: Some(into.into()),
        commit: "0".into(),
    });
    interaction
}

/// A row as `(id, depth, named source)`.
type Placement<'a> = (&'a str, usize, Option<String>);

fn placements(live: &LiveInteractions) -> Vec<Placement<'_>> {
    live.display_rows("", Some("workspace"))
        .into_iter()
        .map(|row| {
            (
                live.items[row.index].id.as_str(),
                row.depth,
                row.branched_from,
            )
        })
        .collect()
}

/// Worktrees do not group the list: entries keep the item order, and a
/// branch hangs beneath its source whichever checkout either works in.
#[test]
fn branches_nest_by_source_whatever_their_checkout() {
    use InteractionActivity::Pending;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("tidy", Pending),
            // Branched, in the same (main) checkout.
            branched("tidy-more", Pending, "tidy"),
            worktree(interaction("rework", Pending), "auth", "main"),
            // Branched, in the same worktree.
            worktree(branched("jwt", Pending, "rework"), "auth", "main"),
            // Not branched, in the same worktree.
            worktree(interaction("review", Pending), "auth", "main"),
            // Not branched, two side by side in a worktree made from
            // that one.
            worktree(interaction("tests", Pending), "tests", "auth"),
            worktree(interaction("fixtures", Pending), "tests", "auth"),
            // Branched, into a worktree made from the source's.
            worktree(branched("spike", Pending, "rework"), "spike", "auth"),
            // Branched, into an unrelated worktree.
            worktree(branched("docs", Pending, "rework"), "docs", "main"),
        ],
        vec![],
    );
    assert_eq!(
        placements(&live),
        [
            ("tidy", 0, None),
            ("tidy-more", 1, None),
            ("rework", 0, None),
            ("jwt", 1, None),
            ("spike", 1, None),
            ("docs", 1, None),
            ("review", 0, None),
            ("tests", 0, None),
            ("fixtures", 0, None),
        ]
    );
    assert_walk_follows_display(&live, Some("workspace"));
}

/// `W` walks the cursor's worktree in display order, wrapping, past rows
/// in other checkouts; from the main checkout or a worktree of its own
/// it stays put.
#[test]
fn the_worktree_jump_cycles_through_the_cursors_worktree() {
    use InteractionActivity::Pending;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("tidy", Pending),
            branched("tidy-more", Pending, "tidy"),
            worktree(interaction("rework", Pending), "auth", "main"),
            worktree(branched("jwt", Pending, "rework"), "auth", "main"),
            worktree(branched("spike", Pending, "rework"), "spike", "auth"),
            worktree(interaction("review", Pending), "auth", "main"),
        ],
        vec![],
    );
    let scope = Some("workspace");
    let mut walk = Vec::new();
    for _ in 0..4 {
        let landed = live.cursor_to_next_in_worktree("rework", scope).unwrap();
        walk.push(landed.id);
    }
    assert_eq!(walk, ["jwt", "review", "rework", "jwt"]);

    live.rest();
    assert!(live.cursor_to_next_in_worktree("tidy", scope).is_none());
    assert!(live.cursor_to_next_in_worktree("spike", scope).is_none());
    assert_eq!(live.cursor("spike"), "spike");
}

/// A branch working in a worktree of its own hangs beneath the entry it
/// came from, after that entry's earlier branches, rather than sorting
/// wherever its own place in the item order would put it.
#[test]
fn a_branch_in_another_worktree_follows_its_source() {
    use InteractionActivity::Pending;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("newest", Pending),
            worktree(branched("twig", Pending, "spike"), "twig", "main"),
            worktree(branched("spike", Pending, "source"), "spike", "main"),
            branched("nested", Pending, "source"),
            interaction("source", Pending),
            interaction("oldest", Pending),
        ],
        vec![],
    );
    assert_eq!(
        placements(&live),
        [
            ("newest", 0, None),
            ("source", 0, None),
            ("spike", 1, None),
            ("twig", 2, None),
            ("nested", 1, None),
            ("oldest", 0, None),
        ]
    );
    assert_walk_follows_display(&live, Some("workspace"));
}

/// Entries working in one worktree are not gathered together: each keeps
/// its own place in the item order.
#[test]
fn entries_sharing_a_worktree_keep_their_own_places() {
    use InteractionActivity::Pending;
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            worktree(interaction("rework", Pending), "auth", "main"),
            interaction("tidy", Pending),
            worktree(interaction("review", Pending), "auth", "main"),
        ],
        vec![],
    );
    assert_eq!(
        placements(&live),
        [("rework", 0, None), ("tidy", 0, None), ("review", 0, None)]
    );
    assert_walk_follows_display(&live, Some("workspace"));
}

/// A Workspace whose own folder is a linked worktree of a clone elsewhere
/// is still the Workspace's main checkout, so its rows do not name the
/// folder the Workspace heading already names.
#[test]
fn a_workspace_that_is_itself_a_linked_worktree_is_its_main_checkout() {
    use InteractionActivity::Pending;
    let mut review = interaction("review", Pending);
    review.checkout = Some(styra_protocol::CheckoutState {
        worktree: PathBuf::from("/workspace"),
        repository: PathBuf::from("/clone"),
        branch: Some("rdfstore-introduction".into()),
    });
    let workspaces = vec![crate::workspace::tests::workspace(
        "workspace",
        None,
        "/workspace",
    )];
    assert!(
        in_main_checkout(&review, &workspaces),
        "the Workspace folder is its main checkout"
    );
}

#[test]
fn row_navigation_without_a_selection_starts_at_the_top() {
    for backwards in [false, true] {
        let mut live = LiveInteractions::default();
        let mut elsewhere = interaction("elsewhere", InteractionActivity::Pending);
        elsewhere.workspace_id = "other-workspace".into();
        live.open(
            vec![
                interaction("one", InteractionActivity::Pending),
                interaction("two", InteractionActivity::Pending),
                interaction("three", InteractionActivity::Pending),
                elsewhere,
            ],
            vec![],
        );
        live.toggle_workspace_scope();
        let scope = Some("workspace");

        // The current interaction can be absent or hidden by the scope.
        for current in ["missing", "elsewhere"] {
            live.rest();
            if backwards {
                live.cursor_previous(current, scope);
            } else {
                live.cursor_next(current, scope);
            }
            assert_eq!(live.cursor(current), "one");
            assert_eq!(live.pending(current).unwrap().id, "one");
            assert!(live.due(current).is_none());

            live.cursor_next(current, scope);
            assert_eq!(live.cursor(current), "two");
            live.cursor_previous(current, scope);
            assert_eq!(live.cursor(current), "one");
        }
    }

    let mut empty = LiveInteractions::default();
    empty.cursor_next("missing", None);
    empty.cursor_previous("missing", None);
    assert_eq!(empty.cursor("missing"), "missing");
    assert!(empty.pending("missing").is_none());
}

/// Each Workspace's entries are drawn together, in item order, even where
/// sorting interleaves the Workspaces — a stopped entry included: its row
/// says that it stopped, so it needs no place of its own.
#[test]
fn entries_are_grouped_by_workspace_in_item_order() {
    let mut other_stopped = interaction("other-stopped", InteractionActivity::Stopped);
    other_stopped.workspace_id = "other-workspace".into();
    let mut other_running = interaction("other-running", InteractionActivity::Running);
    other_running.workspace_id = "other-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("pending", InteractionActivity::Pending),
            other_stopped,
            interaction("stopped", InteractionActivity::Stopped),
            other_running,
        ],
        vec![],
    );
    let workspace = Some("workspace");

    assert_eq!(
        shown(&live, workspace),
        ["pending", "stopped", "other-stopped", "other-running"]
    );
    assert_walk_follows_display(&live, workspace);

    // Workspace scope lists only this Workspace's entries.
    live.toggle_workspace_scope();
    assert_eq!(shown(&live, workspace), ["pending", "stopped"]);
    assert_walk_follows_display(&live, workspace);
}

#[test]
fn workspace_jumps_walk_the_first_entry_of_each_group() {
    let mut other_pending = interaction("other-pending", InteractionActivity::Pending);
    other_pending.workspace_id = "other-workspace".into();
    let mut other_running = interaction("other-running", InteractionActivity::Running);
    other_running.workspace_id = "other-workspace".into();
    let mut third = interaction("third", InteractionActivity::Running);
    third.workspace_id = "third-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("pending", InteractionActivity::Pending),
            other_pending,
            interaction("running", InteractionActivity::Running),
            other_running,
            third,
        ],
        vec![],
    );
    let workspace = Some("workspace");

    // Display order: pending, running | other-pending, other-running | third
    assert_eq!(
        live.next_workspace("pending", "", workspace).unwrap().id,
        "other-pending"
    );
    assert_eq!(
        live.next_workspace("running", "", workspace).unwrap().id,
        "other-pending"
    );
    assert_eq!(
        live.next_workspace("other-running", "", workspace)
            .unwrap()
            .id,
        "third"
    );
    assert!(live.next_workspace("third", "", workspace).is_none());

    // From the middle of a group, ctrl-k stops at that group's own first
    // entry before leaving it.
    assert_eq!(
        live.previous_workspace("other-running", "", workspace)
            .unwrap()
            .id,
        "other-pending"
    );
    assert_eq!(
        live.previous_workspace("other-pending", "", workspace)
            .unwrap()
            .id,
        "pending"
    );
    assert_eq!(
        live.previous_workspace("running", "", workspace)
            .unwrap()
            .id,
        "pending"
    );
    assert!(live.previous_workspace("pending", "", workspace).is_none());
    live.toggle_workspace_scope();
    assert!(live.next_workspace("pending", "", workspace).is_none());
    assert!(live.previous_workspace("running", "", workspace).is_none());
}

/// The group jump crosses more of the list per press than j/k, so it has
/// all the more reason to load only where the cursor stops.
#[test]
fn workspace_jumps_defer_their_load_like_the_row_moves_do() {
    let mut other = interaction("other-pending", InteractionActivity::Pending);
    other.workspace_id = "other-workspace".into();
    let mut third = interaction("third", InteractionActivity::Pending);
    third.workspace_id = "third-workspace".into();
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            interaction("pending", InteractionActivity::Pending),
            other,
            third,
        ],
        vec![],
    );
    let workspace = Some("workspace");

    live.cursor_next_workspace("pending", workspace);
    live.cursor_next_workspace("pending", workspace);

    assert_eq!(live.cursor("pending"), "third");
    assert!(live.due("pending").is_none());

    // Back the way it came, onto the interaction already on screen: two
    // groups crossed in each direction, and nothing to load at the end.
    live.cursor_previous_workspace("pending", workspace);
    live.cursor_previous_workspace("pending", workspace);
    assert_eq!(live.cursor("pending"), "pending");
    assert!(live.pending("pending").is_none());
}

/// The Workspace jump lands on the chosen Workspace's first entry, and
/// widens a Workspace scope that would have hidden it.
#[test]
fn the_workspace_jump_lands_on_the_workspaces_first_entry() {
    let mut first = interaction("first", InteractionActivity::Running);
    first.workspace_id = "other-workspace".into();
    let mut second = interaction("second", InteractionActivity::Pending);
    second.workspace_id = "other-workspace".into();
    let mut live = LiveInteractions {
        only_current_workspace: true,
        ..Default::default()
    };
    live.open(
        vec![
            interaction("current", InteractionActivity::Pending),
            first,
            second,
        ],
        vec![],
    );

    assert!(live.cursor_to_workspace("other-workspace", "current", Some("workspace")));
    assert!(!live.only_current_workspace);
    assert_eq!(live.cursor("current"), "first");
    assert!(!live.cursor_to_workspace("missing", "current", Some("workspace")));
}

#[test]
fn first_live_selection_respects_scope_recency_and_waiting_work() {
    use InteractionActivity::{Pending, Running, Stopped};
    let mut other = interaction("other", Pending);
    other.workspace_id = "other-workspace".into();
    let mut idle = interaction("idle", Pending);
    idle.interacted_at_ms = 10;
    let mut running = interaction("running", Running);
    running.interacted_at_ms = 20;
    let mut stopped = interaction("stopped", Stopped);
    stopped.interacted_at_ms = 30;
    let mut elsewhere = interaction("elsewhere", Pending);
    elsewhere.workspace_id = "other-workspace".into();
    for (name, items, expectations) in [
        (
            "waiting operator",
            vec![
                other,
                interaction("stopped", Stopped),
                interaction("running", Running),
                interaction("idle", Pending),
            ],
            vec![
                (Some("workspace"), Some("idle")),
                (Some("other-workspace"), Some("other")),
            ],
        ),
        (
            "last visited live entry",
            vec![idle, running, stopped, interaction("untouched", Pending)],
            vec![(Some("workspace"), Some("running"))],
        ),
        (
            "all stopped",
            vec![
                interaction("stopped", Stopped),
                interaction("also-stopped", Stopped),
            ],
            vec![
                (Some("workspace"), None),
                (Some("unknown"), None),
                (None, None),
            ],
        ),
        (
            "across workspaces",
            vec![
                interaction("stopped", Stopped),
                interaction("running", Running),
                elsewhere,
            ],
            vec![(None, Some("elsewhere"))],
        ),
    ] {
        for (scope, expected) in expectations {
            let chosen = match scope {
                Some(workspace) => first_live_in_workspace(&items, workspace),
                None => first_live(&items),
            };
            assert_eq!(
                chosen.as_ref().map(|item| item.id.as_str()),
                expected,
                "{name}: {scope:?}"
            );
        }
    }
}

#[test]
fn completed_work_is_not_a_live_workspace_entry_even_when_its_agent_accepts() {
    for completed in [CompletionState::Completed, CompletionState::Abandoned] {
        let mut finished = interaction("finished", InteractionActivity::Pending);
        finished.completed = completed;
        finished.interacted_at_ms = 100;
        let unfinished = interaction("unfinished", InteractionActivity::Running);
        let items = [finished.clone(), unfinished];

        assert_eq!(
            first_live_in_workspace(&items, "workspace").unwrap().id,
            "unfinished"
        );
        assert_eq!(first_live(&items).unwrap().id, "unfinished");
        assert!(first_live_in_workspace(&[finished.clone()], "workspace").is_none());
        assert!(first_live(&[finished]).is_none());
    }
}

fn named(id: &str, name: &str) -> InteractionSummary {
    let mut interaction = interaction(id, InteractionActivity::Pending);
    interaction.name = Some(name.into());
    interaction
}

#[test]
fn the_filter_narrows_the_list_by_name_or_tag_case_insensitively() {
    let mut tagged = named("three", "refactor");
    tagged.tags = vec!["Billing".into()];
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            named("one", "Fix checkout"),
            named("two", "write docs"),
            tagged,
        ],
        vec![],
    );

    live.start_filter();
    assert!(live.typing_filter());
    for character in "CHECK".chars() {
        live.type_filter(Edit::Push(character), "one", Some("workspace"));
    }
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0]);

    live.clear_filter();
    live.start_filter();
    for character in "bill".chars() {
        live.type_filter(Edit::Push(character), "one", Some("workspace"));
    }
    assert_eq!(live.visible_indices("", Some("workspace")), vec![2]);

    // Enter keeps the filter but hands the keys back to the list.
    live.finish_filter();
    assert!(!live.typing_filter());
    assert_eq!(live.filter(), Some("bill"));

    live.clear_filter();
    assert_eq!(live.visible_indices("", Some("workspace")), vec![0, 1, 2]);
}

/// A cursor left on a row the filter hid would act on something the
/// operator can no longer see, so it moves to the first match.
#[test]
fn the_cursor_follows_the_filter_onto_the_first_match() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![
            named("one", "Fix checkout"),
            named("two", "write docs"),
            named("three", "more docs"),
        ],
        vec![],
    );

    live.start_filter();
    for character in "docs".chars() {
        live.type_filter(Edit::Push(character), "one", Some("workspace"));
    }
    assert_eq!(live.cursor("one"), "two");

    // Still matching as the term changes, so it stays put.
    live.cursor_next("one", Some("workspace"));
    live.type_filter(Edit::Backspace, "one", Some("workspace"));
    assert_eq!(live.cursor("one"), "three");
}

#[test]
fn the_tag_filter_lists_rows_carrying_every_chosen_tag_and_outlasts_reopening() {
    let tagged = |id: &str, tags: &[&str]| {
        let mut interaction = named(id, id);
        interaction.tags = tags.iter().map(|tag| tag.to_string()).collect();
        interaction
    };
    let items = vec![
        tagged("one", &["bug"]),
        tagged("two", &["bug", "urgent"]),
        tagged("three", &["urgent"]),
    ];
    let mut live = LiveInteractions::default();
    live.open(items.clone(), vec![]);

    live.set_tag_filter(vec!["bug".into(), "urgent".into()], "one", None);
    assert_eq!(live.visible_indices("", None), vec![1]);
    assert_eq!(live.cursor("one"), "two");

    live.open(items, vec![]);
    assert_eq!(live.visible_indices("", None), vec![1]);

    live.set_tag_filter(Vec::new(), "one", None);
    assert_eq!(live.visible_indices("", None), vec![0, 1, 2]);
    live.set_tag_filter(vec!["bug".into(), "-urgent".into()], "one", None);
    assert_eq!(live.visible_indices("", None), vec![0]);
}

#[test]
fn live_tags_are_sorted_unique_and_only_come_from_the_current_snapshot() {
    let mut one = named("one", "one");
    one.tags = vec!["urgent".into(), "bug".into()];
    let mut two = named("two", "two");
    two.tags = vec!["bug".into(), "docs".into()];
    let mut live = LiveInteractions::default();
    live.open(vec![one, two], vec![]);

    assert_eq!(live.live_tags(), ["bug", "docs", "urgent"]);
}

#[test]
fn reopening_the_list_drops_the_filter() {
    let mut live = LiveInteractions::default();
    live.open(vec![named("one", "Fix checkout")], vec![]);
    live.start_filter();
    live.type_filter(Edit::Push('z'), "one", None);
    live.open(vec![named("one", "Fix checkout")], vec![]);
    assert!(live.filter().is_none());
    assert!(!live.typing_filter());
}

/// The fleet is listed several times a second and is usually exactly as it
/// was. The event loop paints a frame only when something changed, so a
/// snapshot identical to the last one must not read as news — otherwise
/// the poll alone would keep the screen repainting forever.
#[test]
fn a_snapshot_identical_to_the_last_one_is_not_a_change() {
    let mut live = LiveInteractions::default();
    live.open(
        vec![interaction("one", InteractionActivity::Running)],
        vec![],
    );

    assert!(
        !live.refresh(vec![interaction("one", InteractionActivity::Running)]),
        "the same fleet, listed again"
    );
    assert!(
        live.refresh(vec![interaction("one", InteractionActivity::Stopped)]),
        "that interaction stopped"
    );
    assert!(
        live.refresh(vec![
            interaction("one", InteractionActivity::Stopped),
            interaction("two", InteractionActivity::Running),
        ]),
        "another interaction joined the fleet"
    );
}

#[test]
fn a_row_flashes_when_something_new_happens_on_it() {
    let mut live = LiveInteractions::default();
    let mut talking = interaction("talking", InteractionActivity::Running);
    live.refresh(vec![
        talking.clone(),
        interaction("quiet", InteractionActivity::Running),
    ]);
    assert!(
        !live.flashing("talking") && !live.flashing("quiet"),
        "the fleet as this client found it is not news"
    );

    talking.last_message = Some("done".into());
    talking.events = 5;
    let mut busy = interaction("quiet", InteractionActivity::Running);
    busy.events = 9;
    live.refresh(vec![
        talking.clone(),
        busy,
        interaction("arrived", InteractionActivity::Running),
    ]);
    assert!(live.flashing("talking"), "its agent said something new");
    assert!(!live.flashing("quiet"), "events alone are not news");
    assert!(live.flashing("arrived"), "a new row is news");

    live.flashed.values_mut().for_each(|at| *at -= FLASH);
    assert!(live.expire_flashes());
    assert!(!live.flashing("talking"));
    assert!(!live.expire_flashes(), "nothing left to put out");

    talking.activity = InteractionActivity::Pending;
    live.refresh(vec![talking]);
    assert!(live.flashing("talking"), "it changed what it is doing");
}

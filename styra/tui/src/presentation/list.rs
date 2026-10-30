//! Mapping from the application timeline to the event-list presentation model.

use crate::activity::Status;
use crate::app::App;
use styra_ui::event_list::EntryVersion;
use styra_ui::markdown::LinkDisplay;

/// Hand the timeline's event identity to the renderer, which keys its cached
/// rows on it. Two types rather than one because the direction of the
/// dependency matters: `styra_ui` renders for any host and must not know what
/// a `Timeline` is, so it takes the pair opaquely — see
/// [`styra_ui::event_list::EntryVersion`].
pub(crate) fn ui_version(value: crate::timeline::EventVersion) -> EntryVersion {
    EntryVersion {
        id: value.id.as_u64(),
        revision: value.revision,
    }
}

pub(crate) fn ui_link_display(value: crate::app::LinkDisplay) -> LinkDisplay {
    match value {
        crate::app::LinkDisplay::Compact => LinkDisplay::Compact,
        crate::app::LinkDisplay::Full => LinkDisplay::Full,
    }
}

/// For a branch marker, the other session's current name, state, and latest
/// message, resolved live from the roster rather than trusted from the event
/// itself: the event's own `name` is a snapshot taken when the marker was
/// written, and a rename since would leave it stale. Empty both for an event
/// that is not a marker and for one whose target the roster has no row for.
pub(crate) fn branch_link<'a>(
    app: &'a App,
    event: &styra_protocol::event::AgentEvent,
) -> styra_ui::event_list::BranchLink<'a> {
    let Some(id) = event.branch_target() else {
        return Default::default();
    };
    let interaction = app.interactions.current(id);
    styra_ui::event_list::BranchLink {
        name: app.interactions.name(id),
        state: interaction.map(super::interactions::state),
        message: interaction.and_then(|interaction| interaction.last_message.as_deref()),
    }
}

pub(crate) fn branch_provider_switch(
    app: &App,
    event: &styra_protocol::event::AgentEvent,
) -> Option<(
    styra_protocol::agent::Provider,
    styra_protocol::agent::Provider,
)> {
    use styra_protocol::event::{AgentEvent, BranchDirection};
    match event {
        AgentEvent::Branched {
            direction: BranchDirection::From,
            session,
            ..
        } => app
            .interactions
            .provider_switch_from(session, &app.session_id),
        AgentEvent::Branched {
            direction: BranchDirection::To,
            session,
            ..
        } => app.interactions.provider_switch_to(session),
        _ => None,
    }
}

/// The status tail under the last entry, from where the interaction is and
/// how far along it is there.
pub(super) fn status(
    status: &Status,
    progress: crate::activity::Progress,
) -> styra_ui::event_list::EventListStatus {
    match status {
        Status::Pending => styra_ui::event_list::EventListStatus::Pending,
        Status::Running => styra_ui::event_list::EventListStatus::Running {
            elapsed: progress.in_status,
            quiet: progress.since_event,
            events: progress.events,
        },
        Status::Idle(why) => styra_ui::event_list::EventListStatus::Idle {
            reason: status.reason_label(),
            failed: matches!(why, crate::activity::IdleReason::Failed { .. }),
        },
        Status::Background => styra_ui::event_list::EventListStatus::Background {
            elapsed: progress.in_status,
        },
        Status::Stopped(why) => styra_ui::event_list::EventListStatus::Stopped {
            elapsed: progress.in_status,
            reason: status.reason_label(),
            tone: super::stop_tone(why),
        },
        Status::Ended { .. } => styra_ui::event_list::EventListStatus::Ended,
    }
}

pub(crate) fn view(app: &App) -> styra_ui::event_list::EventListView<'_> {
    let branch_point = app.timeline.branch_point();
    let entries = app
        .timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(index, _)| app.timeline.is_visible(*index))
        .map(|(index, entry)| {
            let branch = branch_link(app, entry.event());
            // A marker's only detail is the message of where it leads, which
            // the roster may not have.
            let has_detail = match entry.event().branch_target() {
                Some(_) => branch.message.is_some(),
                None => entry.has_detail(),
            };
            styra_ui::event_list::EventEntry {
                event: entry.event(),
                version: ui_version(entry.version()),
                expanded: app.timeline.entry_expanded(index),
                has_detail,
                contract: entry.contract.as_ref(),
                selected: index == app.timeline.selected,
                link_highlight: app
                    .link_highlight
                    .filter(|highlight| highlight.entry == index)
                    .map(|highlight| highlight.link),
                branch,
                branch_provider_switch: branch_provider_switch(app, entry.event()),
                inherited: branch_point.is_some_and(|point| index < point),
            }
        })
        .collect();
    let status = status(&app.activity.status, app.activity.progress());
    let usage = app.activity.latest_usage.as_ref().map(|usage| {
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_input_tokens,
        )
    });
    styra_ui::event_list::EventListView {
        chrome: super::panel_chrome(app, None),
        entries,
        activity: {
            let counts = app.interactions.activity_counts();
            styra_ui::event_list::ActivityCounts {
                running: counts.running,
                idle: counts.idle,
                stopped: counts.stopped,
            }
        },
        all_events: app.timeline.all_events,
        show_minor: app.timeline.show_minor,
        uncommitted_changes: super::uncommitted_changes(app),
        usage,
        can_configure_launch: app.can_configure_launch(),
        selection_name: app.selection.name(),
        requested_offset: app.timeline.list_offset,
        requested_row_offset: app.timeline.list_row_offset,
        scroll_delta: app.timeline.list_scroll_delta,
        anchor_selection: app.timeline.anchor_selection,
        keep_viewport: app.timeline.keep_viewport,
        moved_backward: app
            .timeline
            .rendered_selection
            .is_some_and(|rendered| app.timeline.selected < rendered),
        max_lines_above_selection: app.timeline.max_lines_above_selection,
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
        search: app.search.view(),
        status,
        queued: app.outbox.queued(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{apply_feedback, test_support};
    use super::view;
    use crate::app::App;

    use styra_protocol::event::AgentEvent;

    /// The `/` prompt is shown along the bottom of the list it is marking, and
    /// holds every printable key while it is open — including the letters that
    /// are commands on the list underneath.
    #[test]
    fn the_search_prompt_shows_what_is_being_typed_into_it() {
        use crossterm::event::{KeyCode, KeyEvent};

        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "reworked the retry queue".into(),
        });
        app.search.open();
        for character in "que".chars() {
            crate::input::handle_search_key(&mut app, KeyEvent::from(KeyCode::Char(character)));
        }

        assert_eq!(view(&app).search.query, Some("que"));
        assert!(view(&app).search.typing);

        // `q` quits the list; here it is a letter of the term.
        crate::input::handle_search_key(&mut app, KeyEvent::from(KeyCode::Char('q')));
        assert_eq!(app.search.query(), Some("queq"));
    }

    #[test]
    fn queued_messages_are_passed_in_send_order() {
        use styra_protocol::QueuedMessage;

        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "rename the config loader".into(),
        });
        app.outbox.replace_queued(vec![
            QueuedMessage::new("then run the tests"),
            QueuedMessage::new("and commit it\nwith a short message"),
        ]);

        assert_eq!(view(&app).queued, app.outbox.queued());
        assert_eq!(view(&app).queued.len(), 2);
    }

    #[test]
    fn the_entire_queue_is_passed_to_ui() {
        use styra_protocol::QueuedMessage;

        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "rename the config loader".into(),
        });
        app.outbox.replace_queued(
            (0..8)
                .map(|n| QueuedMessage::new(format!("follow-up {n}")))
                .collect(),
        );

        assert_eq!(
            view(&app).queued.len(),
            8,
            "all data is passed; UI chooses how much fits"
        );
        assert_eq!(view(&app).queued, app.outbox.queued());
    }

    #[test]
    fn new_interactions_pass_distinct_entry_identities_and_events() {
        let mut earlier = test_support::app("s1");
        earlier.push_event(AgentEvent::UserMessage {
            text: "rename the config loader".into(),
        });
        let mut next = test_support::app("s2");
        next.push_event(AgentEvent::UserMessage {
            text: "add retries to the uploader".into(),
        });
        let old = view(&earlier);
        let new = view(&next);
        assert_ne!(old.entries[0].version.id, new.entries[0].version.id);
        assert_eq!(new.entries[0].event, next.timeline.entries[0].event());
    }

    fn scrolled_conversation() -> App {
        let mut app = test_support::app("s1");
        app.timeline.all_events = false;
        app.push_event(AgentEvent::UserMessage {
            text: "please refactor the retry backoff logic".into(),
        });
        for step in 0..12 {
            app.push_event(AgentEvent::AgentMessage {
                text: format!("step {step}: looking at the backoff code in detail"),
            });
        }
        // The UI reports the anchor. Application tests must not calculate it.
        apply_feedback(
            &mut app,
            &styra_ui::RenderFeedback {
                list_offset: Some(7),
                list_row_offset: Some(0),
                ..Default::default()
            },
        );
        app
    }

    /// A tool finishing replaces the row that showed it starting, in place.
    /// Under the default conversation-only filter neither row is shown, so
    /// the operator's view of the conversation must not move at all: nothing
    /// they can see has changed, and they did not navigate.
    ///
    /// It used to jump to the very top of the log. `ingest`'s replacement
    /// paths follow the tail without checking that the tail is a row the
    /// filters show, which leaves the selection on a hidden entry; the list
    /// then renders with nothing selected, and ratatui's `ListState::select`
    /// documents that selecting `None` also resets the offset to zero. That
    /// zero is what the render reports back as its effective offset, so the
    /// reset is persisted rather than lasting a single frame.
    #[test]
    fn a_hidden_tool_finishing_does_not_scroll_the_conversation() {
        let mut app = scrolled_conversation();
        let anchored = app.timeline.list_offset;

        app.push_event(AgentEvent::ToolStarted {
            id: "x1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test backoff\"}".into(),
        });
        assert_eq!(view(&app).requested_offset, anchored);
        assert!(view(&app).entries.iter().any(|entry| entry.selected));
        assert_eq!(
            app.timeline.list_offset, anchored,
            "a hidden row arriving is not a reason to scroll"
        );

        app.push_event(AgentEvent::ToolCompleted {
            id: "x1".into(),
            name: "Bash".into(),
            detail: String::new(),
            status: "ok".into(),
            output: "test result: ok\n".into(),
        });
        assert_eq!(view(&app).requested_offset, anchored);
        assert!(view(&app).entries.iter().any(|entry| entry.selected));

        assert_eq!(
            app.timeline.list_offset, anchored,
            "the tool completion replaced a hidden row, so the viewport stays"
        );
    }

    /// The same defect through the other replacement path: a shell command
    /// finishing. `CommandCompleted` replaces its `CommandStarted` row, and
    /// both are hidden by the conversation filter.
    #[test]
    fn a_hidden_command_finishing_does_not_scroll_the_conversation() {
        let mut app = scrolled_conversation();
        let anchored = app.timeline.list_offset;

        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test backoff".into(),
        });
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test backoff".into(),
            status: "ok".into(),
            exit_code: Some(0),
            output: "test result: ok\n".into(),
        });
        assert_eq!(view(&app).requested_offset, anchored);
        assert!(view(&app).entries.iter().any(|entry| entry.selected));

        assert_eq!(
            app.timeline.list_offset, anchored,
            "the command completion replaced a hidden row, so the viewport stays"
        );
    }

    #[test]
    fn navigation_passes_the_existing_viewport_anchor() {
        let mut app = scrolled_conversation();
        let anchored = app.timeline.list_offset;
        app.select_prev_line();
        assert_eq!(view(&app).requested_offset, anchored);
        assert!(view(&app).entries.iter().any(|entry| entry.selected));
        assert!(view(&app).entries.iter().any(|e| e.selected));
        app.select_next_line();
        assert_eq!(view(&app).requested_offset, anchored);
        assert!(view(&app).entries.iter().any(|entry| entry.selected));
    }

    #[test]
    fn new_messages_request_anchoring_when_following_the_tail() {
        let mut app = scrolled_conversation();
        app.select_prev_line();
        app.select_next_line();
        app.push_event(AgentEvent::AgentMessage {
            text: "step 12: the newest".into(),
        });
        let view = view(&app);
        assert!(view.anchor_selection);
        assert!(view.entries.last().unwrap().selected);
        assert!(
            matches!(view.entries.last().unwrap().event, AgentEvent::AgentMessage { text } if text == "step 12: the newest")
        );
    }

    /// The selection is what the viewport is anchored to, so following the
    /// tail may only land on a row the filters actually show. Landing on a
    /// hidden one is what leaves the list rendering with nothing selected.
    #[test]
    fn following_the_tail_lands_on_a_row_the_filters_show() {
        let mut app = scrolled_conversation();

        app.push_event(AgentEvent::ToolStarted {
            id: "x1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test backoff\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "x1".into(),
            name: "Bash".into(),
            detail: String::new(),
            status: "ok".into(),
            output: "test result: ok\n".into(),
        });

        assert!(
            app.timeline.is_visible(app.timeline.selected),
            "the selection must stay on a visible row"
        );
        assert!(
            view(&app).entries.iter().any(|entry| entry.selected),
            "some rendered row is the selected one"
        );
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;
    use styra_protocol::{
        agent::Provider,
        event::{AgentEvent, BranchDirection},
        BranchHistory, SessionOrigin,
    };

    #[test]
    fn provider_switch_resolves_on_both_sides_from_persisted_origin() {
        let mut app = App::new(
            styra_protocol::agent::Selection::new(Provider::Claude),
            "source",
        );
        let origin = SessionOrigin {
            session_id: "source".into(),
            provider: Provider::Claude,
            at_ms: None,
            history: BranchHistory::ThroughSelected,
        };
        app.interactions
            .note_origin("destination", Provider::Codex, Some(&origin));
        let marker = |direction, session: &str| AgentEvent::Branched {
            direction,
            session: session.into(),
            name: None,
        };
        let expected = Some((Provider::Claude, Provider::Codex));
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::To, "destination")),
            expected
        );
        app.session_id = "destination".into();
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::From, "source")),
            expected
        );
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::From, "unrelated")),
            None
        );
        let inherited_origin = SessionOrigin {
            session_id: "destination".into(),
            provider: Provider::Codex,
            at_ms: None,
            history: BranchHistory::ThroughSelected,
        };
        app.interactions
            .note_origin("descendant", Provider::Codex, Some(&inherited_origin));
        app.session_id = "descendant".into();
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::From, "source")),
            expected
        );
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::To, "destination")),
            expected
        );
        app.interactions
            .note_origin("ordinary", Provider::Claude, Some(&origin));
        app.session_id = "source".into();
        assert_eq!(
            branch_provider_switch(&app, &marker(BranchDirection::To, "ordinary")),
            None
        );
    }
}

//! Mapping from the application timeline to the event-list presentation model.

use crate::activity::Status;
use crate::app::App;
use styra_ui::markdown::LinkDisplay;

pub(crate) fn ui_link_display(value: crate::app::LinkDisplay) -> LinkDisplay {
    match value {
        crate::app::LinkDisplay::Compact => LinkDisplay::Compact,
        crate::app::LinkDisplay::Full => LinkDisplay::Full,
    }
}

pub(crate) fn view(app: &App) -> styra_ui::event_list::EventListView<'_> {
    let entries = app
        .timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(index, _)| app.timeline.is_visible(*index))
        .map(|(index, entry)| styra_ui::event_list::EventEntry {
            event: &entry.event,
            expanded: app.timeline.entry_expanded(index),
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: index == app.timeline.selected,
            link_highlight: app
                .link_highlight
                .filter(|highlight| highlight.entry == index)
                .map(|highlight| highlight.link),
        })
        .collect();
    let progress = app.activity.progress();
    let status = match app.activity.status {
        Status::Pending => styra_ui::event_list::EventListStatus::Pending,
        Status::Running => styra_ui::event_list::EventListStatus::Running {
            elapsed: progress.in_status,
            quiet: progress.since_event,
            events: progress.events,
        },
        Status::Idle(_) => styra_ui::event_list::EventListStatus::Idle {
            reason: app.activity.status.reason_label(),
        },
        Status::Background => styra_ui::event_list::EventListStatus::Background {
            elapsed: progress.in_status,
        },
        Status::Stopped(_) => styra_ui::event_list::EventListStatus::Stopped {
            elapsed: progress.in_status,
            reason: app.activity.status.reason_label(),
        },
        Status::Ended { .. } => styra_ui::event_list::EventListStatus::Ended,
    };
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
        conversation_only: app.timeline.conversation_only,
        uncommitted_changes: super::uncommitted_changes(app),
        usage,
        can_configure_launch: app.can_configure_launch(),
        selection_name: app.selection.name(),
        requested_offset: app.timeline.list_offset,
        moved_backward: app
            .timeline
            .rendered_selection
            .is_some_and(|rendered| app.timeline.selected < rendered),
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
        search: app.search.view(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{apply_feedback, draw_application, test_support};
    use super::view;
    use crate::app::App;

    use styra_protocol::event::AgentEvent;
    use styra_ui::{TestUi, Ui};

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
            crate::keys::handle_search_key(&mut app, KeyEvent::from(KeyCode::Char(character)));
        }

        assert!(test_support::rendered(&app).contains("/que▌"));

        // `q` quits the list; here it is a letter of the term.
        crate::keys::handle_search_key(&mut app, KeyEvent::from(KeyCode::Char('q')));
        assert_eq!(app.search.query(), Some("queq"));
    }

    /// Draw `app` and feed the render's own offset back into it, exactly as
    /// the event loop does. The viewport offset only survives across frames
    /// through this round trip, so a test about scrolling has to make it.
    fn draw(ui: &mut dyn Ui, app: &mut App) {
        let feedback = draw_application(ui, app).unwrap();
        apply_feedback(app, &feedback);
    }

    /// A running interaction whose log is longer than the viewport, so it has
    /// scrolled away from the top and a reset is observable.
    fn scrolled_conversation(ui: &mut dyn Ui) -> App {
        let mut app = test_support::app("s1");
        app.timeline.conversation_only = true;
        app.timeline.show_minor = false;
        app.push_event(AgentEvent::UserMessage {
            text: "please refactor the retry backoff logic".into(),
        });
        for step in 0..12 {
            app.push_event(AgentEvent::AgentMessage {
                text: format!("step {step}: looking at the backoff code in detail"),
            });
        }
        draw(ui, &mut app);
        assert!(
            app.timeline.list_offset > 0,
            "the log must have scrolled for a reset to be visible"
        );
        app
    }

    /// A tool finishing replaces the row that showed it starting, in place.
    /// Under the default `conversation_only` filter neither row is shown, so
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
        let mut ui = TestUi::new(80, 16).unwrap();
        let mut app = scrolled_conversation(&mut ui);
        let anchored = app.timeline.list_offset;
        let top = ui.rows()[1].clone();

        app.push_event(AgentEvent::ToolStarted {
            id: "x1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test backoff\"}".into(),
        });
        draw(&mut ui, &mut app);
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
        draw(&mut ui, &mut app);

        assert_eq!(
            app.timeline.list_offset, anchored,
            "the tool completion replaced a hidden row, so the viewport stays"
        );
        assert_eq!(ui.rows()[1], top, "the same entry is still at the top");
    }

    /// The same defect through the other replacement path: a shell command
    /// finishing. `CommandCompleted` replaces its `CommandStarted` row, and
    /// both are hidden by the conversation filter.
    #[test]
    fn a_hidden_command_finishing_does_not_scroll_the_conversation() {
        let mut ui = TestUi::new(80, 16).unwrap();
        let mut app = scrolled_conversation(&mut ui);
        let anchored = app.timeline.list_offset;
        let top = ui.rows()[1].clone();

        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test backoff".into(),
        });
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test backoff".into(),
            status: "ok".into(),
            exit_code: Some(0),
            output: "test result: ok\n".into(),
        });
        draw(&mut ui, &mut app);

        assert_eq!(
            app.timeline.list_offset, anchored,
            "the command completion replaced a hidden row, so the viewport stays"
        );
        assert_eq!(ui.rows()[1], top, "the same entry is still at the top");
    }

    /// The selection is what the viewport is anchored to, so following the
    /// tail may only land on a row the filters actually show. Landing on a
    /// hidden one is what leaves the list rendering with nothing selected.
    #[test]
    fn following_the_tail_lands_on_a_row_the_filters_show() {
        let mut ui = TestUi::new(80, 16).unwrap();
        let mut app = scrolled_conversation(&mut ui);

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

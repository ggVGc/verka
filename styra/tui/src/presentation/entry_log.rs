//! The entry-log pane: the full interaction log behind the selected
//! conversation entry — that message and every entry under it, tool calls,
//! thinking, and lifecycle alike.
//!
//! The event list answers "what was said"; with the conversation-only filter
//! on, which is how it is usually read, the work between two messages is off
//! screen entirely. This view answers "what happened for this one" without
//! making the operator turn the filter off and find their place again, so it
//! deliberately ignores both list filters: the entries it shows are exactly the
//! ones the list is hiding.
//!
//! Entries use the same collapsed, one-line rows as the main event list. This
//! is a scoped interaction log, not a second detail reader: the operator can
//! compare its compact sequence with the selected row immediately above it.

use super::list::ui_link_display;
use crate::app::App;
pub(crate) fn view(app: &App) -> styra_ui::event_list::EntryLogView<'_> {
    let span = app.timeline.conversation_span();
    let entries = app.timeline.entries[span]
        .iter()
        .map(|entry| styra_ui::event_list::EventEntry {
            event: &entry.event,
            expanded: false,
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: false,
        })
        .collect();
    styra_ui::event_list::EntryLogView {
        entries,
        requested_scroll: app.entry_log.offset,
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support;
    use crate::app::App;

    use styra_protocol::event::AgentEvent;

    /// A session whose log holds two turns, each a message followed by the
    /// work done under it.
    fn app_with_two_turns() -> App {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix the retry backoff".into(),
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test backoff".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "backoff fixed".into(),
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "git commit --amend".into(),
        });
        app
    }

    #[test]
    fn entry_log_follows_the_selected_message() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        let screen = test_support::screen_sized(&app, 120, 30);
        let (_, entry_log_y) = screen.find("entry log · follows selection");
        let (_, command_y) = screen.find("cargo test backoff");
        assert!(
            command_y > entry_log_y,
            "the scoped entry is below its title"
        );
        // The next message begins its own stretch, so neither it nor the work
        // under it belongs to this one.
        assert!(!screen.body().contains("git commit --amend"));
    }

    /// The view exists for the conversation-only reader, so the entries it
    /// shows must be exactly the ones that filter hides — not filtered again.
    #[test]
    fn entry_log_ignores_the_conversation_only_filter() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        assert!(test_support::rendered(&app).contains("cargo test backoff"));
    }

    /// With the filter off the cursor can rest on a tool row. That row is part
    /// of the message's stretch, so it shows the same stretch.
    #[test]
    fn a_selection_inside_a_stretch_shows_that_whole_stretch() {
        let mut app = app_with_two_turns();
        app.timeline.selected = 1;
        let span = app.timeline.conversation_span();
        assert_eq!(span, 0..2);
    }

    /// This is a scoped list, not a detail preview. The shared collapsed-row
    /// renderer must keep command output out of this window just as it does in
    /// the main interaction log.
    #[test]
    fn entry_log_keeps_each_scoped_entry_to_one_line() {
        let mut app = test_support::app("s1");
        app.timeline.conversation_only = true;
        app.push_event(AgentEvent::UserMessage {
            text: "run the tests".into(),
        });
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "this command output must not be in the entry log".into(),
        });
        app.select_first();
        app.toggle_entry_log();

        let screen = test_support::screen_sized(&app, 120, 30);
        let (_, command_y) = screen.find("cargo test");
        assert!(command_y > 15, "the command summary is in the bottom pane");
        assert!(!screen.body().contains("this command output"));
    }

    /// The pane occupies the bottom of the Events screen while the list stays
    /// above it. Moving that list changes the pane's content; it does not turn
    /// the pane into a second navigation target.
    #[test]
    fn entry_log_is_a_selection_following_pane_below_the_event_list() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();

        let screen = test_support::screen_sized(&app, 120, 30);
        let (_, entry_log_y) = screen.find("entry log · follows selection");
        let (_, command_y) = screen.find("cargo test backoff");
        assert!(entry_log_y > 15, "the entry log is the bottom pane");
        assert!(
            command_y > entry_log_y,
            "the selected entry's work is in that pane"
        );

        app.select_next_line();
        let screen = test_support::screen_sized(&app, 120, 30);
        let (_, y) = screen.find("git commit --amend");
        assert!(y > entry_log_y, "the pane followed the list selection");
    }

    /// The scoped log belongs to the interaction-log pane on the left. A
    /// preview stays beside that whole pane, so it begins at the top and runs
    /// the screen's full primary height rather than being shortened to the
    /// main list's upper half.
    #[test]
    fn preview_stays_full_height_when_the_entry_log_is_open() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.preview.show();

        let screen = test_support::screen_sized(&app, 120, 30);
        let (preview_x, preview_y) = screen.find("preview · pretty");
        let (entry_log_x, entry_log_y) = screen.find("entry log · follows selection");
        assert!(preview_x > 70, "the preview is the right-hand pane");
        assert_eq!(preview_y, 0, "the preview starts at the top");
        assert!(entry_log_x < 70, "the entry log stays in the left pane");
        assert!(entry_log_y > 15, "the entry log stays below the main list");
    }
}

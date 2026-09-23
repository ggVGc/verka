//! The entry-log pane: the full interaction log behind the selected
//! conversation entry — that message and every entry under it, tool calls,
//! thinking, and lifecycle alike.
//!
//! The event list answers "what was said"; with the conversation-only filter
//! on, which is how it is usually read, the work between two messages is off
//! screen entirely. This view answers "what happened for this one" without
//! making the operator turn the filter off and find their place again, so it
//! deliberately ignores that filter: the entries it shows are exactly the ones
//! the list is hiding. It does honour `m`, though — minor lifecycle events
//! turned off are noise everywhere on this screen, not just in the list.
//!
//! Entries use the same collapsed, one-line rows as the main event list. This
//! is a scoped interaction log, not a second detail reader: the operator can
//! compare its compact sequence with the selected row immediately above it.
//!
//! The pane opens with its cursor on its last, newest entry, and `Tab` hands
//! it the navigation keys. The preview shows whatever its cursor is on for as
//! long as it is open, keys or not; see [`crate::entry_log`].

use super::list::ui_link_display;
use crate::app::App;
pub(crate) fn view(app: &App) -> styra_ui::event_list::EntryLogView<'_> {
    let shown = app.entry_log_indices();
    // The cursor is drawn whether or not the pane holds the keys: it is what
    // the preview beside it is showing, so the row it is on has to be visible
    // for the pair to be read together.
    let cursor = app.entry_log.cursor(shown.len());
    let entries = shown
        .iter()
        .map(|&idx| &app.timeline.entries[idx])
        .enumerate()
        .map(|(index, entry)| styra_ui::event_list::EventEntry {
            event: &entry.event,
            expanded: false,
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: cursor == Some(index),
        })
        .collect();
    styra_ui::event_list::EntryLogView {
        entries,
        focused: app.entry_log.focused(),
        requested_scroll: app.entry_log.scroll.offset,
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

    /// `m` says whether minor lifecycle events are worth screen space, and it
    /// says it for the whole Events screen: a pane that kept showing them
    /// would put back what the operator just turned off.
    #[test]
    fn entry_log_hides_minor_events_unless_the_list_shows_them() {
        let mut app = test_support::app("s1");
        app.timeline.conversation_only = true;
        app.push_event(AgentEvent::UserMessage {
            text: "run the tests".into(),
        });
        app.push_event(AgentEvent::Thinking {
            text: "weighing the retry backoff".into(),
            tokens: None,
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test backoff".into(),
        });
        app.select_first();
        app.toggle_entry_log();
        assert!(!app.timeline.show_minor);

        let screen = test_support::rendered(&app);
        assert!(screen.contains("cargo test backoff"));
        assert!(!screen.contains("weighing the retry backoff"));

        app.toggle_minor();
        assert!(test_support::rendered(&app).contains("weighing the retry backoff"));
    }

    /// The cursor walks what the pane draws, so a hidden entry is not a row it
    /// can land on.
    #[test]
    fn the_panes_cursor_skips_the_entries_it_is_not_showing() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "run the tests".into(),
        });
        app.push_event(AgentEvent::Thinking {
            text: "weighing it".into(),
            tokens: None,
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test".into(),
        });
        app.select_first();
        app.toggle_entry_log();
        app.toggle_entry_log_focus();
        app.entry_log_select_first();

        assert_eq!(app.entry_log_index(), Some(0));
        app.entry_log_select_next();
        assert_eq!(
            app.entry_log_entry().map(|entry| entry.event.tag()),
            Some("shell"),
            "the thinking entry is not on screen to stop at"
        );
        app.entry_log_select_next();
        assert_eq!(app.entry_log_index(), Some(2), "and that is the last row");
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
    /// above it. Until `Tab` hands it the keys, moving that list is what
    /// changes the pane's content.
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

    /// `Tab` makes the pane the window the movement keys act on. The preview
    /// reads the pane's cursor for as long as the pane is open, so `Tab`
    /// itself changes nothing about what it shows: the two windows are read as
    /// one pair, with the finer-grained of them driving the preview.
    #[test]
    fn tab_moves_the_keys_to_the_pane_and_the_preview_stays_on_its_cursor() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.preview.show();

        // The pane opened on its newest entry — the command under the message
        // — and that is what the preview shows, before any key is pressed.
        assert_eq!(app.entry_log_index(), Some(1));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("shell")
        );

        app.toggle_entry_log_focus();
        assert!(app.entry_log.focused());
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("shell"),
            "taking the keys leaves the preview on the entry it was showing"
        );

        app.entry_log_select_prev();
        assert_eq!(app.entry_log_index(), Some(0));
        assert_eq!(
            app.timeline.selected, 0,
            "the event list keeps its own place"
        );
        assert!(test_support::rendered(&app).contains("cargo test backoff"));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("user"),
            "the preview shows the entry the pane's cursor is on"
        );

        // And back: the movement keys belong to the event list again, but the
        // preview keeps reading the open pane's cursor.
        app.toggle_entry_log_focus();
        assert!(!app.entry_log.focused());
        assert_eq!(app.entry_log_index(), Some(0));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("user")
        );
    }

    /// Closing the pane hands the preview back to the event list's selection.
    #[test]
    fn closing_the_pane_returns_the_preview_to_the_list_selection() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.toggle_entry_log_focus();
        app.entry_log_select_last();

        app.toggle_entry_log();
        assert_eq!(app.entry_log_index(), None);
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("user")
        );
    }

    /// Moving the event list while the pane follows it moves the pane's cursor
    /// too: the preview reads that cursor, so it has to land on the entry the
    /// operator just selected rather than on a row of the stretch they left.
    #[test]
    fn an_unfocused_panes_cursor_follows_the_list_selection() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = true;
        app.select_first();
        app.toggle_entry_log();

        app.select_next_line();
        assert_eq!(
            app.entry_log_index(),
            Some(app.timeline.selected),
            "the pane's cursor is on the newly selected message"
        );
        assert_eq!(
            app.preview_entry().map(|entry| entry.event.tag()),
            Some("agent")
        );
    }

    /// A stretch taller than the pane. The cursor is the operator's place in
    /// it, so moving it past the bottom row scrolls the pane rather than
    /// leaving the highlight off screen.
    #[test]
    fn the_pane_scrolls_to_keep_its_cursor_on_screen() {
        let mut app = test_support::app("s1");
        app.timeline.conversation_only = true;
        app.push_event(AgentEvent::UserMessage {
            text: "run everything".into(),
        });
        for index in 0..40 {
            app.push_event(AgentEvent::CommandStarted {
                command: format!("cargo test case-{index}"),
            });
        }
        app.select_first();
        app.toggle_entry_log();
        app.toggle_entry_log_focus();
        app.entry_log_select_first();

        let screen = test_support::screen_sized(&app, 120, 30);
        assert!(screen.body().contains("cargo test case-0"));
        assert!(!screen.body().contains("cargo test case-39"));

        app.entry_log_select_last();
        let screen = test_support::screen_sized(&app, 120, 30);
        assert!(screen.body().contains("cargo test case-39"));
        assert!(!screen.body().contains("cargo test case-0"));
        assert_eq!(
            app.preview_entry()
                .map(|entry| crate::files::entry_text(entry).contains("case-39")),
            Some(true),
            "the preview shows the entry the cursor reached"
        );
    }

    /// Which window has the keys has to be visible, or the operator cannot
    /// tell what `j` is about to move.
    #[test]
    fn the_pane_says_when_it_is_holding_the_navigation_keys() {
        let mut app = app_with_two_turns();
        app.timeline.selected = 0;
        app.toggle_entry_log();
        assert!(test_support::rendered(&app).contains("follows selection"));

        app.toggle_entry_log_focus();
        let screen = test_support::rendered(&app);
        assert!(screen.contains("Tab: back to the list"));
        assert!(!screen.contains("follows selection"));
    }

    /// `Tab` is the pane's key. With the pane closed there is only one window
    /// on the Events screen, so it has nothing to move.
    #[test]
    fn tab_does_nothing_while_the_pane_is_closed() {
        let mut app = app_with_two_turns();
        app.toggle_entry_log_focus();
        assert!(!app.entry_log.focused());
        assert_eq!(app.entry_log_index(), None);
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

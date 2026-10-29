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

use super::list::{branch_link, branch_provider_switch, ui_link_display};
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
            event: entry.event(),
            version: super::list::ui_version(entry.version()),
            expanded: false,
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: cursor == Some(index),
            link_highlight: None,
            branch: branch_link(app, entry.event()),
            branch_provider_switch: branch_provider_switch(app, entry.event()),
            inherited: false,
        })
        .collect();
    styra_ui::event_list::EntryLogView {
        entries,
        focused: app.entry_log.focused() && !app.diff_visible(),
        requested_scroll: app.entry_log.scroll.offset,
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
        focus_key: crate::keybindings::EVENTS_ENTRY_LOG_FOCUS.keys_label(),
        close_key: crate::keybindings::GLOBAL_ENTRY_LOG.keys_label(),
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

    /// `m` says whether minor lifecycle events are worth screen space, and it
    /// says it for the whole Events screen: a pane that kept showing them
    /// would put back what the operator just turned off.
    #[test]
    fn entry_log_hides_minor_events_unless_the_list_shows_them() {
        let mut app = test_support::app("s1");
        app.timeline.all_events = false;
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

        assert!(!super::view(&app)
            .entries
            .iter()
            .any(|e| matches!(e.event, AgentEvent::Thinking { .. })));
        app.toggle_minor();
        assert!(super::view(&app).entries.iter().any(|e| matches!(e.event, AgentEvent::Thinking { text, .. } if text == "weighing the retry backoff")));
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
            app.entry_log_entry().map(|entry| entry.event().tag()),
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

    #[test]
    fn entry_log_passes_collapsed_entries_with_their_full_events() {
        let mut app = test_support::app("s1");
        app.timeline.all_events = false;
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

        let view = super::view(&app);
        assert_eq!(view.entries.len(), 2);
        assert!(view.entries.iter().all(|e| !e.expanded));
        assert!(
            matches!(view.entries[1].event, AgentEvent::CommandCompleted { output, .. } if output == "this command output must not be in the entry log")
        );
    }

    #[test]
    fn entry_log_data_follows_the_list_selection() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = false;
        app.timeline.selected = 0;
        app.toggle_entry_log();

        test_support::inspect(&app, |v| {
            let styra_ui::application::MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(!e.entry_log.unwrap().focused);
            let entries = &e.entry_log.unwrap().entries;
            assert_eq!(entries.len(), 2);
            assert!(entries.iter().any(|entry| matches!(entry.event,
                AgentEvent::CommandStarted { command } if command == "cargo test backoff")));
            assert!(!entries.iter().any(|entry| matches!(entry.event,
                AgentEvent::CommandStarted { command } if command == "git commit --amend")));
            assert_eq!(
                e.entry_log.unwrap().entries[1].event,
                app.timeline.entries[1].event()
            );
        });
        app.select_next_line();
        assert_eq!(
            super::view(&app).entries[1].event,
            app.timeline.entries[3].event()
        );
    }

    /// `Tab` makes the pane the window the movement keys act on. The preview
    /// reads the pane's cursor for as long as the pane is open, so `Tab`
    /// itself changes nothing about what it shows: the two windows are read as
    /// one pair, with the finer-grained of them driving the preview.
    #[test]
    fn tab_moves_the_keys_to_the_pane_and_the_preview_stays_on_its_cursor() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.preview.show();

        // The pane opened on its newest entry — the command under the message
        // — and that is what the preview shows, before any key is pressed.
        assert_eq!(app.entry_log_index(), Some(1));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("shell")
        );

        app.toggle_entry_log_focus();
        assert!(app.entry_log.focused());
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("shell"),
            "taking the keys leaves the preview on the entry it was showing"
        );

        app.entry_log_select_prev();
        assert_eq!(app.entry_log_index(), Some(0));
        assert_eq!(
            app.timeline.selected, 0,
            "the event list keeps its own place"
        );
        assert!(super::view(&app).entries.iter().any(|e| matches!(e.event, AgentEvent::CommandStarted { command } if command == "cargo test backoff")));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("user"),
            "the preview shows the entry the pane's cursor is on"
        );

        // And back: the movement keys belong to the event list again, but the
        // preview keeps reading the open pane's cursor.
        app.toggle_entry_log_focus();
        assert!(!app.entry_log.focused());
        assert_eq!(app.entry_log_index(), Some(0));
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("user")
        );
    }

    /// Closing the pane hands the preview back to the event list's selection.
    #[test]
    fn closing_the_pane_returns_the_preview_to_the_list_selection() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = false;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.toggle_entry_log_focus();
        app.entry_log_select_last();

        app.toggle_entry_log();
        assert_eq!(app.entry_log_index(), None);
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("user")
        );
    }

    /// Moving the event list while the pane follows it moves the pane's cursor
    /// too: the preview reads that cursor, so it has to land on the entry the
    /// operator just selected rather than on a row of the stretch they left.
    #[test]
    fn an_unfocused_panes_cursor_follows_the_list_selection() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = false;
        app.select_first();
        app.toggle_entry_log();

        app.select_next_line();
        assert_eq!(
            app.entry_log_index(),
            Some(app.timeline.selected),
            "the pane's cursor is on the newly selected message"
        );
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("agent")
        );
    }

    #[test]
    fn entry_log_marks_the_entry_its_cursor_reaches() {
        let mut app = test_support::app("s1");
        app.timeline.all_events = false;
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

        assert!(super::view(&app).entries[0].selected);
        app.entry_log_select_last();
        let view = super::view(&app);
        assert!(view.entries.last().unwrap().selected);
        assert!(
            matches!(view.entries.last().unwrap().event, AgentEvent::CommandStarted { command } if command == "cargo test case-39")
        );
        assert_eq!(
            app.preview_entry().unwrap().event(),
            view.entries.last().unwrap().event
        );
    }

    /// Which window has the keys has to be visible, or the operator cannot
    /// tell what `j` is about to move.
    #[test]
    fn the_pane_says_when_it_is_holding_the_navigation_keys() {
        let mut app = app_with_two_turns();
        app.timeline.selected = 0;
        app.toggle_entry_log();
        assert!(!super::view(&app).focused);
        app.toggle_entry_log_focus();
        let view = super::view(&app);
        assert!(view.focused);
        assert_eq!(view.focus_key, "Tab/Shift+Tab");
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

    #[test]
    fn entry_log_and_preview_are_dispatched_together() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = true;
        app.timeline.selected = 0;
        app.toggle_entry_log();
        app.preview.show();

        test_support::inspect(&app, |v| {
            let styra_ui::application::MainView::Events(e) = &v.main else {
                panic!()
            };
            assert!(e.entry_log.is_some());
            assert!(matches!(
                e.preview,
                Some(styra_ui::application::SidePreview::Entry(_))
            ));
        });
    }
}

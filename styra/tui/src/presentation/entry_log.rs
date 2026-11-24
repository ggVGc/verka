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
            branch: branch_link(app, &app.session_id, entry.event()),
            branch_provider_switch: branch_provider_switch(app, entry.event()),
            branch_follows_marker: false,
            selected_branch: None,
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
mod tests;

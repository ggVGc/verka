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
        usage,
        can_configure_launch: app.can_configure_launch(),
        selection_name: app.selection.name(),
        requested_offset: app.timeline.list_offset,
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
        status,
    }
}

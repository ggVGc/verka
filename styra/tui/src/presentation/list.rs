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

/// For a branch marker in `owner`'s conversation, the other session's current
/// name, tags, state, latest message, and worktree when it is not `owner`'s,
/// resolved live from the roster rather than trusted from the event itself:
/// the event's own `name` is a snapshot taken when the marker was written,
/// and a rename since would leave it stale — as would a worktree made for
/// the branch afterward. Empty both for an event that is not a marker and for
/// one whose target the roster has no row for.
///
/// A `branched to` marker also carries the listed sessions branched off its
/// target, and theirs in turn, so the marker can show the whole subtree.
pub(crate) fn branch_link<'a>(
    app: &'a App,
    owner: &str,
    event: &styra_protocol::event::AgentEvent,
) -> styra_ui::event_list::BranchLink<'a> {
    use styra_protocol::event::{AgentEvent, BranchDirection};
    let Some(id) = event.branch_target() else {
        return Default::default();
    };
    let mut link = session_link(app, owner, id);
    if matches!(
        event,
        AgentEvent::Branched {
            direction: BranchDirection::To,
            ..
        }
    ) {
        let tree = app.interactions.branch_tree(id, owner);
        link.children = nested(app, owner, &mut tree.iter().peekable(), 1);
    }
    link
}

/// The links for the run of `tree` — [`crate::interactions::Interactions::branch_tree`]
/// — at `depth`, each holding the deeper run that follows it.
fn nested<'a>(
    app: &'a App,
    owner: &str,
    tree: &mut std::iter::Peekable<std::slice::Iter<'_, (usize, &str)>>,
    depth: usize,
) -> Vec<styra_ui::event_list::BranchLink<'a>> {
    let mut links = Vec::new();
    while let Some(&&(at, id)) = tree.peek() {
        if at != depth {
            break;
        }
        tree.next();
        let mut link = session_link(app, owner, id);
        link.children = nested(app, owner, tree, depth + 1);
        links.push(link);
    }
    links
}

/// [`branch_link`] for the session `id`, without the branches beneath it.
fn session_link<'a>(app: &'a App, owner: &str, id: &str) -> styra_ui::event_list::BranchLink<'a> {
    let interaction = app.interactions.current(id);
    styra_ui::event_list::BranchLink {
        name: app.interactions.name(id),
        state: interaction.map(super::interactions::state),
        message: interaction.and_then(|interaction| interaction.last_message.as_deref()),
        worktree: interaction.and_then(|interaction| {
            let here = app.interactions.current(owner)?.checkout.as_ref()?;
            other_worktree(here, interaction.checkout.as_ref()?)
        }),
        tags: interaction.map_or(&[], |interaction| &interaction.tags),
        children: Vec::new(),
    }
}

/// What a branch marker calls `there`'s worktree, seen from `here`: nothing
/// when they are the same one, a linked worktree by its directory name as the
/// header names it, and the main checkout as that, since its directory name
/// is the repository's and would read as just another worktree.
fn other_worktree<'a>(
    here: &styra_protocol::CheckoutState,
    there: &'a styra_protocol::CheckoutState,
) -> Option<&'a str> {
    if here.worktree == there.worktree {
        None
    } else if there.linked() {
        there.worktree.file_name()?.to_str()
    } else {
        Some("main checkout")
    }
}

pub(crate) fn branch_provider_switch(
    app: &App,
    event: &styra_protocol::event::AgentEvent,
) -> Option<(
    styra_protocol::agent::Provider,
    styra_protocol::agent::Provider,
)> {
    owned_branch_provider_switch(app, &app.session_id, event)
}

/// [`branch_provider_switch`] for a marker in the log of the session `owner`.
pub(crate) fn owned_branch_provider_switch(
    app: &App,
    owner: &str,
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
        } => app.interactions.provider_switch_from(session, owner),
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
    let entries = app
        .timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(index, _)| app.timeline.is_visible(*index))
        .map(|(index, entry)| {
            let branch = branch_link(app, &app.session_id, entry.event());
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
                branch_follows_marker: false,
                selected_branch: (index == app.timeline.selected)
                    .then(|| app.selected_branch())
                    .flatten(),
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
        read_only: app.launch.workspace_read_only(),
        usage,
        can_configure_launch: app.can_configure_launch(),
        selection_name: app.selection.name(),
        requested_offset: app.timeline.list_offset,
        requested_row_offset: app.timeline.list_row_offset,
        scroll_delta: app.timeline.list_scroll_delta,
        rendered_selection_row: app
            .timeline
            .rendered_selection_row
            .filter(|_| app.timeline.rendered_selection == Some(app.timeline.selected)),
        anchor: app.timeline.list_anchor,
        moved_backward: app
            .timeline
            .rendered_selection
            .is_some_and(|rendered| app.timeline.selected < rendered),
        protocol: app.selection.provider.protocol(),
        links: ui_link_display(app.link_display),
        search: app.search.view(),
        status,
        queued: app.outbox.queued(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod branch_tests;

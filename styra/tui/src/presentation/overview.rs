//! Mapping from the live-interaction snapshot to the overview's tiles.

use crate::activity::{IdleReason, Status};
use crate::app::App;
use std::borrow::Cow;
use std::time::Duration;
use styra_protocol::{InteractionActivity, InteractionSummary};
use styra_ui::overview::{OverviewMessage, OverviewTile, OverviewView};

pub(crate) fn view(app: &App) -> OverviewView<'_> {
    let now_ms = super::quota::now_ms();
    let tiles = app
        .interactions
        .overview_indices()
        .into_iter()
        .map(|index| tile(app, &app.interactions.items[index], now_ms))
        .collect();
    OverviewView {
        tiles,
        selected: app.overview.selected(&app.interactions, &app.session_id),
        links: super::list::ui_link_display(app.link_display),
    }
}

fn tile<'a>(app: &'a App, interaction: &'a InteractionSummary, now_ms: u64) -> OverviewTile<'a> {
    let reported = Status::reported(interaction);
    let rate_limited = match &reported {
        Status::Idle(IdleReason::RateLimited(limit)) => Some(Cow::Owned(limit.window.clone())),
        _ => None,
    };
    OverviewTile {
        name: app
            .interactions
            .name(&interaction.id)
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Borrowed(styra_ui::picker::short_id(&interaction.id))),
        workspace: app
            .interactions
            .workspaces
            .iter()
            .find(|workspace| workspace.id == interaction.workspace_id)
            .map(crate::workspace::display_name)
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(interaction.workspace_id.as_str())),
        selection: Cow::Owned(crate::launcher::label(&interaction.selection)),
        branch: interaction
            .checkout
            .as_ref()
            .map(|checkout| checkout.branch.as_deref().unwrap_or("detached head")),
        status: super::interactions::status(&reported, interaction),
        elapsed: elapsed(interaction, now_ms),
        current: interaction.id == app.session_id,
        newly_idle: interaction.activity == InteractionActivity::Pending && interaction.idle_unseen,
        rate_limited,
        uncommitted: interaction.uncommitted_changes,
        tags: &interaction.tags,
        protocol: interaction.selection.provider.protocol(),
        messages: messages(interaction),
    }
}

/// The conversation's tail, or just the agent's last message from a server
/// too old to send more.
fn messages(interaction: &InteractionSummary) -> Vec<OverviewMessage<'_>> {
    if interaction.recent_messages.is_empty() {
        return interaction
            .last_message
            .as_deref()
            .map(|text| OverviewMessage {
                from_operator: false,
                text,
                contract: None,
            })
            .into_iter()
            .collect();
    }
    interaction
        .recent_messages
        .iter()
        .map(|message| OverviewMessage {
            from_operator: message.from_operator,
            text: &message.text,
            contract: message.contract,
        })
        .collect()
}

/// How long a working interaction has been at its turn, by the server's
/// clock — the same figure the attached one's title shows. Idle ones are not
/// timed, as they are not in the title.
fn elapsed(interaction: &InteractionSummary, now_ms: u64) -> Option<String> {
    let working = matches!(
        interaction.activity,
        InteractionActivity::Running | InteractionActivity::Background
    );
    (working && interaction.activity_since_ms > 0).then(|| {
        super::format_duration(Duration::from_millis(
            now_ms.saturating_sub(interaction.activity_since_ms),
        ))
    })
}

/// Every figure on the grid the clock moves, for [`super::clock_reading`].
pub(crate) fn clock_reading(app: &App) -> String {
    let now_ms = super::quota::now_ms();
    app.interactions
        .overview_indices()
        .into_iter()
        .filter_map(|index| elapsed(&app.interactions.items[index], now_ms))
        .collect::<Vec<_>>()
        .join(" ")
}

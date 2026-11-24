//! The live list's rolling time window, including its controls and labels.

use super::{last_message_or_creation_ms, LiveInteractions};
use std::time::{SystemTime, UNIX_EPOCH};
use styra_protocol::InteractionSummary;

/// Kept opaque to the navigator; only membership crosses the module boundary.
#[derive(Clone, Debug, Default)]
pub(super) struct TimeFilter {
    window: Window,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Window {
    #[default]
    Disabled,
    PastDay,
    Past3Days,
    PastWeek,
}

impl Window {
    fn label(self) -> &'static str {
        match self {
            Self::Disabled => "time: disabled",
            Self::PastDay => "past day",
            Self::Past3Days => "past 3 days",
            Self::PastWeek => "past week",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Disabled => Self::PastDay,
            Self::PastDay => Self::Past3Days,
            Self::Past3Days => Self::PastWeek,
            Self::PastWeek => Self::Disabled,
        }
    }

    fn includes_at(self, interaction: &InteractionSummary, now_ms: u64) -> bool {
        let days = match self {
            Self::Disabled => return true,
            Self::PastDay => 1,
            Self::Past3Days => 3,
            Self::PastWeek => 7,
        };
        last_message_or_creation_ms(interaction) >= now_ms.saturating_sub(days * 86_400_000)
    }
}

impl LiveInteractions {
    /// Whether `interaction` falls inside the window, which a branch group
    /// does whole: every listed Session sharing its branch root is in when any
    /// one of them is, so a recent branch keeps the work it came from — and
    /// its siblings — in view, and an old one stays beside a recent source.
    pub(super) fn in_time_window(&self, interaction: &InteractionSummary) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.in_time_window_at(interaction, now_ms)
    }

    fn in_time_window_at(&self, interaction: &InteractionSummary, now_ms: u64) -> bool {
        let window = self.time_filter.window;
        if window.includes_at(interaction, now_ms) {
            return true;
        }
        let root = self.branch_root(&interaction.id);
        self.items.iter().any(|other| {
            other.id != interaction.id
                && self.branch_root(&other.id) == root
                && window.includes_at(other, now_ms)
        })
    }

    pub(crate) fn time_filter_label(&self) -> &'static str {
        self.time_filter.window.label()
    }

    pub(crate) fn cycle_time_filter(&mut self, current: &str, workspace_id: Option<&str>) {
        self.time_filter.window = self.time_filter.window.next();
        self.follow_filter(current, workspace_id);
    }
}

#[cfg(test)]
mod tests;

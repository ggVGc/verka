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

impl TimeFilter {
    pub(super) fn includes(&self, interaction: &InteractionSummary) -> bool {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.window.includes_at(interaction, now_ms)
    }
}

impl LiveInteractions {
    pub(crate) fn time_filter_label(&self) -> &'static str {
        self.time_filter.window.label()
    }

    pub(crate) fn cycle_time_filter(&mut self, current: &str, workspace_id: Option<&str>) {
        self.time_filter.window = self.time_filter.window.next();
        self.follow_filter(current, workspace_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactions::tests::interaction;
    use styra_protocol::InteractionActivity;

    fn shown<'a>(live: &'a LiveInteractions, workspace_id: Option<&str>) -> Vec<&'a str> {
        live.display_indices("", workspace_id)
            .into_iter()
            .map(|index| live.items[index].id.as_str())
            .collect()
    }

    #[test]
    fn time_windows_include_the_boundary_and_use_the_latest_user_message() {
        const DAY: u64 = 86_400_000;
        let now = 20 * DAY;
        let mut item = interaction("1-0-0", InteractionActivity::Pending);
        for (filter, days) in [
            (Window::PastDay, 1),
            (Window::Past3Days, 3),
            (Window::PastWeek, 7),
        ] {
            item.last_user_message_at_ms = now - days * DAY;
            assert!(filter.includes_at(&item, now));
            item.last_user_message_at_ms -= 1;
            assert!(!filter.includes_at(&item, now));
            item.last_user_message_at_ms = now + 1;
            assert!(filter.includes_at(&item, now));
            item.last_user_message_at_ms = 0;
            item.id = format!("{}-0-0", now - days * DAY);
            assert!(filter.includes_at(&item, now));
            item.id = "1-0-0".into();
            assert!(!filter.includes_at(&item, now));
        }
        item.id = "unknown".into();
        assert!(Window::Disabled.includes_at(&item, now));
        assert!(!Window::PastWeek.includes_at(&item, now));
    }

    #[test]
    fn time_filter_cycles_retains_current_and_composes_with_other_filters() {
        let mut live = LiveInteractions::default();
        let mut recent = interaction("recent", InteractionActivity::Pending);
        recent.last_user_message_at_ms = u64::MAX;
        recent.tags = vec!["keep".into()];
        let old = interaction("1-0-0", InteractionActivity::Pending);
        live.open(vec![recent, old], vec![]);
        assert_eq!(live.time_filter.window, Window::Disabled);
        live.move_cursor_to("1-0-0".into(), "recent");
        for filter in [Window::PastDay, Window::Past3Days, Window::PastWeek] {
            live.cycle_time_filter("recent", None);
            assert_eq!(live.time_filter.window, filter);
            assert_eq!(shown(&live, None), ["recent"]);
            assert_eq!(live.cursor("recent"), "recent");
            // The current screen is kept even when it is outside the window.
            assert_eq!(live.visible_indices("1-0-0", None).len(), 2);
        }
        live.close();
        live.show(vec![]);
        assert_eq!(live.time_filter.window, Window::PastWeek);
        live.set_tag_filter(vec!["missing".into()], "", None);
        assert!(shown(&live, None).is_empty());
        live.set_tag_filter(vec![], "", None);
        live.cycle_time_filter("", None);
        assert_eq!(live.time_filter.window, Window::Disabled);
        assert_eq!(shown(&live, None), ["recent", "1-0-0"]);
    }

    #[test]
    fn the_overview_shares_the_time_window() {
        let mut live = LiveInteractions::default();
        let mut recent = interaction("recent", InteractionActivity::Pending);
        recent.last_user_message_at_ms = u64::MAX;
        let old = interaction("1-0-0", InteractionActivity::Pending);
        live.open(vec![recent, old], vec![]);
        assert_eq!(live.overview_indices(None).len(), 2);
        live.cycle_time_filter("1-0-0", None);
        assert_eq!(live.overview_indices(None), [0]);
    }
}

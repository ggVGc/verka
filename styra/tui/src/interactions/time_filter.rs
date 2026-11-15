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

    #[test]
    fn a_branch_group_is_shown_whole_when_any_of_it_is_in_the_window() {
        const DAY: u64 = 86_400_000;
        let now = 20 * DAY;
        let branch = |id: &str, from: &str| {
            let mut item = interaction(id, InteractionActivity::Pending);
            item.origin = Some(styra_protocol::SessionOrigin {
                session_id: from.into(),
                provider: item.selection.provider,
                at_ms: None,
                history: Default::default(),
            });
            item
        };
        let source = interaction("1-0-0", InteractionActivity::Pending);
        let mut recent = branch("2-0-0", "1-0-0");
        recent.last_user_message_at_ms = now;
        let sibling = branch("3-0-0", "1-0-0");
        let nephew = branch("4-0-0", "3-0-0");
        let unrelated = interaction("5-0-0", InteractionActivity::Pending);
        let mut live = LiveInteractions::default();
        live.open(vec![source, recent, sibling, nephew, unrelated], vec![]);
        live.time_filter.window = Window::PastDay;
        fn included(live: &LiveInteractions, now: u64) -> Vec<&str> {
            let mut ids: Vec<&str> = live
                .items
                .iter()
                .filter(|item| live.in_time_window_at(item, now))
                .map(|item| item.id.as_str())
                .collect();
            ids.sort();
            ids
        }
        assert_eq!(included(&live, now), ["1-0-0", "2-0-0", "3-0-0", "4-0-0"]);
        // Siblings stay grouped once their source has left the listing.
        live.items.retain(|item| item.id != "1-0-0");
        assert_eq!(included(&live, now), ["2-0-0", "3-0-0", "4-0-0"]);
        live.items.retain(|item| item.id != "2-0-0");
        assert!(included(&live, now).is_empty());
    }
}

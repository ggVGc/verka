//! Application adapter for provider quota presentation.

use crate::app::App;
use std::time::{SystemTime, UNIX_EPOCH};
use styra_protocol::QuotaEvent;

pub(crate) fn readings(app: &App) -> Vec<&QuotaEvent> {
    app.quota.iter().collect()
}

pub(crate) fn alert(app: &App) -> Vec<styra_ui::footer::Segment> {
    styra_ui::quota::footer_segments(&readings(app), now_ms())
}

/// Whether the quota footer is waiting on a moment rather than on a reading.
/// See [`styra_ui::quota::footer_ticks`]; the caller is
/// [`super::ticking`](super::ticking).
pub(crate) fn ticking(app: &App) -> bool {
    styra_ui::quota::footer_ticks(&readings(app), now_ms())
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;

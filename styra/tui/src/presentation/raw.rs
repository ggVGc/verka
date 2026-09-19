//! Application adapter for Styra-wire and provider-native raw records.

use super::panel_chrome;
use crate::app::App;
pub(crate) fn view(app: &App) -> styra_ui::raw::RawView<'_> {
    let (source, selected, requested_scroll) =
        match (app.provider_raw.as_ref(), app.provider_raw_open) {
            (Some(raw), true) => (
                styra_ui::raw::RawSource::Provider(raw.as_slice()),
                raw.selected_index(),
                raw.preview.offset,
            ),
            _ => (
                styra_ui::raw::RawSource::Styra(app.raw.as_slice()),
                app.raw.selected_index(),
                app.raw.preview.offset,
            ),
        };
    styra_ui::raw::RawView {
        chrome: panel_chrome(app, None),
        source,
        selected,
        requested_preview_scroll: requested_scroll,
    }
}

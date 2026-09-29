//! Mapping from application preview state to its presentation model.

use super::list::ui_link_display;
use crate::app::App;
use crate::preview::PreviewTarget;

pub(crate) fn view(app: &App, fullscreen: bool) -> styra_ui::preview::PreviewView<'_> {
    let file_target = app.highlighted_link_target().map(|(location, path, line)| {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) if content.is_empty() => styra_ui::preview::FileTargetContent::Empty,
            Ok(content) => styra_ui::preview::FileTargetContent::Ready(content),
            Err(error) => styra_ui::preview::FileTargetContent::Failed(error.to_string()),
        };
        styra_ui::preview::FileTarget {
            location,
            content,
            line,
        }
    });
    let entry = app
        .preview_entry()
        .map(|entry| styra_ui::event_list::EventEntry {
            event: entry.event(),
            expanded: entry.expanded,
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: false,
            link_highlight: app
                .link_highlight
                .filter(|highlight| highlight.entry == app.timeline.selected)
                .map(|highlight| highlight.link),
        });
    styra_ui::preview::PreviewView {
        entry,
        protocol: app.selection.provider.protocol(),
        mode: app.preview.mode(),
        target: match app.preview.target() {
            PreviewTarget::Selection => styra_ui::preview::PreviewTarget::Selection,
            PreviewTarget::Command => styra_ui::preview::PreviewTarget::Command,
        },
        links: ui_link_display(app.link_display),
        link_highlight: app
            .link_highlight
            .filter(|highlight| highlight.entry == app.timeline.selected)
            .map(|highlight| highlight.link),
        file_target,
        requested_scroll: app.preview.scroll.offset,
        fullscreen,
    }
}

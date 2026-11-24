//! Mapping from application preview state to its presentation model.

use std::borrow::Cow;

use styra_protocol::agent::SandboxLayout;
use styra_protocol::event::AgentEvent;

use super::list::{
    branch_link, branch_provider_switch, owned_branch_provider_switch, ui_link_display,
};
use crate::app::App;
use crate::preview::PreviewTarget;

pub(crate) fn view(app: &App, fullscreen: bool) -> styra_ui::preview::PreviewView<'_> {
    let file_target = app.highlighted_link_target().map(|(location, path, line)| {
        let content = match path
            .and_then(|path| std::fs::read_to_string(&path).map_err(|error| error.to_string()))
        {
            Ok(content) if content.is_empty() => styra_ui::preview::FileTargetContent::Empty,
            Ok(content) => styra_ui::preview::FileTargetContent::Ready(content),
            Err(problem) => styra_ui::preview::FileTargetContent::Failed(problem),
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
            version: super::list::ui_version(entry.version()),
            expanded: entry.expanded,
            has_detail: entry.has_detail(),
            contract: entry.contract.as_ref(),
            selected: false,
            link_highlight: None,
            branch: branch_link(app, &app.session_id, entry.event()),
            branch_provider_switch: branch_provider_switch(app, entry.event()),
            // The preview shows the entry on its own, wherever it sits.
            branch_follows_marker: false,
            selected_branch: None,
        });
    let branch_log = branch_log(app);
    styra_ui::preview::PreviewView {
        entry,
        entry_change: app
            .preview_entry()
            .and_then(|entry| change_view(app, entry.event())),
        workspace_roots: workspace_roots(app),
        protocol: app.selection.provider.protocol(),
        target: match app.preview.target() {
            PreviewTarget::Selection => styra_ui::preview::PreviewTarget::Selection,
            PreviewTarget::Command => styra_ui::preview::PreviewTarget::Command,
        },
        links: ui_link_display(app.link_display),
        branch_log,
        file_target,
        answer_target: app.highlighted_answer(),
        requested_scroll: app.preview.scroll.offset,
        fullscreen,
    }
}

/// The linked Session's log while the preview is on a `branch` marker. Until
/// the event loop has started loading the marker the cursor just reached, it
/// reads as loading rather than as the log of the one it left.
///
/// It shows what the main interaction view would show of it under the
/// operator's current filters, each entry folded as a fresh one there is,
/// and its markers leading where that Session's own markers lead.
fn branch_log(app: &App) -> Option<styra_ui::preview::BranchLog<'_>> {
    use crate::branch_log::{shown_from, Contents};
    use styra_ui::preview::BranchLog;
    let (target, direction) = app.preview_branch_target()?;
    let Some(log) = app
        .branch_log
        .as_ref()
        .filter(|log| log.session_id() == target)
    else {
        return Some(BranchLog::Loading);
    };
    Some(match log.contents() {
        Contents::Loading => BranchLog::Loading,
        Contents::Ready { entries, protocol } => BranchLog::Ready {
            entries: shown_from(direction, entries)
                .iter()
                .filter(|entry| app.timeline.event_is_visible(&entry.event))
                .map(|entry| {
                    let branch = branch_link(app, target, &entry.event);
                    styra_ui::event_list::EventEntry {
                        event: &entry.event,
                        // Drawn outside the list's row cache, so never compared.
                        version: styra_ui::event_list::EntryVersion { id: 0, revision: 0 },
                        expanded: !app.timeline.all_events,
                        has_detail: match entry.event.branch_target() {
                            Some(_) => branch.message.is_some(),
                            None => crate::timeline::has_detail(&entry.event),
                        },
                        contract: entry.contract.as_ref(),
                        selected: false,
                        link_highlight: None,
                        branch,
                        branch_provider_switch: owned_branch_provider_switch(
                            app,
                            target,
                            &entry.event,
                        ),
                        branch_follows_marker: false,
                        selected_branch: None,
                    }
                })
                .collect(),
            protocol: *protocol,
        },
        Contents::Failed(error) => BranchLog::Failed(error),
    })
}

/// Where the agent's reported paths can place the Workspace: inside its
/// sandbox, or, for a path it gave on this host, the directory backing it.
fn workspace_roots(app: &App) -> Vec<String> {
    let mut roots = vec![SandboxLayout::default().workspace.display().to_string()];
    if let Some(root) = app.workspace.root() {
        let root = root.display().to_string();
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// A file-change event as the preview draws it, or `None` for any other
/// event. A Claude edit snippet is given its position in the file, if it can
/// still be found there, so its lines can be numbered; see
/// [`crate::snippet`].
fn change_view<'a>(app: &App, event: &'a AgentEvent) -> Option<styra_ui::preview::ChangeView<'a>> {
    match event {
        AgentEvent::FileChanged { paths, diff, .. } => Some(styra_ui::preview::ChangeView {
            paths,
            diff: diff.as_deref().map(|diff| placed(app, diff, paths)),
        }),
        AgentEvent::DiffUpdated { diff } => Some(styra_ui::preview::ChangeView {
            paths: &[],
            diff: Some(Cow::Borrowed(diff)),
        }),
        _ => None,
    }
}

pub(crate) fn placed<'a>(app: &App, diff: &'a str, paths: &[String]) -> Cow<'a, str> {
    let [path] = paths else {
        return Cow::Borrowed(diff);
    };
    if !crate::snippet::has_snippets(diff) {
        return Cow::Borrowed(diff);
    }
    let path = match app.workspace.root_or_current_directory() {
        Some(root) => crate::files::resolve(&root, path),
        None => path.into(),
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|file| crate::snippet::place(diff, &file))
        .map_or(Cow::Borrowed(diff), Cow::Owned)
}

#[cfg(test)]
mod tests;

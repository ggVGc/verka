//! Mapping from application preview state to its presentation model.

use std::borrow::Cow;

use styra_protocol::agent::SandboxLayout;
use styra_protocol::event::AgentEvent;

use super::list::{branch_name, branch_provider_switch, ui_link_display};
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
            link_highlight: app
                .link_highlight
                .filter(|highlight| highlight.entry == app.timeline.selected)
                .map(|highlight| highlight.link),
            branch_name: branch_name(app, entry.event()),
            branch_provider_switch: branch_provider_switch(app, entry.event()),
            // The preview is for reading the entry in full, wherever it sits.
            inherited: false,
        });
    let branch_log = branch_log(app);
    styra_ui::preview::PreviewView {
        entry,
        // The side panel shows a message's turn diff beside the list that
        // already shows the message; `P` is for reading the entry itself. A
        // branch marker is read for the Session it leads to instead.
        changes: (!fullscreen && branch_log.is_none())
            .then(|| app.preview_changes())
            .flatten()
            .map(|changes| {
                changes
                    .into_iter()
                    .filter_map(|event| change_view(app, event))
                    .collect()
            }),
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
        link_highlight: app
            .link_highlight
            .filter(|highlight| highlight.entry == app.timeline.selected)
            .map(|highlight| highlight.link),
        branch_log,
        file_target,
        requested_scroll: app.preview.scroll.offset,
        fullscreen,
    }
}

/// The linked Session's log while the preview is on a `branch` marker. Until
/// the event loop has started loading the marker the cursor just reached, it
/// reads as loading rather than as the log of the one it left.
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
        Contents::Ready { events, protocol } => BranchLog::Ready {
            events: shown_from(direction, events),
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

fn placed<'a>(app: &App, diff: &'a str, paths: &[String]) -> Cow<'a, str> {
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
mod tests {
    use super::super::test_support;
    use crate::app::App;

    use styra_protocol::event::AgentEvent;

    fn changed(path: &str, old: &str, new: &str) -> AgentEvent {
        AgentEvent::FileChanged {
            id: String::new(),
            paths: vec![path.into()],
            diff: Some(format!("@@ edit @@\n-{old}\n+{new}")),
            checkpoint: None,
            checkpoint_error: None,
        }
    }

    /// Two turns, each a request and the work done for it, with the preview
    /// open over the conversation-only list.
    fn app_with_two_turns() -> App {
        let mut app = test_support::app("s1");
        app.timeline.all_events = false;
        app.push_event(AgentEvent::UserMessage {
            text: "fix the retry backoff".into(),
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "cargo test backoff".into(),
        });
        app.push_event(changed("src/retry.rs", "delay * 3", "delay * 2"));
        app.push_event(AgentEvent::UserMessage {
            text: "and rename the helper".into(),
        });
        app.push_event(changed("src/helper.rs", "fn old_name", "fn new_name"));
        app.select_first();
        app.preview.show();
        app
    }

    /// A conversation line's text is already on the list; what the preview
    /// adds is the work done for it, which the list is hiding.
    #[test]
    fn a_selected_message_previews_the_diff_of_its_turn() {
        let app = app_with_two_turns();
        let screen = test_support::screen_sized(&app, 120, 30);
        let (preview_x, _) = screen.find("turn diff · C: command");
        let (diff_x, _) = screen.find("delay * 2");
        assert!(diff_x > preview_x, "the diff is in the preview pane");
        assert!(screen.all().contains("src/retry.rs"));
        // The next message's work belongs to its own turn.
        assert!(!screen.all().contains("new_name"));
    }

    #[test]
    fn the_preview_moves_to_the_next_turns_diff_with_the_selection() {
        let mut app = app_with_two_turns();
        app.select_next_line();
        let screen = test_support::rendered(&app);
        assert!(screen.contains("new_name"));
        assert!(!screen.contains("delay * 2"));
    }

    #[test]
    fn a_turn_without_file_changes_says_so() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "what does this do".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "it retries".into(),
        });
        app.select_first();
        app.preview.show();
        assert!(test_support::rendered(&app).contains("no file changes during this turn"));
    }

    /// Codex follows every file-change item with the turn's whole diff so
    /// far; the newest snapshot covers the items, so they are not repeated.
    #[test]
    fn a_turn_diff_snapshot_stands_for_the_file_changes_before_it() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix it".into(),
        });
        app.push_event(changed("src/a.rs", "item-old", "item-new"));
        app.push_event(AgentEvent::DiffUpdated {
            diff: "diff --git a/src/a.rs b/src/a.rs\n@@\n-first-old\n+first-new\n".into(),
        });
        app.push_event(AgentEvent::DiffUpdated {
            diff: "diff --git a/src/a.rs b/src/a.rs\n@@\n-latest-old\n+latest-new\n".into(),
        });
        app.select_first();
        app.preview.show();
        let changes = app.preview_changes().unwrap();
        assert_eq!(changes.len(), 1);
        let screen = test_support::rendered(&app);
        assert!(screen.contains("latest-new"));
        assert!(!screen.contains("item-new"));
        assert!(!screen.contains("first-new"));
    }

    /// Claude's edit snippets say what changed but not where; the file still
    /// holds the new text, so the preview finds it there and numbers it.
    #[test]
    fn a_claude_edit_is_numbered_from_where_it_sits_in_the_file() {
        let dir = std::env::temp_dir().join(format!("styra-snippet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("retry.rs");
        std::fs::write(&file, "fn a() {}\nfn b() {}\nlet delay = base * 2;\n").unwrap();

        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix the retry backoff".into(),
        });
        app.push_event(AgentEvent::FileChanged {
            id: String::new(),
            paths: vec![file.display().to_string()],
            diff: Some("@@ edit @@\n-let delay = base * 3;\n+let delay = base * 2;".into()),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.select_first();
        app.preview.show();

        let screen = test_support::screen_sized(&app, 160, 30).all();
        assert!(screen.contains("3 +let delay = base * 2;"), "{screen}");
        assert!(screen.contains("3 -let delay = base * 3;"), "{screen}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `P` is for reading the selected entry at full size, so a message there
    /// is its own text rather than its turn's diff.
    #[test]
    fn the_full_screen_preview_shows_a_message_itself() {
        let mut app = app_with_two_turns();
        app.view = crate::app::View::Preview;
        let screen = test_support::rendered(&app);
        assert!(screen.contains("fix the retry backoff"), "{screen}");
        assert!(!screen.contains("delay * 2"));
        assert!(!screen.contains("no file changes during this turn"));
    }

    /// The Workspace root says nothing that differs between changed files,
    /// so it is shortened and the rest of the path is what reads.
    #[test]
    fn a_changed_path_in_the_workspace_is_shown_from_the_workspace_on() {
        let workspace = styra_protocol::agent::SandboxLayout::default().workspace;
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix the retry backoff".into(),
        });
        app.push_event(changed(
            &workspace.join("src/retry.rs").display().to_string(),
            "delay * 3",
            "delay * 2",
        ));
        app.select_first();
        app.preview.show();
        let screen = test_support::screen_sized(&app, 120, 30);
        let (preview_x, _) = screen.find("turn diff · C: command");
        let (path_x, _) = screen.find("ws:src/retry.rs");
        assert!(
            path_x > preview_x,
            "the shortened path is in the preview pane"
        );
    }

    /// Any other entry is its own content, as before.
    #[test]
    fn a_work_entry_still_previews_as_itself() {
        let mut app = app_with_two_turns();
        app.timeline.all_events = true;
        app.select_next_line();
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("shell")
        );
        assert!(app.preview_changes().is_none());
        assert!(test_support::rendered(&app).contains("preview · C: command"));
    }
}

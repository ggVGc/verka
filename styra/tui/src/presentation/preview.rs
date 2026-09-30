//! Mapping from application preview state to its presentation model.

use std::borrow::Cow;

use styra_protocol::event::AgentEvent;

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
            version: super::list::ui_version(entry.version()),
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
        changes: app.preview_changes().map(|changes| {
            changes
                .into_iter()
                .filter_map(|event| change_view(app, event))
                .collect()
        }),
        entry_change: app
            .preview_entry()
            .and_then(|entry| change_view(app, entry.event())),
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
        app.timeline.conversation_only = true;
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
        let (preview_x, _) = screen.find("turn diff · pretty");
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

    /// Any other entry is its own content, as before.
    #[test]
    fn a_work_entry_still_previews_as_itself() {
        let mut app = app_with_two_turns();
        app.timeline.conversation_only = false;
        app.select_next_line();
        assert_eq!(
            app.preview_entry().map(|entry| entry.event().tag()),
            Some("shell")
        );
        assert!(app.preview_changes().is_none());
        assert!(test_support::rendered(&app).contains("preview · pretty"));
    }
}

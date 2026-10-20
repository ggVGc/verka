//! Mapping from application preview state to its presentation model.

use std::borrow::Cow;

use styra_protocol::agent::SandboxLayout;
use styra_protocol::event::AgentEvent;

use super::list::{branch_link, branch_provider_switch, ui_link_display};
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
            branch: branch_link(app, entry.event()),
            branch_provider_switch: branch_provider_switch(app, entry.event()),
            // The preview is for reading the entry in full, wherever it sits.
            inherited: false,
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
        link_highlight: app
            .link_highlight
            .filter(|highlight| highlight.entry == app.timeline.selected)
            .map(|highlight| highlight.link),
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
mod tests {
    use super::super::test_support;
    use crate::app::App;

    use styra_protocol::event::AgentEvent;

    #[test]
    fn highlighted_answers_preview_the_selected_section_even_when_following_commands() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: styra_protocol::contract::frame("show me", styra_protocol::Contract::Lines),
        });
        app.push_event(AgentEvent::CommandStarted {
            command: "echo waiting".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "Before\n<styra:answer>\nfirst answer\n</styra:answer>\nBetween\n<styra:answer>\nsecond answer\n</styra:answer>\n[web](https://example.com)".into(),
        });
        app.preview.show();
        app.preview.toggle_target();
        app.highlight_first_link();

        let screen = test_support::screen_sized(&app, 120, 30).all();
        assert!(screen.contains("answer · lines"), "{screen}");
        assert!(screen.contains("first answer"), "{screen}");
        assert!(!screen.contains("second answer"), "{screen}");

        app.highlight_next_link();
        let screen = test_support::screen_sized(&app, 120, 30).all();
        assert!(screen.contains("second answer"), "{screen}");
        assert!(!screen.contains("first answer"), "{screen}");

        app.highlight_next_link();
        assert!(super::view(&app, false).answer_target.is_none());
        app.highlight_prev_link();
        assert!(super::view(&app, false).answer_target.is_some());
        app.clear_link_highlight();
        assert!(super::view(&app, false).answer_target.is_none());
        assert!(app.preview.open);
        assert!(app.preview.follows_command());
    }

    #[test]
    fn a_highlighted_file_link_opens_at_its_line_and_pages_through_the_file() {
        use super::super::{apply_feedback, draw_application};
        let root = std::env::temp_dir().join("styra-preview-file-scroll");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = (1..=200)
            .map(|n| format!("row {n:03}\n"))
            .collect::<String>();
        std::fs::write(root.join("long.txt"), file).unwrap();
        let mut app = test_support::app("s1");
        app.workspace.enter(root.clone());
        app.push_event(AgentEvent::AgentMessage {
            text: "[long](long.txt:100)".into(),
        });
        app.preview.show();
        app.highlight_first_link();
        let mut ui = styra_ui::TestUi::new(120, 30).unwrap();
        let mut draw = |app: &mut App| {
            let feedback = draw_application(&mut ui, app).unwrap();
            apply_feedback(app, &feedback);
            ui.rows().concat()
        };

        let screen = draw(&mut app);
        assert!(screen.contains("row 100"), "{screen}");
        assert!(!screen.contains("row 001"), "{screen}");
        let opened = app.preview.scroll.offset;
        assert!(opened > 0);

        app.preview.scroll.half_page_down();
        let screen = draw(&mut app);
        assert!(app.preview.scroll.offset > opened);
        assert!(!screen.contains("row 100"), "{screen}");

        app.preview.scroll.scroll_to_end();
        let screen = draw(&mut app);
        assert!(screen.contains("row 200"), "{screen}");

        app.clear_link_highlight();
        assert_eq!(app.preview.scroll.offset, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

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

    /// A message previews as itself: its turn's diff is the diff panel's to
    /// show (see [`crate::turn_diff`]).
    #[test]
    fn a_selected_message_previews_as_itself() {
        let app = app_with_two_turns();
        let screen = test_support::screen_sized(&app, 120, 30).all();
        assert!(screen.contains("preview · C: command"), "{screen}");
        assert!(!screen.contains("turn diff"), "{screen}");
        assert!(!screen.contains("delay * 2"), "{screen}");
    }

    /// A file change is its own diff, its path from the Workspace on, and a
    /// Claude edit numbered from where it sits in the file.
    #[test]
    fn a_file_change_previews_its_own_diff() {
        let dir = std::env::temp_dir().join(format!("styra-snippet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("retry.rs");
        std::fs::write(&file, "fn a() {}\nfn b() {}\nlet delay = base * 2;\n").unwrap();
        let workspace = styra_protocol::agent::SandboxLayout::default().workspace;

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
        app.push_event(changed(
            &workspace.join("src/helper.rs").display().to_string(),
            "fn old_name",
            "fn new_name",
        ));
        app.select_first();
        app.select_next_line();
        app.preview.show();
        let screen = test_support::screen_sized(&app, 160, 30).all();
        assert!(screen.contains("3 +let delay = base * 2;"), "{screen}");
        assert!(screen.contains("3 -let delay = base * 3;"), "{screen}");

        app.select_next_line();
        let screen = test_support::screen_sized(&app, 160, 30).all();
        assert!(screen.contains("ws:src/helper.rs"), "{screen}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

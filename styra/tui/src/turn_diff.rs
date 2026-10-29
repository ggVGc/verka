//! The diff view's turn diff: what the selected message's turn changed, shown
//! by `styra-diff` as a supplied diff in place of the checkout's whole diff
//! while `u` has it so, in the side panel or fullscreen. `H` uses the same
//! turn patch to highlight matching changes within the checkout diff.
//!
//! The turn is the stretch of the conversation the selection is in (see
//! [`crate::timeline::Timeline::conversation_span`]). Where the server took
//! snapshots on both sides of it, the diff is theirs (see
//! [`crate::turn_snapshot`]); otherwise it is the file changes the agent
//! reported making, given Git's file headers where they have none so the
//! view can list them file by file.

use std::path::{Path, PathBuf};

use styra_diff::{CheckoutDiff, Source, Supplied};
use styra_protocol::agent::SandboxLayout;
use styra_protocol::event::AgentEvent;

use crate::app::App;

/// The turn diff the panel shows, and the turn it is of, so moving to
/// another turn starts that turn's diff from its top.
pub struct TurnDiffPanel {
    pub(crate) turn: Option<u64>,
    pub diff: CheckoutDiff,
}

impl App {
    /// Whether a diff is on screen — fullscreen or in the side panel —
    /// showing the turn diff.
    pub fn turn_diff_shown(&self) -> bool {
        self.diff_turn && self.diff_visible()
    }

    /// The diff on screen: the turn diff while it is shown, otherwise the
    /// checkout's.
    pub fn shown_diff(&self) -> Option<&CheckoutDiff> {
        if self.turn_diff_shown() {
            self.turn_diff.as_ref().map(|panel| &panel.diff)
        } else {
            self.checkout_diff.as_ref()
        }
    }

    pub fn shown_diff_mut(&mut self) -> Option<&mut CheckoutDiff> {
        if self.turn_diff_shown() {
            self.turn_diff.as_mut().map(|panel| &mut panel.diff)
        } else {
            self.checkout_diff.as_mut()
        }
    }

    /// The turn the selection is in, counting the operator messages from 1
    /// up to its stretch's, as the server counts them; `None` before the
    /// first message.
    pub(crate) fn selected_turn(&self) -> Option<u64> {
        if self.timeline.entries.is_empty() {
            return None;
        }
        let start = self.timeline.conversation_span().start;
        let turn = self.timeline.entries[..=start]
            .iter()
            .filter(|entry| is_operator_message(entry.event()))
            .count() as u64;
        (turn > 0).then_some(turn)
    }

    /// The message that started the selected turn, which the panel shows
    /// above its diff.
    pub(crate) fn turn_message(&self) -> Option<&AgentEvent> {
        self.selected_turn()?;
        let start = self.timeline.conversation_span().start;
        self.timeline.entries[..=start]
            .iter()
            .rev()
            .map(|entry| entry.event())
            .find(|event| is_operator_message(event))
    }

    /// Bring the turn diff (also used for full-diff highlights) up to date
    /// with the selection and what has been
    /// read of its snapshots, keeping the view where it is while the turn
    /// stays the same. Whether anything on screen changed.
    pub(crate) fn sync_turn_diff(&mut self) -> bool {
        if !self.turn_diff_shown() && !(self.diff_turn_highlight && self.diff_visible()) {
            return false;
        }
        let turn = self.selected_turn();
        let (diff, supplied) = self.turn_diff_contents(turn);
        match &mut self.turn_diff {
            Some(panel) if panel.turn == turn => {
                let relabelled = panel.diff.source != Source::Supplied(supplied.clone());
                panel.diff.source = Source::Supplied(supplied);
                panel.diff.replace(diff) | relabelled
            }
            Some(panel) => {
                panel.turn = turn;
                panel.diff.source = Source::Supplied(supplied);
                panel.diff.replace(diff);
                panel.diff.selected_file = 0;
                panel.diff.file_list_offset = 0;
                panel.diff.scroll.reset();
                true
            }
            None => {
                let mut diff = match diff {
                    Ok(text) => CheckoutDiff::supplied(text, supplied),
                    Err(error) => {
                        let mut diff = CheckoutDiff::supplied(String::new(), supplied);
                        diff.diff = Err(error);
                        diff
                    }
                };
                // Laid out as the checkout's diff is, so switching between
                // the two keeps the panel looking the same.
                if let Some(full) = &self.checkout_diff {
                    diff.per_file = full.per_file;
                    diff.side_by_side = full.side_by_side;
                    diff.hide_removed = full.hide_removed;
                    diff.compact_dirs = full.compact_dirs;
                    diff.tint = full.tint;
                    diff.word_highlights = full.word_highlights;
                }
                self.turn_diff = Some(TurnDiffPanel { turn, diff });
                true
            }
        }
    }

    /// The selected turn's diff, and what to call it: its snapshots' diff
    /// once read, otherwise what the agent reported changing.
    fn turn_diff_contents(&self, turn: Option<u64>) -> (Result<String, String>, Supplied) {
        let workspace = self.workspace.root().map(Path::to_path_buf);
        let Some(turn) = turn else {
            return (
                Err("no turn selected: the turn diff is of the selected message's turn".into()),
                Supplied {
                    label: "no turn".into(),
                    root: workspace,
                },
            );
        };
        if let Some(snapshot) = self.turn_snapshot_diff() {
            let checkout = self
                .interactions
                .current(&self.session_id)
                .and_then(|interaction| interaction.checkout.as_ref())
                .map(|checkout| checkout.worktree.clone());
            let label = if snapshot.ended {
                format!("turn {turn} · from its snapshots")
            } else {
                format!("turn {turn} · from its snapshots, so far")
            };
            return (
                Ok(snapshot.diff.clone()),
                Supplied {
                    label,
                    root: checkout.or(workspace),
                },
            );
        }
        let changes = self.timeline.conversation_changes();
        (
            Ok(patch(self, &changes)),
            Supplied {
                label: format!("turn {turn} · as the agent reported its changes"),
                root: workspace,
            },
        )
    }
}

fn is_operator_message(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::UserMessage { .. })
}

/// The agent's reported changes as one Git-style patch. A diff that already
/// has Git's file headers, as Codex's whole-turn diff does, is kept as it is;
/// any other is given a header naming its file, from the Workspace on, and
/// a Claude edit snippet its place in the file where it can still be found.
fn patch(app: &App, changes: &[&AgentEvent]) -> String {
    let roots = workspace_roots(app);
    let mut text = String::new();
    for event in changes {
        let (paths, diff) = match event {
            AgentEvent::FileChanged { paths, diff, .. } => (paths.as_slice(), diff.as_deref()),
            AgentEvent::DiffUpdated { diff } => (&[][..], Some(diff.as_str())),
            _ => continue,
        };
        let diff = diff.map(|diff| crate::presentation::preview::placed(app, diff, paths));
        match diff.as_deref() {
            Some(diff) if diff.lines().any(|line| line.starts_with("diff --git ")) => {
                push(&mut text, diff)
            }
            _ => {
                for (index, path) in paths.iter().enumerate() {
                    let path = relative(path, &roots);
                    let body = diff.as_deref().filter(|_| index == 0).unwrap_or_default();
                    text.push_str(&format!("diff --git a/{path} b/{path}\n"));
                    if !body.lines().any(|line| line.starts_with("--- ")) {
                        text.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
                    }
                    push(&mut text, body);
                }
            }
        }
    }
    text
}

fn push(text: &mut String, diff: &str) {
    text.push_str(diff);
    if !diff.is_empty() && !diff.ends_with('\n') {
        text.push('\n');
    }
}

/// Where the agent's reported paths can place the Workspace: inside its
/// sandbox, or, for a path it gave on this host, the directory backing it.
fn workspace_roots(app: &App) -> Vec<PathBuf> {
    let mut roots = vec![SandboxLayout::default().workspace];
    if let Some(root) = app.workspace.root() {
        if !roots.iter().any(|known| known == root) {
            roots.push(root.to_path_buf());
        }
    }
    roots
}

/// `path` from the Workspace on, where it is in it; otherwise as reported.
fn relative(path: &str, roots: &[PathBuf]) -> String {
    roots
        .iter()
        .find_map(|root| Path::new(path).strip_prefix(root).ok())
        .map_or_else(|| path.to_owned(), |rest| rest.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::test_support;

    fn changed(path: &str, old: &str, new: &str) -> AgentEvent {
        AgentEvent::FileChanged {
            id: String::new(),
            paths: vec![path.into()],
            diff: Some(format!("@@ edit @@\n-{old}\n+{new}")),
            checkpoint: None,
            checkpoint_error: None,
        }
    }

    /// Two turns, each a request and the work done for it, with the side
    /// diff panel showing the turn diff.
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
        app.diff_preview = true;
        app.diff_turn = true;
        app.sync_turn_diff();
        app
    }

    #[test]
    fn highlighting_follows_the_turn_while_keeping_the_full_diff_and_scroll() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = app_with_two_turns();
        let full = "diff --git a/src/retry.rs b/src/retry.rs\n@@ -1 +1 @@\n-delay * 3\n+delay * 2\ndiff --git a/src/helper.rs b/src/helper.rs\n@@ -1 +1 @@\n-fn old_name\n+fn new_name\n";
        app.checkout_diff = Some(CheckoutDiff::supplied(
            full.into(),
            Supplied {
                label: "full diff".into(),
                root: None,
            },
        ));
        app.checkout_diff.as_mut().unwrap().scroll.offset = 3;
        crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Char('H')));
        assert!(app.diff_turn_highlight);
        assert!(!app.turn_diff_shown());
        assert_eq!(shown(&app), full);
        assert_eq!(app.shown_diff().unwrap().scroll.offset, 3);
        assert!(app
            .turn_diff
            .as_ref()
            .unwrap()
            .diff
            .diff
            .as_ref()
            .unwrap()
            .contains("+delay * 2"));
        app.select_next_line();
        assert!(app.sync_turn_diff());
        assert_eq!(app.turn_diff.as_ref().unwrap().turn, Some(2));
        assert!(app
            .turn_diff
            .as_ref()
            .unwrap()
            .diff
            .diff
            .as_ref()
            .unwrap()
            .contains("+fn new_name"));
        assert_eq!(shown(&app), full);
        crate::input::handle_diff_key(&mut app, KeyEvent::from(KeyCode::Char('H')));
        assert!(!app.diff_turn_highlight);
        assert_eq!(shown(&app), full);
    }

    fn shown(app: &App) -> &str {
        app.shown_diff().unwrap().diff.as_deref().unwrap()
    }

    /// Reported changes are given the file headers the view lists files by,
    /// and the turn's message goes above them.
    #[test]
    fn the_turn_diff_is_the_selected_turns_reported_changes() {
        let app = app_with_two_turns();
        assert_eq!(app.selected_turn(), Some(1));
        let files = styra_diff::files::file_diffs(shown(&app));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/retry.rs");
        assert!(shown(&app).contains("+delay * 2"));
        assert!(
            matches!(app.turn_message(), Some(AgentEvent::UserMessage { text }) if text == "fix the retry backoff")
        );
        let Source::Supplied(supplied) = &app.shown_diff().unwrap().source else {
            panic!()
        };
        assert_eq!(supplied.label, "turn 1 · as the agent reported its changes");
        assert!(!shown(&app).contains("new_name"));
    }

    /// Moving to the next turn starts its diff from the top, with the view's
    /// toggles kept.
    #[test]
    fn the_turn_diff_follows_the_selection() {
        let mut app = app_with_two_turns();
        let panel = app.shown_diff_mut().unwrap();
        panel.per_file = true;
        panel.scroll.offset = 2;
        app.select_next_line();
        assert!(app.sync_turn_diff());
        assert_eq!(app.selected_turn(), Some(2));
        assert!(shown(&app).contains("+fn new_name"));
        let panel = app.shown_diff().unwrap();
        assert!(panel.per_file);
        assert_eq!(panel.scroll.offset, 0);
        assert!(!app.sync_turn_diff(), "nothing changed since");
    }

    /// Once its snapshots are read, a turn is shown as they say, and called
    /// so; a turn still running says it is only so far.
    #[test]
    fn a_snapshotted_turn_shows_its_snapshot_diff() {
        let mut app = app_with_two_turns();
        app.select_last();
        for (ended, label) in [
            (false, "turn 2 · from its snapshots, so far"),
            (true, "turn 2 · from its snapshots"),
        ] {
            app.turn_snapshot = Some(crate::turn_snapshot::TurnSnapshot::loaded_as(
                "s1",
                2,
                Some(
                    "diff --git a/src/helper.rs b/src/helper.rs\n@@ -1 +1 @@\n-fn old_name\n+fn snapshot_name\n",
                ),
                ended,
            ));
            assert!(app.sync_turn_diff());
            assert!(shown(&app).contains("snapshot_name"));
            assert!(!shown(&app).contains("new_name\n"));
            let Source::Supplied(supplied) = &app.shown_diff().unwrap().source else {
                panic!()
            };
            assert_eq!(supplied.label, label);
        }
    }

    /// Snapshots that agree say the turn changed nothing, whatever the agent
    /// reported.
    #[test]
    fn a_snapshotted_turn_that_changed_nothing_says_so() {
        let mut app = app_with_two_turns();
        app.turn_snapshot = Some(crate::turn_snapshot::TurnSnapshot::loaded(
            "s1",
            1,
            Some(""),
        ));
        app.sync_turn_diff();
        assert_eq!(shown(&app), "");
        test_support::inspect(&app, |v| {
            assert!(matches!(v.main, styra_ui::application::MainView::Events(_)))
        });
    }

    /// Codex's whole-turn diff already names its files, and the newest one
    /// stands for the items before it.
    #[test]
    fn a_reported_turn_diff_is_kept_as_it_is() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::UserMessage {
            text: "fix it".into(),
        });
        app.push_event(changed("src/a.rs", "item-old", "item-new"));
        app.push_event(AgentEvent::DiffUpdated {
            diff: "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-first-old\n+first-new\n".into(),
        });
        let latest = "diff --git a/src/a.rs b/src/a.rs\n@@ -1 +1 @@\n-latest-old\n+latest-new\n";
        app.push_event(AgentEvent::DiffUpdated {
            diff: latest.into(),
        });
        app.select_first();
        app.diff_preview = true;
        app.diff_turn = true;
        app.sync_turn_diff();
        assert_eq!(shown(&app), latest);
    }

    /// A path in the Workspace is named from the Workspace on; one elsewhere
    /// as reported. A change reported without a diff is still listed.
    #[test]
    fn reported_paths_are_named_from_the_workspace_on() {
        let workspace = SandboxLayout::default().workspace;
        let app = test_support::app("s1");
        let inside = changed(
            &workspace.join("src/retry.rs").display().to_string(),
            "a",
            "b",
        );
        let bare = AgentEvent::FileChanged {
            id: String::new(),
            paths: vec!["/etc/hosts".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        };
        let text = patch(&app, &[&inside, &bare]);
        let files = styra_diff::files::file_diffs(&text);
        let paths: Vec<_> = files.iter().map(|file| file.path).collect();
        assert_eq!(paths, ["src/retry.rs", "/etc/hosts"]);
    }

    /// Before the first message there is no turn to show.
    #[test]
    fn before_any_message_there_is_no_turn() {
        let mut app = test_support::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.select_first();
        app.diff_preview = true;
        app.diff_turn = true;
        app.sync_turn_diff();
        assert!(app.shown_diff().unwrap().diff.is_err());
        assert!(app.turn_message().is_none());
    }
}

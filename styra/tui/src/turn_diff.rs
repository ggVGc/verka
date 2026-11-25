//! The diff view's turn diff: what the selected message's turn changed, shown
//! by `styra-diff` as a supplied diff in place of the checkout's whole diff
//! while `u` has it so, in the diff pane. `H` uses the same
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
    /// Whether the diff pane is on screen showing the turn diff.
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
                    diff.ultra_compact = full.ultra_compact;
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
mod tests;

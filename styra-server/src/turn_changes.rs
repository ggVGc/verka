//! Durable workspace snapshots and turn-scoped diffs.
//!
//! The provider is deliberately not the source of truth here. A private Git
//! index snapshots the host workspace immediately before a message is sent and
//! again when its turn ends, so writes made by shells, formatters, and other
//! subprocesses are observed even when no provider file-change event exists.

use crate::protocol::{TurnChangeStatus, TurnChanges, TurnFileChange};
use anyhow::{bail, Context, Result};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const JOURNAL: &str = "turn-changes.v1.jsonl";
const INDEX: &str = "turn-snapshots.index";

#[derive(Clone, Debug)]
struct ActiveTurn {
    turn: u64,
    before: Result<String, String>,
}

/// Serial turn recorder owned by one live Interaction.
pub struct Tracker {
    workspace: PathBuf,
    session_path: PathBuf,
    reference: String,
    next_turn: u64,
    active: Option<ActiveTurn>,
}

impl Tracker {
    pub fn open(
        workspace: PathBuf,
        session_path: PathBuf,
        session_id: &str,
        completed_user_messages: u64,
    ) -> Self {
        let safe_id: String = session_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            workspace,
            session_path,
            reference: format!("refs/styra/turn-changes/{safe_id}"),
            next_turn: completed_user_messages + 1,
            active: None,
        }
    }

    /// Capture the state before a message reaches the provider. Snapshot
    /// failure does not reject the message; it becomes visible evidence on the
    /// completed turn instead.
    pub fn begin(&mut self) {
        let turn = self.next_turn;
        self.next_turn += 1;
        let before = self.snapshot(None).map_err(|error| format!("{error:#}"));
        self.active = Some(ActiveTurn { turn, before });
    }

    pub fn finish(&mut self, complete: bool) -> Option<TurnChanges> {
        let active = self.active.take()?;
        let changes = match active.before {
            Ok(before) => match self.snapshot(Some(&before)) {
                Ok(after) => match self.diff(&before, &after) {
                    Ok((files, diff)) => TurnChanges {
                        turn: active.turn,
                        status: if complete {
                            TurnChangeStatus::Complete
                        } else {
                            TurnChangeStatus::Partial
                        },
                        files,
                        diff,
                        error: None,
                    },
                    Err(error) => unavailable(active.turn, error),
                },
                Err(error) => unavailable(active.turn, error),
            },
            Err(error) => TurnChanges {
                turn: active.turn,
                status: TurnChangeStatus::Unavailable,
                files: Vec::new(),
                diff: String::new(),
                error: Some(error),
            },
        };
        let _ = append(&self.session_path, &changes);
        Some(changes)
    }

    fn snapshot(&self, parent: Option<&str>) -> Result<String> {
        let top = run(&self.workspace, None, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim());
        if top.canonicalize()? != self.workspace.canonicalize()? {
            bail!(
                "workspace {} is not a Git worktree root",
                self.workspace.display()
            );
        }
        let index = self.session_path.join(INDEX);
        run(&top, Some(&index), &["read-tree", "HEAD"])?;
        run(&top, Some(&index), &["add", "-A", "--", "."])?;
        let tree = run(&top, Some(&index), &["write-tree"])?;
        let tree = tree.trim();
        let mut args = vec!["commit-tree", tree, "-m", "Styra turn snapshot"];
        if let Some(parent) = parent {
            args.extend(["-p", parent]);
        }
        let commit = run(&top, Some(&index), &args)?;
        let commit = commit.trim().to_owned();
        run(
            &top,
            Some(&index),
            &["update-ref", &self.reference, &commit],
        )?;
        Ok(commit)
    }

    fn diff(&self, before: &str, after: &str) -> Result<(Vec<TurnFileChange>, String)> {
        let names = run_bytes(
            &self.workspace,
            None,
            &[
                "diff",
                "--name-status",
                "-z",
                "--find-renames",
                before,
                after,
                "--",
            ],
        )?;
        let fields: Vec<&[u8]> = names
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .collect();
        let mut files = Vec::new();
        let mut cursor = 0;
        while cursor < fields.len() {
            let status = String::from_utf8_lossy(fields[cursor]).into_owned();
            cursor += 1;
            if status.starts_with('R') || status.starts_with('C') {
                if cursor + 1 >= fields.len() {
                    bail!("malformed Git rename status output");
                }
                let old_path = String::from_utf8_lossy(fields[cursor]).into_owned();
                let path = String::from_utf8_lossy(fields[cursor + 1]).into_owned();
                cursor += 2;
                files.push(TurnFileChange {
                    path,
                    status: status.chars().next().unwrap_or('R').to_string(),
                    old_path: Some(old_path),
                });
            } else {
                if cursor >= fields.len() {
                    bail!("malformed Git name-status output");
                }
                let path = String::from_utf8_lossy(fields[cursor]).into_owned();
                cursor += 1;
                files.push(TurnFileChange {
                    path,
                    status: status.chars().next().unwrap_or('M').to_string(),
                    old_path: None,
                });
            }
        }
        let diff = run_bytes(
            &self.workspace,
            None,
            &["diff", "--binary", "--find-renames", before, after, "--"],
        )?;
        Ok((files, String::from_utf8_lossy(&diff).into_owned()))
    }
}

fn unavailable(turn: u64, error: anyhow::Error) -> TurnChanges {
    TurnChanges {
        turn,
        status: TurnChangeStatus::Unavailable,
        files: Vec::new(),
        diff: String::new(),
        error: Some(format!("{error:#}")),
    }
}

fn run(cwd: &Path, index: Option<&Path>, args: &[&str]) -> Result<String> {
    let bytes = run_bytes(cwd, index, args)?;
    String::from_utf8(bytes).context("Git returned non-UTF-8 output")
}

fn run_bytes(cwd: &Path, index: Option<&Path>, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.current_dir(cwd).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
        .env("GIT_AUTHOR_NAME", "Styra")
        .env("GIT_AUTHOR_EMAIL", "styra@localhost")
        .env("GIT_COMMITTER_NAME", "Styra")
        .env("GIT_COMMITTER_EMAIL", "styra@localhost");
    let output = command
        .output()
        .context("running Git for a turn snapshot")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn append(session_path: &Path, changes: &TurnChanges) -> Result<()> {
    let path = session_path.join(JOURNAL);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    serde_json::to_writer(&mut file, changes)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

pub fn read(session_path: &Path) -> Result<Vec<TurnChanges>> {
    let path = session_path.join(JOURNAL);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(&path)?;
    BufReader::new(file)
        .lines()
        .filter(|line| line.as_ref().map_or(true, |line| !line.trim().is_empty()))
        .map(|line| {
            let line = line?;
            serde_json::from_str(&line).context("parsing turn-change journal")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_shell_style_modifications_additions_and_deletions() {
        let base = std::env::temp_dir().join(format!(
            "styra-turn-changes-{}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ));
        let root = base.join("workspace");
        let session = base.join("session");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&session).unwrap();
        run(&root, None, &["init"]).unwrap();
        std::fs::write(root.join("changed.txt"), "old\n").unwrap();
        std::fs::write(root.join("deleted.txt"), "gone\n").unwrap();
        run(&root, None, &["add", "."]).unwrap();
        run(&root, None, &["commit", "-m", "initial"]).unwrap();

        let mut tracker = Tracker::open(root.clone(), session.clone(), "s1", 0);
        tracker.begin();
        // These direct writes stand for arbitrary shell subprocess activity.
        std::fs::write(root.join("changed.txt"), "new\n").unwrap();
        std::fs::write(root.join("added.txt"), "added\n").unwrap();
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        let changes = tracker.finish(true).unwrap();

        assert_eq!(changes.status, TurnChangeStatus::Complete);
        assert!(changes
            .files
            .iter()
            .any(|file| file.path == "changed.txt" && file.status == "M"));
        assert!(changes
            .files
            .iter()
            .any(|file| file.path == "added.txt" && file.status == "A"));
        assert!(changes
            .files
            .iter()
            .any(|file| file.path == "deleted.txt" && file.status == "D"));
        assert!(changes.diff.contains("-old"));
        assert!(changes.diff.contains("+new"));
        assert_eq!(read(&session).unwrap(), vec![changes]);
        let _ = std::fs::remove_dir_all(base);
    }
}

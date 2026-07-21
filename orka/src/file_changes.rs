//! Durable Git checkpoints for model `file_change` events.
//!
//! Checkpoints use a private index and ref. They never move the execution
//! worktree's HEAD or touch its index, so Linka can still capture only the
//! outputs declared by the agent when the attempt settles.

use crate::events::{decode_codex_line, AgentEvent};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub const FILE_CHANGE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChangeCheckpoint {
    pub schema: u32,
    pub sequence: u64,
    pub event_id: String,
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tails a raw Codex journal and checkpoints every completed file-change
/// event. Finishing drains all complete records before joining the worker.
pub struct FileChangeRecorder {
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl FileChangeRecorder {
    pub fn start(
        workspace: &Path,
        isolated_workspace: &Path,
        raw_events: &Path,
        journal: &Path,
        reference: &str,
    ) -> Result<Self> {
        std::fs::write(journal, b"")
            .with_context(|| format!("creating file-change journal {}", journal.display()))?;
        let top = PathBuf::from(checked(workspace, &["rev-parse", "--show-toplevel"])?);
        if top.canonicalize()? != workspace.canonicalize()? {
            bail!(
                "checkpoint workspace {} is not a Git worktree root",
                workspace.display()
            );
        }
        let parent = checked(workspace, &["rev-parse", "HEAD"])?;
        checked(workspace, &["check-ref-format", reference])?;
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();
        let workspace = workspace.to_path_buf();
        let isolated_workspace = isolated_workspace.to_path_buf();
        let raw_events = raw_events.to_path_buf();
        let journal = journal.to_path_buf();
        let reference = reference.to_string();
        let thread = std::thread::spawn(move || {
            follow_and_checkpoint(
                &workspace,
                &isolated_workspace,
                &raw_events,
                &journal,
                &reference,
                &parent,
                &worker_done,
            )
        });
        Ok(Self {
            done,
            thread: Some(thread),
        })
    }

    pub fn finish(mut self) -> Result<()> {
        self.done.store(true, Ordering::Release);
        self.thread
            .take()
            .expect("file-change recorder thread exists")
            .join()
            .map_err(|_| anyhow::anyhow!("file-change recorder thread panicked"))?
    }
}

fn follow_and_checkpoint(
    workspace: &Path,
    isolated_workspace: &Path,
    raw_events: &Path,
    journal: &Path,
    reference: &str,
    initial_parent: &str,
    done: &AtomicBool,
) -> Result<()> {
    while !raw_events.exists() && !done.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !raw_events.exists() {
        return Ok(());
    }
    let input = File::open(raw_events)
        .with_context(|| format!("opening event journal {}", raw_events.display()))?;
    let output = OpenOptions::new()
        .append(true)
        .open(journal)
        .with_context(|| format!("opening file-change journal {}", journal.display()))?;
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    let index = journal.with_extension("index");
    let mut line = String::new();
    let mut pending = String::new();
    let mut parent = initial_parent.to_string();
    let mut sequence = 0;

    loop {
        line.clear();
        match input.read_line(&mut line)? {
            0 if done.load(Ordering::Acquire) => {
                if !pending.trim().is_empty() {
                    process_line(
                        workspace,
                        isolated_workspace,
                        reference,
                        &index,
                        &mut parent,
                        &mut sequence,
                        pending.trim_end(),
                        &mut output,
                    )?;
                }
                break;
            }
            0 => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                pending.push_str(&line);
                if pending.ends_with('\n') {
                    process_line(
                        workspace,
                        isolated_workspace,
                        reference,
                        &index,
                        &mut parent,
                        &mut sequence,
                        pending.trim_end(),
                        &mut output,
                    )?;
                    pending.clear();
                }
            }
        }
    }
    output.flush()?;
    let _ = std::fs::remove_file(index);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_line(
    workspace: &Path,
    isolated_workspace: &Path,
    reference: &str,
    index: &Path,
    parent: &mut String,
    sequence: &mut u64,
    line: &str,
    output: &mut dyn Write,
) -> Result<()> {
    let AgentEvent::FileChanged { id, paths, .. } = decode_codex_line(line) else {
        return Ok(());
    };
    *sequence += 1;
    let paths = paths
        .iter()
        .map(|path| project_path(path, workspace, isolated_workspace))
        .collect::<Result<Vec<_>>>();
    let record = match paths {
        Ok(paths) if !paths.is_empty() => {
            match checkpoint(workspace, index, reference, parent, *sequence, &id, &paths) {
                Ok(commit) => {
                    *parent = commit.clone();
                    FileChangeCheckpoint {
                        schema: FILE_CHANGE_SCHEMA,
                        sequence: *sequence,
                        event_id: id,
                        paths,
                        commit: Some(commit),
                        error: None,
                    }
                }
                Err(error) => FileChangeCheckpoint {
                    schema: FILE_CHANGE_SCHEMA,
                    sequence: *sequence,
                    event_id: id,
                    paths,
                    commit: None,
                    error: Some(format!("{error:#}")),
                },
            }
        }
        Ok(paths) => FileChangeCheckpoint {
            schema: FILE_CHANGE_SCHEMA,
            sequence: *sequence,
            event_id: id,
            paths,
            commit: None,
            error: Some("file-change event contained no paths".into()),
        },
        Err(error) => FileChangeCheckpoint {
            schema: FILE_CHANGE_SCHEMA,
            sequence: *sequence,
            event_id: id,
            paths: Vec::new(),
            commit: None,
            error: Some(format!("{error:#}")),
        },
    };
    serde_json::to_writer(&mut *output, &record)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn project_path(path: &str, workspace: &Path, isolated_workspace: &Path) -> Result<String> {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(isolated_workspace)
            .or_else(|_| path.strip_prefix(workspace))
            .with_context(|| {
                format!(
                    "file-change path is outside the workspace: {}",
                    path.display()
                )
            })?
    } else {
        path
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!(
            "invalid workspace-relative file-change path: {}",
            path.display()
        );
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn checkpoint(
    workspace: &Path,
    index: &Path,
    reference: &str,
    parent: &str,
    sequence: u64,
    event_id: &str,
    paths: &[String],
) -> Result<String> {
    let index_value = index.to_string_lossy().into_owned();
    git_with_index(workspace, &index_value, &["read-tree", parent])?;
    let mut add = vec!["add", "--all", "--force", "--"];
    add.extend(paths.iter().map(String::as_str));
    git_with_index(workspace, &index_value, &add)?;
    let tree = git_with_index(workspace, &index_value, &["write-tree"])?;
    let message = format!(
        "Orka file change {sequence}\n\nOrka-Event: {event_id}\nOrka-Paths: {}",
        paths.join(", ")
    );
    let commit = git_env(
        workspace,
        &[
            ("GIT_AUTHOR_NAME", "Orka"),
            ("GIT_AUTHOR_EMAIL", "orka@localhost"),
            ("GIT_COMMITTER_NAME", "Orka"),
            ("GIT_COMMITTER_EMAIL", "orka@localhost"),
        ],
        &["commit-tree", &tree, "-p", parent, "-m", &message],
    )?;
    checked(workspace, &["update-ref", reference, &commit])?;
    Ok(commit)
}

fn git_with_index(base: &Path, index: &str, args: &[&str]) -> Result<String> {
    git_env(base, &[("GIT_INDEX_FILE", index)], args)
}

fn checked(base: &Path, args: &[&str]) -> Result<String> {
    git_env(base, &[], args)
}

fn git_env(base: &Path, env: &[(&str, &str)], args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(base).args(args);
    for (name, value) in env {
        command.env(name, value);
    }
    let result = command
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !result.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub fn read_checkpoints(path: &Path) -> Result<Vec<FileChangeCheckpoint>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let input = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    BufReader::new(input)
        .lines()
        .filter(|line| line.as_ref().map_or(true, |line| !line.trim().is_empty()))
        .map(|line| {
            let line = line?;
            serde_json::from_str(&line).context("decoding file-change checkpoint")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repository() -> TempDir {
        let root = std::env::temp_dir().join(format!("orka-file-change-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&root).unwrap();
        checked(&root, &["init", "-q"]).unwrap();
        checked(&root, &["config", "user.name", "test"]).unwrap();
        checked(&root, &["config", "user.email", "test@example.com"]).unwrap();
        std::fs::write(root.join("source.rs"), "base\n").unwrap();
        checked(&root, &["add", "source.rs"]).unwrap();
        checked(&root, &["commit", "-qm", "base"]).unwrap();
        TempDir(root)
    }

    fn append_event(raw: &Path, id: &str) {
        let mut output = OpenOptions::new().append(true).open(raw).unwrap();
        writeln!(
            output,
            r#"{{"type":"item.completed","item":{{"id":"{id}","type":"file_change","changes":[{{"path":"/tmp/orka/workspace/source.rs"}}]}}}}"#
        )
        .unwrap();
    }

    fn wait_for_records(journal: &Path, count: usize) {
        let started = Instant::now();
        while read_checkpoints(journal).unwrap().len() < count {
            assert!(started.elapsed() < Duration::from_secs(2));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn every_file_change_gets_a_commit_without_moving_the_execution_head_or_index() {
        let repository = repository();
        let raw = repository.0.join("events.raw.jsonl");
        let journal = repository.0.join("file-changes.v1.jsonl");
        std::fs::write(&raw, b"").unwrap();
        let head = checked(&repository.0, &["rev-parse", "HEAD"]).unwrap();
        let recorder = FileChangeRecorder::start(
            &repository.0,
            Path::new("/tmp/orka/workspace"),
            &raw,
            &journal,
            "refs/orka/file-changes/test",
        )
        .unwrap();

        std::fs::write(repository.0.join("source.rs"), "first\n").unwrap();
        append_event(&raw, "change-1");
        wait_for_records(&journal, 1);
        std::fs::write(repository.0.join("source.rs"), "second\n").unwrap();
        append_event(&raw, "change-2");
        wait_for_records(&journal, 2);
        recorder.finish().unwrap();

        let records = read_checkpoints(&journal).unwrap();
        let first = records[0].commit.as_deref().unwrap();
        let second = records[1].commit.as_deref().unwrap();
        assert_eq!(
            checked(&repository.0, &["show", &format!("{first}:source.rs")]).unwrap(),
            "first"
        );
        assert_eq!(
            checked(&repository.0, &["show", &format!("{second}:source.rs")]).unwrap(),
            "second"
        );
        assert_eq!(
            checked(&repository.0, &["rev-parse", &format!("{second}^")]).unwrap(),
            first
        );
        assert_eq!(
            checked(&repository.0, &["rev-parse", "HEAD"]).unwrap(),
            head
        );
        assert_eq!(
            checked(&repository.0, &["rev-parse", "refs/orka/file-changes/test"]).unwrap(),
            second
        );
        assert!(checked(&repository.0, &["status", "--porcelain"])
            .unwrap()
            .lines()
            .any(|line| line == "M source.rs"));
        checked(&repository.0, &["diff", "--cached", "--quiet"]).unwrap();
    }
}

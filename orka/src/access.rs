//! Durable observation of project files read during one isolated execution.
//!
//! Codex JSONL explains which commands and tools ran, but a command can read
//! files without naming each one in its event. Orka therefore watches the
//! attempt's unique host-side worktree while Driva executes and journals the
//! conservative read set independently of the agent's declarations.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

#[cfg(target_os = "linux")]
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

pub const ACCESS_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccessEvent {
    TrackingStarted {
        schema: u32,
        method: String,
    },
    FileRead {
        path: String,
    },
    TrackingFinished {
        complete: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessSummary {
    pub method: String,
    pub reads: Vec<String>,
    pub complete: bool,
    pub reason: Option<String>,
}

/// A live recursive watcher. Finishing signals its reader thread, drains all
/// queued kernel events, and writes the terminal completeness record.
pub struct AccessRecorder {
    done: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl AccessRecorder {
    /// Start observing `workspace`, recording startup failure durably rather
    /// than silently returning an empty (apparently complete) read set.
    pub fn start(workspace: &Path, journal: &Path) -> Self {
        match Self::try_start(workspace, journal) {
            Ok(recorder) => recorder,
            Err(error) => {
                let _ = write_events(
                    journal,
                    &[
                        AccessEvent::TrackingStarted {
                            schema: ACCESS_SCHEMA,
                            method: "filesystem-watcher".into(),
                        },
                        AccessEvent::TrackingFinished {
                            complete: false,
                            reason: Some(format!("could not start access tracking: {error:#}")),
                        },
                    ],
                );
                Self {
                    done: Arc::new(AtomicBool::new(true)),
                    worker: None,
                }
            }
        }
    }

    fn try_start(workspace: &Path, journal: &Path) -> Result<Self> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (workspace, journal);
            anyhow::bail!("filesystem access tracking is currently supported only on Linux");
        }
        #[cfg(target_os = "linux")]
        Self::try_start_linux(workspace, journal)
    }

    #[cfg(target_os = "linux")]
    fn try_start_linux(workspace: &Path, journal: &Path) -> Result<Self> {
        let workspace = workspace
            .canonicalize()
            .with_context(|| format!("resolving access-tracking root {}", workspace.display()))?;
        let parent = journal
            .parent()
            .with_context(|| format!("access journal has no parent: {}", journal.display()))?;
        std::fs::create_dir_all(parent)?;
        let mut output = BufWriter::new(
            File::create(journal)
                .with_context(|| format!("creating access journal {}", journal.display()))?,
        );
        write_event(
            &mut output,
            &AccessEvent::TrackingStarted {
                schema: ACCESS_SCHEMA,
                method: "filesystem-watcher".into(),
            },
        )?;
        output.flush()?;

        let mut inotify = Inotify::init().context("creating inotify access tracker")?;
        let mut watches = std::collections::HashMap::new();
        add_watch_tree(&mut inotify, &workspace, &mut watches)?;
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = done.clone();

        let worker = std::thread::spawn(move || -> Result<()> {
            let mut seen = HashSet::new();
            let mut failure = None;
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match inotify.read_events(&mut buffer) {
                    Ok(events) => {
                        let mut had_events = false;
                        for event in events {
                            had_events = true;
                            if event.mask.contains(EventMask::Q_OVERFLOW) {
                                failure = Some("inotify event queue overflowed".into());
                                continue;
                            }
                            let Some(directory) = watches.get(&event.wd).cloned() else {
                                failure =
                                    Some("inotify returned an unknown watch descriptor".into());
                                continue;
                            };
                            let path = event
                                .name
                                .map(|name| directory.join(name))
                                .unwrap_or(directory);
                            if event.mask.contains(EventMask::ISDIR)
                                && (event.mask.contains(EventMask::CREATE)
                                    || event.mask.contains(EventMask::MOVED_TO))
                            {
                                if let Err(error) =
                                    add_watch_tree(&mut inotify, &path, &mut watches)
                                {
                                    failure = Some(format!(
                                        "could not watch new directory {}: {error:#}",
                                        path.display()
                                    ));
                                }
                            }
                            // ACCESS covers ordinary reads. CLOSE_NOWRITE is a
                            // conservative supplement for read-only opens
                            // whose content may have been memory-mapped.
                            if event.mask.contains(EventMask::ACCESS)
                                || event.mask.contains(EventMask::CLOSE_NOWRITE)
                            {
                                if let Some(path) = project_path(&workspace, &path) {
                                    if seen.insert(path.clone()) {
                                        write_event(&mut output, &AccessEvent::FileRead { path })?;
                                        output.flush()?;
                                    }
                                }
                            }
                        }
                        if worker_done.load(Ordering::Acquire) && !had_events {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if worker_done.load(Ordering::Acquire) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(error) => {
                        failure = Some(format!("inotify read failed: {error}"));
                        break;
                    }
                }
            }
            write_event(
                &mut output,
                &AccessEvent::TrackingFinished {
                    complete: failure.is_none(),
                    reason: failure,
                },
            )?;
            output.flush()?;
            Ok(())
        });

        Ok(Self {
            done,
            worker: Some(worker),
        })
    }

    pub fn finish(mut self) -> Result<()> {
        self.done.store(true, Ordering::Release);
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| anyhow::anyhow!("access-tracking worker panicked"))?,
            None => Ok(()),
        }
    }
}

#[cfg(target_os = "linux")]
fn add_watch_tree(
    inotify: &mut Inotify,
    root: &Path,
    watches: &mut std::collections::HashMap<WatchDescriptor, std::path::PathBuf>,
) -> Result<()> {
    if root.file_name().is_some_and(|name| name == ".git") {
        return Ok(());
    }
    let descriptor = inotify
        .add_watch(
            root,
            WatchMask::ACCESS | WatchMask::CLOSE_NOWRITE | WatchMask::CREATE | WatchMask::MOVED_TO,
        )
        .with_context(|| format!("watching {}", root.display()))?;
    watches.insert(descriptor, root.to_path_buf());
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("enumerating access-tracking root {}", root.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            add_watch_tree(inotify, &entry.path(), watches)?;
        }
    }
    Ok(())
}

fn project_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || relative
            .components()
            .any(|component| matches!(component, Component::Normal(name) if name == ".git"))
        || !path.is_file()
    {
        return None;
    }
    let path = relative.to_string_lossy().into_owned();
    path.parse::<linka::ProjectPath>()
        .ok()
        .map(|p| p.to_string())
}

pub fn read_access_summary(path: &Path) -> Result<Option<AccessSummary>> {
    let input = match File::open(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let mut summary = AccessSummary::default();
    let mut seen = HashSet::new();
    let mut started = false;
    let mut finished = false;
    for line in BufReader::new(input).lines() {
        let line = line.with_context(|| format!("reading {}", path.display()))?;
        let event: AccessEvent = serde_json::from_str(&line)
            .with_context(|| format!("parsing access journal {}", path.display()))?;
        match event {
            AccessEvent::TrackingStarted { schema, method } => {
                if schema != ACCESS_SCHEMA {
                    anyhow::bail!(
                        "unsupported access journal schema {schema} in {}",
                        path.display()
                    );
                }
                summary.method = method;
                started = true;
            }
            AccessEvent::FileRead { path } if seen.insert(path.clone()) => summary.reads.push(path),
            AccessEvent::FileRead { .. } => {}
            AccessEvent::TrackingFinished { complete, reason } => {
                summary.complete = complete;
                summary.reason = reason;
                finished = true;
            }
        }
    }
    if !started || !finished {
        summary.complete = false;
        summary.reason = Some("access journal is missing lifecycle records".into());
    }
    Ok(Some(summary))
}

/// Used by test executors and by startup-failure handling to produce the same
/// durable format as the live watcher.
pub fn write_access_summary(
    path: &Path,
    method: &str,
    reads: &[String],
    complete: bool,
    reason: Option<String>,
) -> Result<()> {
    let mut events = vec![AccessEvent::TrackingStarted {
        schema: ACCESS_SCHEMA,
        method: method.into(),
    }];
    events.extend(
        reads
            .iter()
            .cloned()
            .map(|path| AccessEvent::FileRead { path }),
    );
    events.push(AccessEvent::TrackingFinished { complete, reason });
    write_events(path, &events)
}

fn write_events(path: &Path, events: &[AccessEvent]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("access journal has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;
    let mut output = BufWriter::new(File::create(path)?);
    for event in events {
        write_event(&mut output, event)?;
    }
    output.flush()?;
    Ok(())
}

fn write_event(output: &mut dyn Write, event: &AccessEvent) -> Result<()> {
    serde_json::to_writer(&mut *output, event)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_round_trip_and_deduplicate_reads() {
        let dir = std::env::temp_dir().join(format!("orka-access-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = dir.join("accesses.v1.jsonl");
        write_access_summary(
            &journal,
            "test",
            &["src/lib.rs".into(), "src/lib.rs".into(), "README.md".into()],
            true,
            None,
        )
        .unwrap();
        let summary = read_access_summary(&journal).unwrap().unwrap();
        assert!(summary.complete);
        assert_eq!(summary.reads, ["src/lib.rs", "README.md"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_watcher_records_reads_but_not_git_or_outside_files() {
        let dir = std::env::temp_dir().join(format!("orka-access-test-{}", ulid::Ulid::new()));
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::write(workspace.join("src/lib.rs"), "content").unwrap();
        std::fs::write(workspace.join(".git/config"), "git").unwrap();
        let journal = dir.join("accesses.v1.jsonl");

        let recorder = AccessRecorder::start(&workspace, &journal);
        let _ = std::fs::read_to_string(workspace.join("src/lib.rs")).unwrap();
        let _ = std::fs::read_to_string(workspace.join(".git/config")).unwrap();
        recorder.finish().unwrap();

        let summary = read_access_summary(&journal).unwrap().unwrap();
        assert!(summary.complete, "{:?}", summary.reason);
        assert_eq!(summary.reads, ["src/lib.rs"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

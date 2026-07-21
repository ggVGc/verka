//! In-memory test doubles for the replaceable boundaries.
//!
//! Public (not `cfg(test)`) so integration tests and downstream harnesses can
//! drive the orchestration engine without a container engine or a git
//! repository. The Linka store is not faked — Orka orchestrates Linka
//! specifically, so tests that touch selection or submission use a real store.

use crate::executor::{ExecutionReport, ExecutionSpec, IsolatedExecutor};
use crate::workspace::{CleanupOutcome, DiscardOutcome, PreparedWorkspace, WorkspaceManager};
use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// An [`IsolatedExecutor`] that writes a canned transcript and returns a
/// canned report. `on_run` can mutate the filesystem the way a real agent
/// command would (e.g. write an outcome file into a mounted directory).
#[allow(clippy::type_complexity)]
pub struct FakeExecutor {
    pub exit_code: i32,
    pub transcript: String,
    pub runs: RefCell<Vec<ExecutionSpec>>,
    pub on_run: Option<Box<dyn Fn(&ExecutionSpec) -> Result<()>>>,
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self {
            exit_code: 0,
            transcript: String::new(),
            runs: RefCell::new(Vec::new()),
            on_run: None,
        }
    }
}

impl IsolatedExecutor for FakeExecutor {
    fn run(&self, spec: &ExecutionSpec, transcript: &Path) -> Result<ExecutionReport> {
        std::fs::write(transcript, &self.transcript)?;
        if let Some(hook) = &self.on_run {
            hook(spec)?;
        }
        self.runs.borrow_mut().push(spec.clone());
        Ok(ExecutionReport {
            backend: "fake".into(),
            exit_code: self.exit_code,
            started_at_ms: 0,
            finished_at_ms: 0,
        })
    }
}

/// A [`WorkspaceManager`] over plain temp directories: no git, no branches.
pub struct FakeWorkspaces {
    pub root: PathBuf,
    /// Workspaces cleanup should report dirty (by attempt name).
    pub dirty: Vec<String>,
    pub cleanups: RefCell<Vec<PreparedWorkspace>>,
}

impl FakeWorkspaces {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            dirty: Vec::new(),
            cleanups: RefCell::new(Vec::new()),
        }
    }
}

impl WorkspaceManager for FakeWorkspaces {
    fn plan(&self, attempt: &str, input_commit: &str) -> PreparedWorkspace {
        PreparedWorkspace {
            path: self.root.join(attempt),
            branch: format!("orka/attempts/{attempt}"),
            input_commit: input_commit.to_string(),
        }
    }

    fn prepare(&self, attempt: &str, input_commit: &str) -> Result<PreparedWorkspace> {
        let planned = self.plan(attempt, input_commit);
        if planned.path.exists() {
            return Err(anyhow!(
                "workspace already exists: {}",
                planned.path.display()
            ));
        }
        std::fs::create_dir_all(&planned.path)?;
        Ok(planned)
    }

    fn cleanup(&self, workspace: &PreparedWorkspace) -> Result<CleanupOutcome> {
        self.cleanups.borrow_mut().push(workspace.clone());
        let attempt = workspace
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.dirty.contains(&attempt) {
            return Ok(CleanupOutcome::RetainedDirty);
        }
        if !workspace.path.exists() {
            return Ok(CleanupOutcome::AlreadyAbsent);
        }
        std::fs::remove_dir_all(&workspace.path)?;
        Ok(CleanupOutcome::Removed)
    }

    fn discard_unchanged(&self, workspace: &PreparedWorkspace) -> Result<DiscardOutcome> {
        self.cleanups.borrow_mut().push(workspace.clone());
        let attempt = workspace
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.dirty.contains(&attempt) {
            return Ok(DiscardOutcome::RetainedChanged);
        }
        if workspace.path.exists() {
            std::fs::remove_dir_all(&workspace.path)?;
        }
        Ok(DiscardOutcome::Discarded)
    }
}

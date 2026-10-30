//! The processes running in a live interaction's sandbox, as a process tree.

use crate::chrome::PanelChrome;
use std::collections::HashSet;
use styra_protocol::SandboxProcess;

pub enum ProcessListing<'a> {
    /// Asked, not yet answered.
    Loading,
    /// No sandbox runs for the Session: it is stopped, or still starting.
    NotRunning,
    Failed(&'a str),
    Running(&'a [SandboxProcess]),
}

pub struct ProcessesView<'a> {
    pub chrome: PanelChrome,
    pub listing: ProcessListing<'a>,
    /// Row in [`tree`] order. `None` for the pane below the event list,
    /// which is read rather than navigated: the event list keeps the keys.
    pub selected: Option<usize>,
}

/// One process placed in the tree, `depth` levels below a root.
pub struct TreeRow<'a> {
    pub process: &'a SandboxProcess,
    pub depth: usize,
}

/// Processes ordered parent before children, siblings by their sandbox pid.
/// A root is a process whose parent is not listed: the sandbox's init, whose
/// parent is the Bubblewrap supervisor outside it.
pub fn tree(processes: &[SandboxProcess]) -> Vec<TreeRow<'_>> {
    let listed: HashSet<u32> = processes.iter().map(|process| process.pid).collect();
    let mut sorted: Vec<&SandboxProcess> = processes.iter().collect();
    sorted.sort_by_key(|process| (process.sandbox_pid, process.pid));
    let mut rows = Vec::with_capacity(processes.len());
    let mut pending: Vec<(&SandboxProcess, usize)> = sorted
        .iter()
        .rev()
        .filter(|process| !listed.contains(&process.parent_pid))
        .map(|process| (*process, 0))
        .collect();
    let mut placed = HashSet::new();
    while let Some((process, depth)) = pending.pop() {
        if !placed.insert(process.pid) {
            continue;
        }
        rows.push(TreeRow { process, depth });
        pending.extend(
            sorted
                .iter()
                .rev()
                .filter(|child| child.parent_pid == process.pid && child.pid != process.pid)
                .map(|child| (*child, depth + 1)),
        );
    }
    // A parent cycle has no root to reach it from; list it rather than lose it.
    for process in sorted {
        if !placed.contains(&process.pid) {
            rows.push(TreeRow { process, depth: 0 });
        }
    }
    rows
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/processes.rs"]
mod rendering;

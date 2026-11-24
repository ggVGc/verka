//! What runs inside a live interaction's sandbox, read from the host's `/proc`.
//!
//! Driva hands out the host pid of the Bubblewrap supervisor (see
//! [`driva::ExecutionControl::process_id`]). Bubblewrap unshares the PID
//! namespace, so the supervisor's child is the sandbox's init, PID 1 inside,
//! and everything the agent starts lives in that namespace with it. A process
//! that daemonizes is reparented to that init rather than escaping, so
//! matching the namespace finds it where walking the agent's descendants
//! would not.
//!
//! A sandbox that shares the host's PID namespace has no such boundary to
//! match; its processes are then the supervisor's descendants.

use crate::protocol::SandboxProcess;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The processes in the sandbox `supervisor` runs, ordered by their pid
/// inside it. Empty while the sandbox has not started its init.
pub fn sandbox_processes(supervisor: u32) -> Vec<SandboxProcess> {
    processes_in(Path::new("/proc"), supervisor)
}

fn processes_in(proc: &Path, supervisor: u32) -> Vec<SandboxProcess> {
    let Some(host_namespace) = pid_namespace(proc, supervisor) else {
        return Vec::new();
    };
    let children = children(proc, supervisor);
    let sandbox_namespace = children
        .iter()
        .filter_map(|&child| pid_namespace(proc, child))
        .find(|namespace| *namespace != host_namespace);
    let members: BTreeSet<u32> = match sandbox_namespace {
        Some(namespace) => all_pids(proc)
            .filter(|&pid| pid_namespace(proc, pid).as_ref() == Some(&namespace))
            .collect(),
        None => descendants(proc, children),
    };
    let mut processes: Vec<_> = members
        .into_iter()
        .filter_map(|pid| describe(proc, pid))
        .collect();
    processes.sort_by_key(|process| (process.sandbox_pid, process.pid));
    processes
}

fn pid_namespace(proc: &Path, pid: u32) -> Option<PathBuf> {
    std::fs::read_link(proc.join(pid.to_string()).join("ns/pid")).ok()
}

fn all_pids(proc: &Path) -> impl Iterator<Item = u32> {
    std::fs::read_dir(proc)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
}

/// Children of every thread of `pid`: a child belongs to the thread that
/// forked it, not only to the main one.
fn children(proc: &Path, pid: u32) -> Vec<u32> {
    let tasks = proc.join(pid.to_string()).join("task");
    std::fs::read_dir(tasks)
        .into_iter()
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn descendants(proc: &Path, mut pending: Vec<u32>) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if found.insert(pid) {
            pending.extend(children(proc, pid));
        }
    }
    found
}

/// Read one process, or nothing if it has already gone.
fn describe(proc: &Path, pid: u32) -> Option<SandboxProcess> {
    let directory = proc.join(pid.to_string());
    let status = std::fs::read_to_string(directory.join("status")).ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':').map(str::trim))
    };
    let number = |name: &str| field(name).and_then(|value| value.parse().ok());
    let command = std::fs::read(directory.join("cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|&byte| byte == 0)
                .filter(|argument| !argument.is_empty())
                .map(String::from_utf8_lossy)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| format!("[{}]", field("Name").unwrap_or("?")));
    Some(SandboxProcess {
        pid,
        // `NSpid` lists the id in each nested namespace, outermost first.
        sandbox_pid: field("NSpid")
            .and_then(|ids| ids.split_whitespace().last()?.parse().ok())
            .unwrap_or(pid),
        parent_pid: number("PPid").unwrap_or(0),
        state: field("State")
            .and_then(|state| state.chars().next())
            .map(String::from)
            .unwrap_or_default(),
        command,
        threads: number("Threads").unwrap_or(1),
        rss_kib: field("VmRSS")
            .and_then(|rss| rss.split_whitespace().next()?.parse().ok())
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests;

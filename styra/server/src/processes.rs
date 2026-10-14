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
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct FakeProc(tempfile::TempDir);

    impl FakeProc {
        fn new() -> Self {
            Self(tempfile::tempdir().unwrap())
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn process(&self, pid: u32, namespace: u64, nspid: &str, ppid: u32, cmdline: &[u8]) {
            let directory = self.path().join(pid.to_string());
            std::fs::create_dir_all(directory.join("ns")).unwrap();
            std::fs::create_dir_all(directory.join("task").join(pid.to_string())).unwrap();
            symlink(format!("pid:[{namespace}]"), directory.join("ns/pid")).unwrap();
            std::fs::write(
                directory.join("status"),
                format!(
                    "Name:\tproc{pid}\nState:\tS (sleeping)\nPPid:\t{ppid}\n\
                     VmRSS:\t   2048 kB\nThreads:\t3\nNSpid:\t{nspid}\n"
                ),
            )
            .unwrap();
            std::fs::write(directory.join("cmdline"), cmdline).unwrap();
            self.children(pid, &[]);
        }

        fn children(&self, pid: u32, children: &[u32]) {
            let list: Vec<_> = children.iter().map(u32::to_string).collect();
            std::fs::write(
                self.path()
                    .join(pid.to_string())
                    .join("task")
                    .join(pid.to_string())
                    .join("children"),
                list.join(" "),
            )
            .unwrap();
        }
    }

    #[test]
    fn processes_are_the_members_of_the_sandboxs_pid_namespace() {
        let proc = FakeProc::new();
        proc.process(1, 1, "1", 0, b"/sbin/init\0");
        proc.process(100, 1, "100", 1, b"bwrap\0--unshare-all\0");
        proc.process(101, 2, "101 1", 100, b"bwrap\0");
        proc.process(102, 2, "102 2", 101, b"claude\0--print\0");
        // Daemonized: reparented to the sandbox's init, not the agent's.
        proc.process(103, 2, "103 3", 101, b"");
        proc.process(200, 1, "200", 1, b"unrelated\0");
        proc.children(100, &[101]);
        proc.children(101, &[102, 103]);

        let processes = processes_in(proc.path(), 100);

        let ids: Vec<_> = processes
            .iter()
            .map(|process| (process.pid, process.sandbox_pid, process.parent_pid))
            .collect();
        assert_eq!(ids, [(101, 1, 100), (102, 2, 101), (103, 3, 101)]);
        assert_eq!(processes[1].command, "claude --print");
        assert_eq!(processes[2].command, "[proc103]");
        assert_eq!(processes[1].state, "S");
        assert_eq!(processes[1].threads, 3);
        assert_eq!(processes[1].rss_kib, 2048);
    }

    #[test]
    fn a_sandbox_sharing_the_host_namespace_is_its_supervisors_descendants() {
        let proc = FakeProc::new();
        proc.process(100, 1, "100", 1, b"bwrap\0");
        proc.process(101, 1, "101", 100, b"agent\0");
        proc.process(102, 1, "102", 101, b"tool\0");
        proc.process(200, 1, "200", 1, b"unrelated\0");
        proc.children(100, &[101]);
        proc.children(101, &[102]);

        let pids: Vec<_> = processes_in(proc.path(), 100)
            .iter()
            .map(|process| process.pid)
            .collect();
        assert_eq!(pids, [101, 102]);
    }

    #[test]
    fn a_supervisor_that_is_gone_or_not_yet_forked_runs_nothing() {
        let proc = FakeProc::new();
        assert!(processes_in(proc.path(), 100).is_empty());
        proc.process(100, 1, "100", 1, b"bwrap\0");
        assert!(processes_in(proc.path(), 100).is_empty());
    }

    #[test]
    fn a_real_bubblewrap_sandbox_lists_its_daemonized_processes() {
        let spawn = std::process::Command::new("bwrap")
            .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/"])
            .args(["--proc", "/proc", "--dev", "/dev", "sh", "-c"])
            .arg("(sleep 31 &); exec sleep 32")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut supervisor) = spawn else {
            return eprintln!("skipping: bwrap is not available");
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut processes = Vec::new();
        while std::time::Instant::now() < deadline {
            processes = sandbox_processes(supervisor.id());
            let commands: Vec<_> = processes.iter().map(|p| p.command.as_str()).collect();
            if commands.contains(&"sleep 31") && commands.contains(&"sleep 32") {
                break;
            }
            if supervisor.try_wait().unwrap().is_some() {
                return eprintln!("skipping: bwrap cannot create a sandbox here");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        supervisor.kill().unwrap();
        supervisor.wait().unwrap();

        let init = processes
            .iter()
            .find(|process| process.sandbox_pid == 1)
            .expect("the sandbox's init is listed");
        assert_eq!(init.parent_pid, supervisor.id());
        let daemon = processes
            .iter()
            .find(|process| process.command == "sleep 31")
            .expect("a daemonized process is still in the sandbox");
        assert_eq!(daemon.parent_pid, init.pid);
        assert!(processes
            .iter()
            .any(|process| process.command == "sleep 32"));
        assert!(processes
            .iter()
            .all(|process| process.pid != supervisor.id()));
    }

    #[test]
    fn the_real_proc_lists_a_child_of_this_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let processes = sandbox_processes(std::process::id());
        child.kill().unwrap();
        child.wait().unwrap();
        let sleep = processes
            .iter()
            .find(|process| process.pid == child.id())
            .expect("the child is a descendant in the same namespace");
        assert_eq!(sleep.command, "sleep 30");
        assert_eq!(sleep.parent_pid, std::process::id());
    }
}

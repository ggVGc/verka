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
    // spawn() can return before /proc publishes the new argv. Wait for
    // that fixture state rather than asserting against the transient
    // kernel-name fallback ("[sleep]").
    let cmdline = PathBuf::from(format!("/proc/{}/cmdline", child.id()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::fs::read(&cmdline).unwrap_or_default().is_empty()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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

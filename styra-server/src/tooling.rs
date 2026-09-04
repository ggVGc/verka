//! The host executables a launch has to run, as mounts an operator can see.
//!
//! A Styra sandbox is Driva's private root: a tmpfs carrying the host system
//! runtime read-only ([`driva::host_runtime`]) and nothing else. An agent
//! installed outside that runtime — `~/.local/bin/claude`, a toolchain under
//! `~/.local/share` — is simply not there unless the launch puts it there, and
//! the rule is that everything the sandbox holds is a mount, attributed to the
//! layer that asked for it. So the launch names the executables it needs and
//! they arrive as ordinary read-only binds under [`MountOrigin::Tooling`],
//! rather than through a host filesystem the policy never mentions.
//!
//! [`MountOrigin::Tooling`]: crate::protocol::MountOrigin::Tooling

use crate::agent::MountSpec;
use anyhow::{Context, Result};
use driva::RuntimeEntry;
use std::path::{Path, PathBuf};

/// Read-only mounts exposing `executables` at their own host paths, minus
/// whatever the private root already carries.
///
/// A launcher symlink is mounted *and* followed: the path the command names
/// has to exist inside the sandbox, and the file it resolves to has to exist
/// at its own path too, so a tool that locates its runtime relative to where
/// it really lives still finds it.
pub fn executable_mounts(executables: &[PathBuf]) -> Result<Vec<MountSpec>> {
    let runtime = driva::host_runtime().context("resolving Driva's host system runtime")?;
    let mut mounts: Vec<MountSpec> = Vec::new();
    for executable in executables {
        let resolved = executable.canonicalize().with_context(|| {
            format!(
                "resolving the host executable {} for the sandbox",
                executable.display()
            )
        })?;
        for path in [executable.clone(), resolved] {
            if carried_by_runtime(&runtime, &path) {
                continue;
            }
            if mounts.iter().any(|mount| mount.destination == path) {
                continue;
            }
            mounts.push(MountSpec {
                source: path.clone(),
                destination: path,
                writable: false,
            });
        }
    }
    Ok(mounts)
}

/// Whether the private root already exposes `path`, so mounting it again would
/// add a row that grants nothing.
///
/// Only read-only binds count: a runtime *symlink* entry (`/bin` pointing at
/// `usr/bin`, say) leaves the path itself resolving elsewhere, and that
/// elsewhere is what the executable's own resolved path is tested against.
fn carried_by_runtime(runtime: &[RuntimeEntry], path: &Path) -> bool {
    runtime.iter().any(|entry| match entry {
        RuntimeEntry::ReadOnly { path: carried, .. } => path.starts_with(carried),
        RuntimeEntry::Symlink { .. } => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The private root already holds the system runtime, so a tool installed
    /// there needs no mount of its own — and one installed outside it does.
    #[test]
    fn only_executables_outside_the_private_root_become_mounts() {
        let runtime = vec![
            RuntimeEntry::ReadOnly {
                source: PathBuf::from("/usr"),
                path: PathBuf::from("/usr"),
            },
            RuntimeEntry::Symlink {
                target: PathBuf::from("usr/bin"),
                path: PathBuf::from("/bin"),
            },
        ];
        assert!(carried_by_runtime(&runtime, Path::new("/usr/bin/tmux")));
        assert!(!carried_by_runtime(
            &runtime,
            Path::new("/home/operator/.local/bin/claude")
        ));
        // The symlinked path is not itself bound; the resolved target is what
        // the private root carries.
        assert!(!carried_by_runtime(&runtime, Path::new("/bin/tmux")));
    }

    /// A launcher outside the runtime is mounted at the path the command names
    /// and at the path it resolves to, once each.
    #[test]
    fn a_launcher_and_its_target_are_both_mounted_once() {
        let directory = std::env::temp_dir().join(format!("styra-tooling-{}", std::process::id()));
        let installed = directory.join("share/agent");
        let launcher = directory.join("bin/agent");
        std::fs::create_dir_all(installed.parent().expect("share directory")).expect("create");
        std::fs::create_dir_all(launcher.parent().expect("bin directory")).expect("create");
        std::fs::write(&installed, b"#!/bin/sh\n").expect("write agent");
        std::os::unix::fs::symlink(&installed, &launcher).expect("link agent");

        let mounts = executable_mounts(&[launcher.clone(), launcher.clone()]).expect("mounts");

        let destinations: Vec<&Path> = mounts
            .iter()
            .map(|mount| mount.destination.as_path())
            .collect();
        assert_eq!(
            destinations,
            vec![
                launcher.as_path(),
                installed.canonicalize().expect("canonical").as_path()
            ]
        );
        assert!(mounts.iter().all(|mount| !mount.writable));
        std::fs::remove_dir_all(&directory).ok();
    }

    /// An executable that is not there is a launch that cannot work; say so
    /// while the session is being planned rather than after it fails to spawn.
    #[test]
    fn a_missing_executable_is_an_error() {
        let missing = PathBuf::from("/nonexistent/styra/agent");
        assert!(executable_mounts(&[missing]).is_err());
    }
}

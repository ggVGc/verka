//! The host executables a launch has to run, as mounts an operator can see.
//!
//! A Styra sandbox is Driva's private root: a tmpfs carrying the base system
//! read-only (see [`driva::base`]) and nothing else. An agent installed
//! outside that base — `~/.local/bin/claude`, a toolchain under
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
pub fn executable_mounts(
    executables: &[PathBuf],
    base: &driva::BaseConfig,
) -> Result<Vec<MountSpec>> {
    let base = driva::resolve_base(base).context("resolving the sandbox base for this host")?;
    let runtime: Vec<RuntimeEntry> = base.entries().cloned().collect();
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
mod tests;

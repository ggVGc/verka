//! Locating an agent binary on the host's `PATH`.
//!
//! Provider-agnostic: which binary to look for is the provider's business
//! ([`Provider::executable`](super::Provider::executable)), finding it is not.

use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Locate the agent binary `name` on the host's `PATH`, as
/// [`Profile::builtin`](super::Profile::builtin) does.
///
/// A profile's command must name a binary that resolves *inside* the sandbox,
/// where the isolation backend clears the environment and supplies a fixed
/// system `PATH`. An operator's own `PATH` entries — `~/.local/bin`, a version
/// manager's shims — are not part of it, so a bare `claude` or `codex` there
/// fails deep inside the sandbox with an opaque `execvp: No such file or
/// directory`. Resolving on the host instead pins the exact binary the operator
/// would have run (the sandbox binds the host root, so the path is valid on both
/// sides) and turns a missing agent into a clear error before any isolation is
/// built.
pub fn resolve_executable(name: &Path) -> Result<PathBuf> {
    let search = std::env::var_os("PATH").unwrap_or_default();
    resolve_executable_on_path(name, &search)
}

/// [`resolve_executable`] against an explicit `PATH`-shaped search path.
///
/// A `name` containing a separator is a path already and is only checked, not
/// searched for — matching `execvp`. Symlinks are followed to check the target,
/// but the returned path is the one given: a launcher symlink is what the
/// operator installed, and it resolves the same way inside the sandbox.
pub fn resolve_executable_on_path(name: &Path, search: &OsStr) -> Result<PathBuf> {
    if name
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        if is_executable_file(name) {
            return Ok(name.to_path_buf());
        }
        bail!(
            "agent executable {} is not an executable file",
            name.display()
        );
    }
    std::env::split_paths(search)
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable_file(candidate))
        .with_context(|| {
            format!(
                "agent executable {} was not found on PATH ({}); install it or configure an absolute path",
                name.display(),
                search.to_string_lossy(),
            )
        })
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;

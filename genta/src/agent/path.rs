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
mod tests {
    use super::*;

    #[test]
    fn resolution_takes_the_first_executable_file_on_the_search_path() {
        let root = std::env::temp_dir().join(format!("genta-resolve-{}", std::process::id()));
        let (empty, decoy, real) = (root.join("empty"), root.join("decoy"), root.join("real"));
        for directory in [&empty, &decoy, &real] {
            std::fs::create_dir_all(directory).unwrap();
        }
        // A same-named file that is not executable must not shadow the install.
        std::fs::write(decoy.join("claude"), "notes about claude").unwrap();
        let installed = real.join("claude");
        std::fs::write(&installed, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();

        let search = std::env::join_paths([&empty, &decoy, &real]).unwrap();
        assert_eq!(
            resolve_executable_on_path(Path::new("claude"), &search).unwrap(),
            installed
        );
    }

    /// The common Claude Code install is a launcher symlink into a versioned
    /// directory. Both are visible inside the sandbox, so the link is kept: it is
    /// what the operator installed and what an update repoints.
    #[test]
    fn an_explicit_executable_is_used_as_given_including_a_launcher_symlink() {
        let root = std::env::temp_dir().join(format!("genta-symlink-{}", std::process::id()));
        let (bin, versions) = (root.join("bin"), root.join("versions"));
        for directory in [&bin, &versions] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let versioned = versions.join("2.1.219");
        std::fs::write(&versioned, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&versioned, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = bin.join("claude");
        let _ = std::fs::remove_file(&launcher);
        std::os::unix::fs::symlink(&versioned, &launcher).unwrap();

        assert_eq!(
            resolve_executable_on_path(&launcher, OsStr::new("")).unwrap(),
            launcher,
            "the symlink is followed to check the target, but not resolved away"
        );
    }

    #[test]
    fn an_explicit_executable_that_is_not_a_program_is_rejected() {
        let path = std::env::temp_dir().join(format!("genta-not-a-program-{}", std::process::id()));
        std::fs::write(&path, "not executable").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = resolve_executable_on_path(&path, OsStr::new(""))
            .expect_err("a non-executable file is not an agent");
        assert!(format!("{error:#}").contains("not an executable file"));
    }
}

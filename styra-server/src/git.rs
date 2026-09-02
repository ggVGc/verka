//! Git repository discovery and the mounts needed to make an associated
//! repository usable inside a Styra sandbox.

use crate::agent::MountSpec;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve `path` to the root of its nearest enclosing Git checkout.
pub fn repository_root(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Git repository path {} must exist", path.display()))?;
    git_path(&path, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not inside a Git repository", path.display()))
}

/// Mandatory mounts for a Workspace's associated repository.
pub fn mounts(root: &Path) -> Result<Vec<MountSpec>> {
    let root = repository_root(root)?;
    let git_dir = git_path(&root, &["rev-parse", "--absolute-git-dir"])
        .context("resolving Git directory")?;
    let common_dir = git_path(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .context("resolving Git common directory")?;

    let mut mounts = Vec::new();
    push(&mut mounts, root, false);
    if let Some(parent) = common_dir.parent() {
        push(&mut mounts, parent.to_path_buf(), false);
    }
    push(&mut mounts, common_dir.clone(), true);
    if !git_dir.starts_with(&common_dir) {
        if let Some(parent) = git_dir.parent() {
            push(&mut mounts, parent.to_path_buf(), false);
        }
        push(&mut mounts, git_dir, true);
    }
    Ok(mounts)
}

fn push(mounts: &mut Vec<MountSpec>, path: PathBuf, writable: bool) {
    if let Some(existing) = mounts.iter_mut().find(|mount| mount.destination == path) {
        existing.writable |= writable;
        return;
    }
    mounts.push(MountSpec {
        source: path.clone(),
        destination: path,
        writable,
    });
}

fn git_path(directory: &Path, arguments: &[&str]) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .context("running git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git failed: {}", stderr.trim());
    }
    let value = String::from_utf8(output.stdout).context("Git returned a non-UTF-8 path")?;
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("git returned an empty path");
    }
    Path::new(value)
        .canonicalize()
        .with_context(|| format!("resolving Git path {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("styra-git-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn git(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    fn init(repository: &Path) {
        git(repository, &["init", "--quiet"]);
        git(repository, &["config", "user.name", "Styra Test"]);
        git(repository, &["config", "user.email", "styra@example.invalid"]);
        std::fs::write(repository.join("tracked"), "initial\n").unwrap();
        git(repository, &["add", "tracked"]);
        git(repository, &["commit", "--quiet", "-m", "initial"]);
    }

    #[test]
    fn a_regular_checkout_mounts_the_tree_read_only_and_git_writable() {
        let root = temp("regular");
        init(&root);

        let resolved = mounts(&root).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].source, root.canonicalize().unwrap());
        assert!(!resolved[0].writable);
        assert_eq!(
            resolved[1].source,
            root.join(".git").canonicalize().unwrap()
        );
        assert!(resolved[1].writable);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_linked_worktree_mounts_its_common_git_directory() {
        let base = temp("worktree");
        let main = base.join("main");
        let worktree = base.join("feature");
        std::fs::create_dir(&main).unwrap();
        init(&main);
        git(
            &main,
            &["worktree", "add", "--quiet", "-b", "feature", worktree.to_str().unwrap()],
        );

        let resolved = mounts(&worktree).unwrap();
        let common = main.join(".git").canonicalize().unwrap();
        assert!(resolved
            .iter()
            .any(|mount| mount.source == common && mount.writable));
        assert!(resolved
            .iter()
            .any(|mount| { mount.source == main.canonicalize().unwrap() && !mount.writable }));
        assert!(resolved
            .iter()
            .any(|mount| { mount.source == worktree.canonicalize().unwrap() && !mount.writable }));
        std::fs::remove_dir_all(base).ok();
    }
}

//! Git repository discovery and the mounts needed to make an associated
//! repository usable inside a Styra sandbox.

use crate::agent::MountSpec;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolve `path` to the root of its nearest enclosing Git checkout.
pub fn repository_root(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("Git repository path {} must exist", path.display()))?;
    path.ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
        .with_context(|| format!("{} is not inside a Git repository", path.display()))
}

/// Mandatory mounts for a Workspace's associated repository.
pub fn mounts(root: &Path) -> Result<Vec<MountSpec>> {
    let root = repository_root(root)?;
    let git_file = root.join(".git");
    let git_dir = if git_file.is_dir() {
        git_file.canonicalize()?
    } else {
        let pointer = std::fs::read_to_string(&git_file)
            .with_context(|| format!("reading Git pointer {}", git_file.display()))?;
        let target = pointer
            .lines()
            .find_map(|line| line.trim().strip_prefix("gitdir:"))
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .with_context(|| format!("invalid Git pointer {}", git_file.display()))?;
        resolve(&root, Path::new(target))
            .canonicalize()
            .with_context(|| format!("resolving Git directory from {}", git_file.display()))?
    };
    let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .map(|target| resolve(&git_dir, Path::new(target.trim())))
        .unwrap_or_else(|| git_dir.clone())
        .canonicalize()
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

fn resolve(base: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        base.join(target)
    }
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

    #[test]
    fn a_regular_checkout_mounts_the_tree_read_only_and_git_writable() {
        let root = temp("regular");
        std::fs::create_dir(root.join(".git")).unwrap();

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
        let worktree_git = main.join(".git/worktrees/feature");
        std::fs::create_dir_all(&worktree_git).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        std::fs::write(worktree_git.join("commondir"), "../..\n").unwrap();

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

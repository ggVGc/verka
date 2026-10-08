//! An interaction's checkout diffed against the commit its branch was made
//! at: what `d` shows inside Styra, and what `D` hands to the configured
//! tool instead (see [`crate::config::Configuration::open_diff`]).

use std::path::{Path, PathBuf};
use std::process::Command;
use styra_protocol::InteractionSummary;

use crate::app::Scroll;

/// What a diff of an interaction's checkout compares, as the server last
/// reported the interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffTarget {
    /// Root of the working tree the agent works in.
    pub worktree: PathBuf,
    /// The branch checked out there; `None` when its head is detached.
    pub branch: Option<String>,
    /// The branch the checkout's branch was made from, for saying so; `None`
    /// when the repository's head was detached at the time.
    pub base_branch: Option<String>,
    /// The commit the checkout's branch was made at, which is what is diffed
    /// against: the branch it was made from has moved on since.
    pub base: String,
}

impl DiffTarget {
    /// The diff `interaction` has to offer, or why it has none. One outside a
    /// repository has no checkout, and one Styra made no branch for has
    /// nowhere recorded to measure from; neither is guessed at.
    pub fn of(interaction: &InteractionSummary) -> Result<Self, &'static str> {
        let checkout = interaction
            .checkout
            .as_ref()
            .ok_or("this interaction has no Git checkout to diff")?;
        let branched_from = interaction
            .branched_from
            .as_ref()
            .ok_or("no recorded start commit to diff this checkout against")?;
        Ok(Self {
            worktree: checkout.worktree.clone(),
            branch: checkout.branch.clone(),
            base_branch: branched_from.branch.clone(),
            base: branched_from.commit.clone(),
        })
    }
}

/// A diff read for the in-window view, and how far it is scrolled.
pub struct CheckoutDiff {
    pub target: DiffTarget,
    /// `git diff`'s output, or why it could not be read.
    pub diff: Result<String, String>,
    pub scroll: Scroll,
}

impl CheckoutDiff {
    /// Read the diff now. It is read once, when the view opens, rather than
    /// on every frame: opening the view again is what refreshes it.
    pub fn read(target: DiffTarget) -> Self {
        let diff = git_diff(&target.worktree, &target.base);
        Self {
            target,
            diff,
            scroll: Scroll::default(),
        }
    }
}

/// The working tree against `base`, so what the agent has not committed yet
/// is shown alongside what it has. Colour and external drivers are turned
/// off: the view styles the diff itself, and needs Git's plain format to.
fn git_diff(worktree: &Path, base: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--no-color", "--no-ext-diff"])
        .arg(base)
        .arg("--")
        .output()
        .map_err(|error| format!("could not run git diff: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff failed: {}", stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(directory: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// Committed and uncommitted work both count: the diff is of the working
    /// tree, not of the branch's last commit.
    #[test]
    fn the_diff_covers_work_committed_since_the_start_and_work_not_yet_committed() {
        let root = std::env::temp_dir().join("styra-checkout-diff");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "start"]);
        let start = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.txt"), "one\ncommitted\n").unwrap();
        git(&root, &["commit", "-q", "-am", "more"]);
        std::fs::write(root.join("a.txt"), "one\ncommitted\nloose\n").unwrap();

        let read = CheckoutDiff::read(DiffTarget {
            worktree: root.clone(),
            branch: None,
            base_branch: None,
            base: start,
        });

        let diff = read.diff.unwrap();
        assert!(diff.contains("+committed"), "{diff}");
        assert!(diff.contains("+loose"), "{diff}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_start_commit_git_does_not_know_is_reported_rather_than_shown_as_empty() {
        let root = std::env::temp_dir().join("styra-checkout-diff-unknown");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);

        let read = CheckoutDiff::read(DiffTarget {
            worktree: root.clone(),
            branch: None,
            base_branch: None,
            base: "0123456789abcdef0123456789abcdef01234567".into(),
        });

        assert!(read.diff.unwrap_err().starts_with("git diff failed"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

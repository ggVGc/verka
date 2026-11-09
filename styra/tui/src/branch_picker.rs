//! Fuzzy branch pickers: the branch a new interaction's checkout starts
//! from, and the branch an existing one is retargeted onto.
//!
//! Without a base, a new checkout starts from the linked checkout carried
//! over with `n`, or else from the Workspace's own. Choosing a base branch
//! instead makes the launch a fresh checkout of its own, branched from that
//! branch's tip and integrating back into it.
//!
//! Retargeting lists the branches the interaction worked on before first,
//! choosing one of which returns to it, then every other local branch,
//! choosing one of which rebases onto it.

use std::path::Path;
use std::process::Command;

use crossterm::event::{KeyCode, KeyEvent};
use styra_ui::fuzzy_list::FuzzyList;
use styra_ui::query::Edit;

/// The local branches of the repository `directory` is in, the one with the
/// newest commit first — the order the work being picked from was done in.
/// Read here rather than through the server: there is no interaction yet to
/// ask about, and the blank screen already reads its Workspace's branch the
/// same way.
pub fn local_branches(directory: &Path) -> anyhow::Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "for-each-ref",
            "--sort=refname",
            "--sort=-committerdate",
            "--format=%(refname:strip=2)",
            "refs/heads/",
        ])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git could not list branches: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// A list of branches typed at, newest commit first. Each row stands for a
/// branch, or for none at all where a picker offers a way back to no choice.
pub struct BranchPicker {
    pub rows: Vec<String>,
    branches: Vec<Option<String>>,
    /// For retargeting, which rows name a branch the interaction worked on
    /// before, returned to rather than retargeted onto. `None` for a base.
    previous: Option<Vec<bool>>,
    pub list: FuzzyList,
}

pub enum BranchChoice {
    Stay,
    Cancel,
    /// A row of [`BranchPicker::base`]: `None` is its first row, starting
    /// where the launch would without a base.
    Base(Option<String>),
    /// A row of [`BranchPicker::retarget`] naming a branch to rebase onto.
    Retarget(String),
    /// A row of [`BranchPicker::retarget`] naming a previous branch.
    Restore(String),
}

impl BranchPicker {
    /// The base of a new interaction's checkout. `default` names where the
    /// launch starts without a choice, when known. The list opens on
    /// `chosen`, the base already picked, if it is listed.
    pub fn base(default: Option<&str>, branches: Vec<String>, chosen: Option<&str>) -> Self {
        let mut rows = vec![match default {
            Some(branch) => format!("Default ({branch})"),
            None => "Default".to_owned(),
        }];
        rows.extend(branches.iter().cloned());
        let mut values = vec![None];
        values.extend(branches.into_iter().map(Some));
        let row = chosen
            .and_then(|chosen| values.iter().position(|v| v.as_deref() == Some(chosen)))
            .unwrap_or(0);
        Self::new(rows, values, None, row)
    }

    /// The branch to move an interaction to. `previous` is its branch
    /// history, oldest first, and is listed first, most recent first;
    /// `branches` follow it, newest commit first. `current`, the branch it
    /// is on, is neither.
    pub fn retarget(previous: &[String], branches: Vec<String>, current: Option<&str>) -> Self {
        let mut rows = Vec::new();
        let mut values = Vec::new();
        let mut flags = Vec::new();
        for branch in previous.iter().rev() {
            if Some(branch.as_str()) == current || values.contains(&Some(branch.clone())) {
                continue;
            }
            rows.push(format!("{branch} · previous branch"));
            values.push(Some(branch.clone()));
            flags.push(true);
        }
        for branch in branches {
            if Some(branch.as_str()) == current || values.contains(&Some(branch.clone())) {
                continue;
            }
            rows.push(branch.clone());
            values.push(Some(branch));
            flags.push(false);
        }
        Self::new(rows, values, Some(flags), 0)
    }

    fn new(
        rows: Vec<String>,
        branches: Vec<Option<String>>,
        previous: Option<Vec<bool>>,
        row: usize,
    ) -> Self {
        Self {
            list: FuzzyList::at(&rows, row).keeping_order(),
            rows,
            branches,
            previous,
        }
    }

    /// What the list is for, said in its border.
    pub fn title(&self) -> &'static str {
        match self.previous {
            None => " Base branch · newest commits first ",
            Some(_) => " Retarget onto branch · previous branches, then newest commits ",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> BranchChoice {
        match key.code {
            KeyCode::Esc => return BranchChoice::Cancel,
            KeyCode::Enter => {
                if let Some(index) = self.list.selected_row(&self.rows) {
                    let branch = self.branches[index].clone();
                    return match (&self.previous, branch) {
                        (None, branch) => BranchChoice::Base(branch),
                        (Some(previous), Some(branch)) if previous[index] => {
                            BranchChoice::Restore(branch)
                        }
                        (Some(_), Some(branch)) => BranchChoice::Retarget(branch),
                        (Some(_), None) => BranchChoice::Stay,
                    };
                }
            }
            KeyCode::Down => self.list.next(&self.rows),
            KeyCode::Up => self.list.prev(&self.rows),
            KeyCode::PageDown => self.list.page_down(&self.rows),
            KeyCode::PageUp => self.list.page_up(&self.rows),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    self.list.edit(edit);
                }
            }
        }
        BranchChoice::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(picker: &mut BranchPicker, code: KeyCode) -> BranchChoice {
        picker.handle_key(KeyEvent::from(code))
    }

    #[test]
    fn typing_narrows_without_reordering_and_enter_picks_the_branch() {
        let mut picker = BranchPicker::base(
            Some("main (Workspace)"),
            vec!["z-new-topic".into(), "a-old-topic".into(), "other".into()],
            None,
        );
        assert_eq!(picker.rows[0], "Default (main (Workspace))");
        for c in "topic".chars() {
            press(&mut picker, KeyCode::Char(c));
        }
        let shown: Vec<_> = picker
            .list
            .matches(&picker.rows)
            .iter()
            .map(|m| m.index)
            .collect();
        assert_eq!(shown, vec![1, 2], "newest commit first, as listed");
        press(&mut picker, KeyCode::Down);
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            BranchChoice::Base(Some(branch)) if branch == "a-old-topic"
        ));
        picker.list.query = "nothing-matches".into();
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            BranchChoice::Stay
        ));
        assert!(matches!(
            press(&mut picker, KeyCode::Esc),
            BranchChoice::Cancel
        ));
    }

    #[test]
    fn the_first_row_clears_the_choice_and_reopening_starts_on_it() {
        let branches = vec!["main".to_owned(), "topic".to_owned()];
        let mut picker = BranchPicker::base(None, branches.clone(), Some("topic"));
        assert_eq!(picker.list.selected_row(&picker.rows), Some(2));
        press(&mut picker, KeyCode::Up);
        press(&mut picker, KeyCode::Up);
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            BranchChoice::Base(None)
        ));
        let unlisted = BranchPicker::base(None, branches, Some("gone"));
        assert_eq!(unlisted.list.selected_row(&unlisted.rows), Some(0));
    }

    #[test]
    fn retargeting_lists_previous_branches_first_and_leaves_out_the_current() {
        let previous = vec!["styra/first".to_owned(), "styra/second".to_owned()];
        let branches = vec![
            "main".to_owned(),
            "styra/now".to_owned(),
            "styra/first".to_owned(),
            "release".to_owned(),
        ];
        let mut picker = BranchPicker::retarget(&previous, branches, Some("styra/now"));
        assert_eq!(
            picker.rows,
            vec![
                "styra/second · previous branch",
                "styra/first · previous branch",
                "main",
                "release",
            ]
        );
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            BranchChoice::Restore(branch) if branch == "styra/second"
        ));
        for c in "rel".chars() {
            press(&mut picker, KeyCode::Char(c));
        }
        assert!(matches!(
            press(&mut picker, KeyCode::Enter),
            BranchChoice::Retarget(branch) if branch == "release"
        ));
    }

    #[test]
    fn branches_are_listed_newest_commit_first() {
        let root = std::env::temp_dir().join(format!("styra-base-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str], date: &str| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .env("GIT_COMMITTER_DATE", date)
                .env("GIT_AUTHOR_DATE", date)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"], "2026-01-01T00:00:00");
        git(
            &["commit", "-q", "--allow-empty", "-m", "a"],
            "2026-01-01T00:00:00",
        );
        git(&["switch", "-q", "-c", "newer"], "2026-01-01T00:00:00");
        git(
            &["commit", "-q", "--allow-empty", "-m", "b"],
            "2026-03-01T00:00:00",
        );
        git(
            &["switch", "-q", "-c", "middle", "main"],
            "2026-01-01T00:00:00",
        );
        git(
            &["commit", "-q", "--allow-empty", "-m", "c"],
            "2026-02-01T00:00:00",
        );

        assert_eq!(
            local_branches(&root).unwrap(),
            vec!["newer", "middle", "main"]
        );
        assert!(local_branches(&root.join("no-such-directory")).is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}

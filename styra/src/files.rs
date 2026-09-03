//! Files the agent touched or named, and the operator's place in that list.
//!
//! Two things live here. The first is the [`FilesView`] selection. The second
//! is turning a path as the agent reported it into a path on this host, which
//! is not obvious: the agent names files inside its sandbox, so an absolute
//! path has to be re-rooted at the Workspace's host directory, and a path
//! outside the sandbox has to be left exactly as it is.
//!
//! That resolution was written twice — once in `App::selected_file_path` to
//! decide what `e` opens, and once in [`crate::ui::files`] to decide what to
//! draw — including the grouping and sort that pair a row with its file. Two
//! copies of an ordering is one copy too many when a disagreement between them
//! means opening the wrong file.

use std::path::{Path, PathBuf};

use styra_server::agent::SandboxLayout;
use styra_server::event::{AgentEvent, DetailBlock};

use crate::timeline::Entry;

/// Which file the Files view has selected, and whether it is listing the whole
/// session or only the focused entry.
#[derive(Default)]
pub struct FilesView {
    selected: usize,
    show_all: bool,
}

impl FilesView {
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn shows_all(&self) -> bool {
        self.show_all
    }

    /// `last` is the index of the final row, which the caller knows because
    /// only it can say how many files the current scope found.
    pub fn select_next(&mut self, last: usize) {
        self.selected = self.selected.saturating_add(1).min(last);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Return to the first row, for the moves that replace the list under the
    /// selection rather than moving within it — changing the focused entry,
    /// or `g`.
    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self, last: usize) {
        self.selected = last;
    }

    /// Adopt a scope chosen on a previous screen. The selection is not
    /// adopted with it: it indexes into that screen's file list, not this
    /// one's. See [`crate::app::OperatorState`].
    pub fn set_scope(&mut self, show_all: bool) {
        self.show_all = show_all;
        self.selected = 0;
    }

    /// Widen to the whole session or narrow to the focused entry. The
    /// selection returns to the first row, since the list it indexed into is
    /// being replaced by a different one.
    pub fn toggle_scope(&mut self) {
        self.show_all = !self.show_all;
        self.selected = 0;
    }
}

/// One file as the list shows it: the spelling the agent used, where that
/// lands on this host, and the root it is displayed beneath.
pub struct FileItem {
    pub reported: String,
    pub resolved: PathBuf,
    pub root: PathBuf,
    pub relative: PathBuf,
}

/// Turn a path as the agent reported it into a path on this host.
///
/// An absolute path inside the agent's sandbox names the same file as
/// `root`-relative on the host, so its sandbox prefix is swapped for `root`.
/// An absolute path outside the sandbox is already a host path and is left
/// alone. A relative path is relative to the Workspace.
pub fn resolve(root: &Path, reported: &str) -> PathBuf {
    let path = Path::new(reported);
    if path.is_absolute() {
        match path.strip_prefix(&SandboxLayout::default().workspace) {
            Ok(relative) => root.join(relative),
            Err(_) => path.to_path_buf(),
        }
    } else {
        root.join(path)
    }
}

/// Resolve and group every reported path, in the order the list shows them.
///
/// Files under the Workspace are grouped beneath it; anything else is grouped
/// beneath its own parent directory, so an external file is still named
/// somewhere sensible rather than under a root it is not in.
///
/// The one place this ordering is decided. The renderer draws this order and
/// the `e` key indexes into it, so they cannot disagree about which row is
/// which file.
pub fn items(root: &Path, reported: Vec<String>) -> Vec<FileItem> {
    let mut items: Vec<_> = reported
        .into_iter()
        .map(|reported| {
            let resolved = resolve(root, &reported);
            let (item_root, relative) = match resolved.strip_prefix(root) {
                Ok(relative) => (root.to_path_buf(), relative.to_path_buf()),
                Err(_) => {
                    let external = resolved.parent().unwrap_or(Path::new("/")).to_path_buf();
                    let relative = resolved
                        .strip_prefix(&external)
                        .unwrap_or(&resolved)
                        .to_path_buf();
                    (external, relative)
                }
            };
            FileItem {
                reported,
                resolved,
                root: item_root,
                relative,
            }
        })
        .collect();
    items.sort_by(|a, b| (&a.root, &a.relative).cmp(&(&b.root, &b.relative)));
    items
}

/// Files explicitly touched by an event, plus path-like text mentions that
/// currently resolve to files on disk.
///
/// Paths retain their reported spelling, so the renderer can still tell a
/// Workspace-relative mention from an external one. The `is_file` check is
/// what keeps ordinary prose out of the list: plenty of words contain a dot,
/// but almost none of them name a file that exists.
pub fn mentioned<'a>(entries: impl Iterator<Item = &'a Entry>, root: Option<&Path>) -> Vec<String> {
    let mut paths = Vec::new();
    for entry in entries {
        if let AgentEvent::FileChanged { paths: changed, .. } = &entry.event {
            paths.extend(changed.iter().cloned());
        }
        let mut text = entry.event.summary();
        for block in entry.event.detail() {
            text.push('\n');
            match block {
                DetailBlock::Text(part) | DetailBlock::Code { text: part, .. } => {
                    text.push_str(&part)
                }
            }
        }
        for token in text.split_whitespace() {
            let Some(candidate) = path_like(token) else {
                continue;
            };
            // Without a Workspace there is nothing to resolve a relative
            // mention against, so only absolute ones can be confirmed.
            let resolved = match (Path::new(candidate).is_absolute(), root) {
                (true, Some(root)) => resolve(root, candidate),
                // Nothing to re-root against, so an absolute mention can only
                // be checked as the host path it already spells.
                (true, None) => PathBuf::from(candidate),
                (false, Some(root)) => root.join(candidate),
                (false, None) => continue,
            };
            if resolved.is_file() {
                paths.push(candidate.to_owned());
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// A token stripped of the punctuation prose wraps paths in, if what is left
/// could be a path at all.
fn path_like(token: &str) -> Option<&str> {
    let candidate = token.trim_matches(|ch: char| {
        matches!(
            ch,
            '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':' | ';'
        )
    });
    let path_like = !candidate.is_empty() && (candidate.contains('/') || candidate.contains('.'));
    path_like.then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> PathBuf {
        SandboxLayout::default().workspace
    }

    /// The agent names files inside its sandbox; the operator's editor opens
    /// them on the host. The same file has two absolute paths and this is
    /// where they are reconciled.
    #[test]
    fn a_sandbox_path_is_re_rooted_at_the_workspace_on_this_host() {
        let resolved = resolve(
            Path::new("/home/me/project"),
            sandbox().join("src/main.rs").to_str().unwrap(),
        );

        assert_eq!(resolved, PathBuf::from("/home/me/project/src/main.rs"));
    }

    #[test]
    fn a_relative_path_is_taken_against_the_workspace() {
        let resolved = resolve(Path::new("/home/me/project"), "src/main.rs");

        assert_eq!(resolved, PathBuf::from("/home/me/project/src/main.rs"));
    }

    /// A path the agent reached outside its sandbox is already a host path,
    /// and re-rooting it would name a file that does not exist.
    #[test]
    fn an_absolute_path_outside_the_sandbox_is_left_alone() {
        let resolved = resolve(Path::new("/home/me/project"), "/etc/hosts");

        assert_eq!(resolved, PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn workspace_files_are_grouped_under_it_and_others_under_their_own_parent() {
        let root = Path::new("/home/me/project");
        let items = items(
            root,
            vec![
                "src/main.rs".into(),
                "/etc/hosts".into(),
                "README.md".into(),
            ],
        );

        let grouped: Vec<_> = items
            .iter()
            .map(|item| (item.root.clone(), item.relative.clone()))
            .collect();
        assert_eq!(
            grouped,
            vec![
                (PathBuf::from("/etc"), PathBuf::from("hosts")),
                (root.to_path_buf(), PathBuf::from("README.md")),
                (root.to_path_buf(), PathBuf::from("src/main.rs")),
            ],
            "external roots sort before the Workspace, and each root's files by path"
        );
    }

    /// The renderer draws this order and `e` indexes into it, so a row and the
    /// file it opens are the same thing by construction.
    #[test]
    fn the_reported_spelling_survives_grouping() {
        let items = items(Path::new("/work"), vec!["src/main.rs".into()]);

        assert_eq!(items[0].reported, "src/main.rs");
        assert_eq!(items[0].resolved, PathBuf::from("/work/src/main.rs"));
    }

    #[test]
    fn punctuation_around_a_path_in_prose_is_stripped() {
        assert_eq!(path_like("`src/main.rs`"), Some("src/main.rs"));
        assert_eq!(path_like("(src/main.rs)"), Some("src/main.rs"));
        assert_eq!(path_like("src/main.rs,"), Some("src/main.rs"));
    }

    #[test]
    fn a_word_that_could_not_be_a_path_is_not_one() {
        assert_eq!(path_like("running"), None);
        assert_eq!(path_like("``"), None);
        // A trailing full stop is not trimmed: it is as likely to be part of
        // a filename as it is to be the end of a sentence, so the `is_file`
        // check settles it rather than a guess here.
        assert_eq!(path_like("word."), Some("word."));
    }

    #[test]
    fn the_selection_holds_at_the_last_row_and_resets_when_the_scope_changes() {
        let mut view = FilesView::default();
        view.select_next(2);
        view.select_next(2);
        view.select_next(2);
        assert_eq!(view.selected_index(), 2);

        view.toggle_scope();

        assert!(view.shows_all());
        assert_eq!(view.selected_index(), 0, "a different list, from its start");
    }
}

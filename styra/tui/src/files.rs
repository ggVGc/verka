//! Files the agent touched or named, and the operator's place in that list.
//!
//! Two things live here. The first is the [`FilesView`] selection. The second
//! is turning a path as the agent reported it into a path on this host, which
//! is not obvious: the agent names files inside its sandbox, so an absolute
//! path has to be re-rooted at the Workspace's host directory, and a path
//! outside the sandbox has to be left exactly as it is.
//!
//! That resolution was written twice — once in `App::selected_file_path` to
//! decide what `e` opens, and once in [`crate::presentation::files`] to decide what to
//! draw — including the grouping and sort that pair a row with its file. Two
//! copies of an ordering is one copy too many when a disagreement between them
//! means opening the wrong file.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::path::{Path, PathBuf};

use styra_protocol::agent::SandboxLayout;
use styra_protocol::event::{AgentEvent, DetailBlock};

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

/// Find the file a link names on this host, or say why there is none.
///
/// Unlike [`resolve`], this looks. A relative link is tried against `start`,
/// the directory the interaction is working in, and then against each
/// directory above it in turn, up to and including `workspace`. An agent
/// working in a subdirectory names some files relative to where it stands and
/// others relative to the top of the project, and both should open.
///
/// The search never climbs past the Workspace: a match above it would be a
/// file the agent had no business naming. When `start` is not inside the
/// Workspace at all, the two are tried on their own. The one exception is
/// `repository`, the root of the Git checkout the Workspace is part of, tried
/// last: agents name files from the top of the repository out of habit, and
/// that may be above a Workspace made in one of its subdirectories.
///
/// An absolute link names one file and is only re-rooted, as [`resolve`]
/// does, against the Workspace (or `start`, when there is none).
pub fn locate(
    start: &Path,
    workspace: Option<&Path>,
    repository: Option<&Path>,
    reported: &str,
) -> Result<PathBuf, String> {
    let root = workspace.unwrap_or(start);
    if Path::new(reported).is_absolute() {
        let path = resolve(root, reported);
        return if path.exists() {
            Ok(path)
        } else {
            Err(format!("{}: no such file", path.display()))
        };
    }
    let mut directories: Vec<&Path> = if start.starts_with(root) {
        start
            .ancestors()
            .take_while(|directory| directory.starts_with(root))
            .collect()
    } else {
        vec![start, root]
    };
    let repository = repository.filter(|repository| !directories.contains(repository));
    directories.extend(repository);
    directories
        .iter()
        .map(|directory| directory.join(reported))
        .find(|path| path.exists())
        .ok_or_else(|| {
            let searched = if start == root {
                format!("{reported}: not found in {}", root.display())
            } else {
                format!(
                    "{reported}: not found in {} or any directory up to {}",
                    start.display(),
                    root.display()
                )
            };
            match repository {
                Some(repository) => format!("{searched}, nor in {}", repository.display()),
                None => searched,
            }
        })
}

/// The top of the Git checkout `path` is in: the nearest directory, `path`
/// itself included, holding a `.git` — a directory in a main checkout, a file
/// in a linked worktree.
pub fn enclosing_checkout(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|directory| directory.join(".git").exists())
        .map(Path::to_path_buf)
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
        if let AgentEvent::FileChanged { paths: changed, .. } = entry.event() {
            paths.extend(changed.iter().cloned());
        }
        for candidate in candidates(&entry_text(entry)) {
            let candidate = candidate.as_str();
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

/// Everything one entry says, as one blob of text: its summary and every
/// detail block under it.
///
/// The text a path can be named in, and the one definition of it — both this
/// module's own scan for mentions and [`crate::references`]'s scan for
/// `path:line` references read an entry through here, so neither can look at
/// less of it than the other.
pub fn entry_text(entry: &Entry) -> String {
    let mut text = entry.event().summary();
    for block in entry.event().detail() {
        text.push('\n');
        match block {
            DetailBlock::Text(part) | DetailBlock::Code { text: part, .. } => text.push_str(&part),
        }
    }
    text
}

/// Every stretch of `text` that could be a path, in the order it appears.
///
/// Agent replies are markdown, so they are read as markdown: the same
/// `pulldown-cmark` the message renderer uses ([`crate::presentation`]) walks the text,
/// and a link's destination is taken as the parser reports it. That is the
/// whole reason for parsing rather than scanning — in
/// `[monitor.c:484](/home/me/src/monitor.c:484)` the destination *is* the
/// path, with no syntax left around it to guess at, and a scan of the raw text
/// would see one token that looks like nothing at all.
///
/// Prose and code spans are still tokenized on whitespace, because a bare
/// citation in a sentence is not markup and the parser hands those back as
/// plain text. What that leaves for [`path_like`] is only the punctuation a
/// sentence ends with.
pub fn candidates(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut push = |token: &str| {
        if let Some(candidate) = path_like(token) {
            found.push(candidate.to_owned());
        }
    };
    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH) {
        match event {
            // Both halves of a citation reach the list: the destination here,
            // and the link's text as the `Event::Text` inside it. They name one
            // file, which the callers settle by the path each resolves to.
            Event::Start(Tag::Link { dest_url, .. }) => push(&dest_url),
            Event::Text(text) | Event::Code(text) => text.split_whitespace().for_each(&mut push),
            _ => {}
        }
    }
    found
}

/// A token stripped of the punctuation prose wraps paths in, if what is left
/// could be a path at all.
///
/// A colon is trimmed only at the ends, where it is punctuation. Inside a
/// token it separates a path from the line it cites (`monitor.c:484`) and is
/// [`crate::references`]'s to read.
pub fn path_like(token: &str) -> Option<&str> {
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
mod tests;

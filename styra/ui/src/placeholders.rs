//! Short stand-ins for the long directories most paths on screen sit under.
//!
//! A Workspace's paths and its linked worktrees' paths all share one long
//! prefix each, and repeated in full they bury the part that tells them apart:
//! the worktree's own name, the file under the workspace. Each prefix is
//! written as a placeholder instead, and spelled out once where it is defined.

#[cfg(feature = "renderer")]
use ratatui::text::Line;
use std::path::Path;

/// The Workspace's own directory.
pub const WORKSPACE: &str = "<ws>";
/// The directory the server parks a Workspace's linked worktrees in.
pub const WORKTREES: &str = "<wt>";

/// The prefixes to replace, each with its placeholder.
#[derive(Clone, Debug, Default)]
pub struct Placeholders {
    prefixes: Vec<(String, &'static str)>,
}

impl Placeholders {
    /// Also write `path` as `placeholder`. Ignored when there is no path, or
    /// when it is `/`, which every path is under.
    pub fn with(mut self, path: Option<&Path>, placeholder: &'static str) -> Self {
        let Some(path) = path.and_then(Path::to_str) else {
            return self;
        };
        let path = path.trim_end_matches('/');
        if !path.is_empty() {
            self.prefixes.push((path.to_owned(), placeholder));
            // Longest first, so a prefix nested inside another is taken as the
            // more specific of the two.
            self.prefixes
                .sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        }
        self
    }

    /// `text` with every whole occurrence of a prefix, or of a path under it,
    /// written as its placeholder.
    pub fn apply(&self, text: &str) -> String {
        self.prefixes
            .iter()
            .fold(text.to_owned(), |text, (prefix, placeholder)| {
                replace_path(&text, prefix, placeholder)
            })
    }

    /// [`Self::apply`] over every span of `lines`.
    #[cfg(feature = "renderer")]
    pub fn lines(&self, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        if self.prefixes.is_empty() {
            return lines;
        }
        lines
            .into_iter()
            .map(|mut line| {
                for span in &mut line.spans {
                    if self
                        .prefixes
                        .iter()
                        .any(|(prefix, _)| span.content.contains(prefix.as_str()))
                    {
                        span.content = self.apply(&span.content).into();
                    }
                }
                line
            })
            .collect()
    }
}

/// `text` with each occurrence of `path` that is a whole path, or the leading
/// part of one, replaced by `placeholder`. `/a/proj` is not taken out of
/// `/a/project` or `/b/a/proj`.
fn replace_path(text: &str, path: &str, placeholder: &str) -> String {
    let part_of_path = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(path) {
        let before = rest[..at].chars().last().or_else(|| out.chars().last());
        let after = rest[at + path.len()..].chars().next();
        out.push_str(&rest[..at]);
        let whole = !before.is_some_and(part_of_path)
            && !after.is_some_and(|c| c != '/' && part_of_path(c));
        out.push_str(if whole { placeholder } else { path });
        rest = &rest[at + path.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Placeholders {
        Placeholders::default().with(Some(Path::new("/home/me/proj")), WORKSPACE)
    }

    #[test]
    fn only_whole_paths_are_replaced() {
        let ws = workspace();
        assert_eq!(ws.apply("/home/me/proj"), "<ws>");
        assert_eq!(
            ws.apply("/home/me/proj → /home/me/proj/src (rw)"),
            "<ws> → <ws>/src (rw)"
        );
        assert_eq!(ws.apply("/home/me/project"), "/home/me/project");
        assert_eq!(ws.apply("/mnt/home/me/proj"), "/mnt/home/me/proj");
        assert_eq!(ws.apply("PWD=/home/me/proj:"), "PWD=<ws>:");
    }

    #[test]
    fn a_worktree_is_named_under_its_own_placeholder() {
        let both = workspace().with(Some(Path::new("/store/workspaces/w1/worktrees")), WORKTREES);
        assert_eq!(
            both.apply(
                "/store/workspaces/w1/worktrees/fix-retries/src · linked from /home/me/proj"
            ),
            "<wt>/fix-retries/src · linked from <ws>"
        );
    }

    #[test]
    fn the_more_specific_of_two_nested_prefixes_wins() {
        let nested = workspace().with(Some(Path::new("/home/me/proj/.worktrees")), WORKTREES);
        assert_eq!(nested.apply("/home/me/proj/.worktrees/a"), "<wt>/a");
        assert_eq!(nested.apply("/home/me/proj/src"), "<ws>/src");
    }
}

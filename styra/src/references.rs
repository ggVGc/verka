//! File references inside one reply, and the operator's choice among them.
//!
//! Agents cite their work by location: `monitor.c:484`, commonly as a markdown
//! link to the same file spelled in full —
//! `[monitor.c:484](/home/me/src/monitor.c:484)`. Those citations are the
//! fastest route from
//! reading a reply to reading the code it is about, so they are collected into
//! a list the operator picks from and the choice is handed to the configured
//! opener.
//!
//! Distinct from [`crate::files`], which lists what a whole session touched:
//! this is one reply's citations, `path:line` forms included, offered as a
//! modal over whatever the operator was reading when they asked.
//!
//! The line number is carried for display only. What opens the file is the
//! operator's configured program ([`crate::config::Configuration`]), and only
//! that program knows how it is told to jump to a line — so nothing here
//! invents a syntax for saying so.

use std::path::{Path, PathBuf};

use crate::files;

/// One cited file: the spelling the reply used, the line it named, and where
/// that file is on this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    /// As the reply wrote it, line number and all — what the row shows, so the
    /// operator recognises the citation they just read.
    pub reported: String,
    pub line: Option<u32>,
    /// The host path the opener is given; see [`files::resolve`].
    pub resolved: PathBuf,
}

impl Reference {
    /// The row's label: the citation as written, with the resolved path after
    /// it when the two name different places — which is exactly when the reply
    /// named a file inside the agent's sandbox, or relative to the Workspace.
    ///
    /// The line number is not a difference, so a citation that already spells
    /// its host path is shown once rather than pointed at itself.
    pub fn label(&self) -> String {
        let resolved = self.resolved.display().to_string();
        if split_location(&self.reported).0 == resolved {
            self.reported.clone()
        } else {
            format!("{}  →  {resolved}", self.reported)
        }
    }
}

/// Every file `text` cites that exists on this host, in the order it cites
/// them.
///
/// Existence is the filter that keeps prose out, as it is for
/// [`files::mentioned`]: plenty of words survive punctuation-stripping, but
/// almost none of them name a file. Duplicates are dropped by the file they
/// resolve to rather than by their spelling, because a reply that writes a
/// path twice — short, then in full — is citing one file.
pub fn in_reply(text: &str, root: Option<&Path>) -> Vec<Reference> {
    let mut references: Vec<Reference> = Vec::new();
    for candidate in files::candidates(text) {
        let (path, line) = split_location(&candidate);
        let Some(path) = files::path_like(path) else {
            continue;
        };
        let resolved = match (Path::new(path).is_absolute(), root) {
            (true, Some(root)) => files::resolve(root, path),
            // Nothing to re-root against, so an absolute citation can only be
            // checked as the host path it already spells.
            (true, None) => PathBuf::from(path),
            (false, Some(root)) => root.join(path),
            (false, None) => continue,
        };
        if !resolved.is_file() {
            continue;
        }
        if references
            .iter()
            .any(|reference| reference.resolved == resolved)
        {
            continue;
        }
        references.push(Reference {
            reported: candidate,
            line,
            resolved,
        });
    }
    references
}

/// Split a citation into the file it names and the line it points at.
///
/// The path is everything before the first colon: a colon in a filename is
/// rare enough that reading one as the start of a location is right far more
/// often than not.
fn split_location(candidate: &str) -> (&str, Option<u32>) {
    let mut parts = candidate.split(':');
    let path = parts.next().unwrap_or(candidate);
    let line = parts.next().and_then(|part| part.parse().ok());
    (path, line)
}

/// The open list of one reply's references and the row the operator is on.
///
/// Only built from a non-empty list ([`References::new`] returns `None`
/// otherwise), so the modal can never be on screen with nothing to choose.
pub struct References {
    references: Vec<Reference>,
    selected: usize,
}

impl References {
    pub fn new(references: Vec<Reference>) -> Option<Self> {
        (!references.is_empty()).then_some(Self {
            references,
            selected: 0,
        })
    }

    pub fn items(&self) -> &[Reference] {
        &self.references
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The reference under the cursor. Infallible by construction: the list is
    /// never empty and the selection never leaves it.
    pub fn selected(&self) -> &Reference {
        &self.references[self.selected]
    }

    pub fn select_next(&mut self) {
        let last = self.references.len() - 1;
        self.selected = self.selected.saturating_add(1).min(last);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.references.len() - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_protocol::agent::SandboxLayout;

    /// A host directory holding `src/monitor.c`, standing in for a Workspace.
    fn tree(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("styra-references-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/monitor.c"), "int main(void) { }\n").unwrap();
        std::fs::canonicalize(base).unwrap()
    }

    /// The form agents actually write: the short citation, then the same file
    /// in full. Both name one file, so the list offers it once.
    #[test]
    fn a_cited_line_and_the_full_path_beside_it_are_one_reference() {
        let root = tree("cited");
        let text = format!(
            "Fixed the crash in src/monitor.c:484 ({}/src/monitor.c:484).",
            root.display()
        );

        let references = in_reply(&text, Some(&root));

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].reported, "src/monitor.c:484");
        assert_eq!(references[0].line, Some(484));
        assert_eq!(references[0].resolved, root.join("src/monitor.c"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The form the citation usually arrives in: a markdown link whose text is
    /// the short citation and whose target is the host path. One
    /// whitespace-delimited token, two paths, one file.
    #[test]
    fn a_markdown_link_to_a_cited_line_is_one_reference() {
        let root = tree("markdown");
        let file = root.join("src/monitor.c").display().to_string();
        let text = format!("At [monitor.c:484]({file}:484), every reply increments `num_wired`.");

        let references = in_reply(&text, Some(&root));

        assert_eq!(references.len(), 1, "{references:?}");
        assert_eq!(references[0].resolved, root.join("src/monitor.c"));
        assert_eq!(references[0].line, Some(484));
        assert_eq!(
            references[0].label(),
            format!("{file}:484"),
            "a citation already spelling its host path is not pointed at itself"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A citation of a file inside the agent's sandbox names a file on this
    /// host under the Workspace, and the row says so.
    #[test]
    fn a_sandbox_citation_is_re_rooted_and_shown_alongside_its_host_path() {
        let root = tree("sandbox");
        let sandboxed = SandboxLayout::default()
            .workspace
            .join("src/monitor.c")
            .display()
            .to_string();

        let references = in_reply(&format!("see {sandboxed}:12"), Some(&root));

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].resolved, root.join("src/monitor.c"));
        assert_eq!(references[0].line, Some(12));
        assert!(
            references[0].label().contains("→"),
            "{}",
            references[0].label()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Prose is full of dotted words and none of them are files.
    #[test]
    fn text_that_names_no_existing_file_yields_no_references() {
        let root = tree("prose");

        assert!(in_reply(
            "Done. Nothing else needed, e.g. src/absent.c:3",
            Some(&root)
        )
        .is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_citation_without_a_line_is_still_a_reference() {
        let root = tree("bare");

        let references = in_reply("touched src/monitor.c today", Some(&root));

        assert_eq!(references.len(), 1);
        assert_eq!(references[0].line, None);
        assert_eq!(references[0].label(), {
            let resolved = root.join("src/monitor.c").display().to_string();
            format!("src/monitor.c  →  {resolved}")
        });

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_selection_cannot_leave_the_references_that_exist() {
        let reference = |path: &str| Reference {
            reported: path.into(),
            line: None,
            resolved: PathBuf::from(path),
        };
        let mut open = References::new(vec![reference("/a"), reference("/b")]).unwrap();

        for _ in 0..5 {
            open.select_next();
        }
        assert_eq!(open.selected().reported, "/b");

        for _ in 0..5 {
            open.select_prev();
        }
        assert_eq!(open.selected().reported, "/a");

        open.select_last();
        assert_eq!(open.selected_index(), 1);
        open.select_first();
        assert_eq!(open.selected_index(), 0);
    }

    #[test]
    fn an_empty_list_opens_no_picker() {
        assert!(References::new(Vec::new()).is_none());
    }
}

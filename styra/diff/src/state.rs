//! What a diff view compares, the diff it read, and how it is being looked
//! at: scrolled, split per file, filtered. [`crate::keys`] turns keys into
//! [`Action`]s, so what each one does is decided here once.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::Block;
use std::path::PathBuf;
use styra_ui::scroll::Scroll;
use styra_ui::search::Search;

use crate::files::{filtered_file_diffs, position, FileDiff};
use crate::view::{rows, CheckoutDiffFeedback, CheckoutDiffView, Row, Tint};

/// What a diff of a checkout compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffTarget {
    /// Root of the working tree the diff is read in.
    pub worktree: PathBuf,
    /// The branch checked out there; `None` when its head is detached.
    pub branch: Option<String>,
    /// The branch the base was taken from, for saying so; `None` when there
    /// was none, as with a head detached at the time.
    pub base_branch: Option<String>,
    /// The commit diffed against.
    pub base: String,
    /// Whether [`Self::base`] is the commit the checkout's branch was made at
    /// — the branch it was made from having moved on since — rather than
    /// just a commit to compare with.
    pub branch_point: bool,
}

/// What a key can ask of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Start typing a file-name filter. Per-file view only.
    Search,
    /// Switch between the combined diff and one file's patch at a time.
    ToggleFiles,
    ToggleRemoved,
    /// Step the added and removed rows' bands through full, half, and none.
    CycleTint,
    /// Fold the file tree's directories that hold no files of their own and
    /// a single subdirectory onto one row, or give each its own again.
    ToggleCompactDirs,
    /// The next file in the per-file view; otherwise a line down.
    Down,
    /// The previous file in the per-file view; otherwise a line up.
    Up,
    /// Five files on. Per-file view only.
    FilesDown,
    /// Five files back. Per-file view only.
    FilesUp,
    /// Ten lines down.
    ScrollDown,
    /// Ten lines up.
    ScrollUp,
    /// Half of what is on screen down.
    HalfPageDown,
    /// Half of what is on screen up.
    HalfPageUp,
    First,
    Last,
}

/// A diff read for the view, and how far it is scrolled.
pub struct CheckoutDiff {
    pub target: DiffTarget,
    /// `git diff`'s output, or why it could not be read.
    pub diff: Result<String, String>,
    /// The branches and tags at the base commit, read with the diff.
    pub base_names: Vec<String>,
    pub scroll: Scroll,
    /// The combined diff's scroll, put by while the per-file view is shown so
    /// going back to it returns to the same place.
    pub combined_scroll: Scroll,
    pub per_file: bool,
    pub hide_removed: bool,
    pub compact_dirs: bool,
    pub tint: Tint,
    pub selected_file: usize,
    pub file_list_offset: usize,
    pub search: Search,
}

impl CheckoutDiff {
    /// Read the diff now. It is read when the view opens rather than on every
    /// frame: opening the view again refreshes it, and so can the host every
    /// few seconds while the checkout is being worked on (see
    /// [`Self::refresh`]).
    pub fn read(target: DiffTarget) -> Self {
        let diff = crate::git::diff(&target.worktree, &target.base);
        let base_names = crate::git::names_at(&target.worktree, &target.base);
        Self {
            target,
            diff,
            base_names,
            scroll: Scroll::default(),
            combined_scroll: Scroll::default(),
            per_file: false,
            hide_removed: false,
            compact_dirs: true,
            tint: Tint::Full,
            selected_file: 0,
            file_list_offset: 0,
            search: Default::default(),
        }
    }

    /// Read the diff again, keeping the view where it is: the same file stays
    /// selected while it is still in the diff, and the scroll is left for the
    /// renderer to hold to what now fits. Reports whether the diff changed.
    pub fn refresh(&mut self) -> bool {
        let diff = crate::git::diff(&self.target.worktree, &self.target.base);
        if diff == self.diff {
            return false;
        }
        let selected = self.selected_path();
        self.diff = diff;
        let files = self.visible_files();
        let kept = selected.and_then(|path| {
            files
                .iter()
                .find(|(_, file)| file.path == path)
                .map(|(index, _)| *index)
        });
        let next = kept.or_else(|| files.first().map(|(index, _)| *index));
        drop(files);
        match next {
            Some(next) if next == self.selected_file => {}
            Some(next) => {
                self.selected_file = next;
                if kept.is_none() {
                    self.scroll.reset();
                }
            }
            None => self.selected_file = 0,
        }
        true
    }

    fn selected_path(&self) -> Option<String> {
        self.visible_files()
            .into_iter()
            .find(|(index, _)| *index == self.selected_file)
            .map(|(_, file)| file.path.to_owned())
    }

    /// The view to draw this diff as, inside `block`.
    pub fn view<'a>(&'a self, block: Block<'a>) -> CheckoutDiffView<'a> {
        CheckoutDiffView {
            block,
            worktree: &self.target.worktree,
            branch: self.target.branch.as_deref(),
            base_branch: self.target.base_branch.as_deref(),
            base_commit: &self.target.base,
            base_names: &self.base_names,
            branch_point: self.target.branch_point,
            diff: match &self.diff {
                Ok(text) => Ok(text),
                Err(error) => Err(error),
            },
            requested_scroll: self.scroll.clamped(),
            per_file: self.per_file,
            hide_removed: self.hide_removed,
            compact_dirs: self.compact_dirs,
            tint: self.tint,
            selected_file: self.selected_file,
            file_list_offset: self.file_list_offset,
            search: self.search.view(),
        }
    }

    /// Keep what drawing the view measured, for the next keys to move by.
    pub fn note(&mut self, measured: CheckoutDiffFeedback) {
        self.scroll.offset = measured.scroll.effective_scroll;
        self.scroll.note_limit(measured.scroll.limit);
        self.scroll.note_viewport(measured.scroll.viewport);
        self.file_list_offset = measured.file_list_offset;
    }

    pub(crate) fn apply(&mut self, action: Action) {
        match action {
            Action::Search if self.per_file => {
                self.search.open();
                self.update_search();
            }
            // Each file's patch opens at its top; the combined diff opens
            // where it was left.
            Action::ToggleFiles => {
                self.per_file = !self.per_file;
                std::mem::swap(&mut self.scroll, &mut self.combined_scroll);
                if self.per_file {
                    self.scroll.reset();
                }
            }
            Action::ToggleRemoved => {
                let hidden = self.hide_removed;
                let top = self.scroll.clamped();
                if let Some(offset) = self.shown_text().map(|text| keep_top(text, hidden, top)) {
                    self.scroll.offset = offset;
                }
                if self.per_file {
                    let top = self.combined_scroll.clamped();
                    if let Ok(offset) = self.diff.as_deref().map(|text| keep_top(text, hidden, top))
                    {
                        self.combined_scroll.offset = offset;
                    }
                }
                self.hide_removed = !hidden;
            }
            // The selected file stays selected, and the list keeps it in view.
            Action::CycleTint => self.tint = self.tint.next(),
            Action::ToggleCompactDirs => self.compact_dirs = !self.compact_dirs,
            Action::Down if self.per_file => self.select_file(true, 1),
            Action::Up if self.per_file => self.select_file(false, 1),
            Action::FilesDown if self.per_file => self.select_file(true, 5),
            Action::FilesUp if self.per_file => self.select_file(false, 5),
            Action::Down => self.scroll.line_down(),
            Action::Up => self.scroll.line_up(),
            Action::ScrollDown => self.scroll.page_down(),
            Action::ScrollUp => self.scroll.page_up(),
            Action::HalfPageDown => self.scroll.half_page_down(),
            Action::HalfPageUp => self.scroll.half_page_up(),
            Action::First => self.scroll.reset(),
            Action::Last => self.scroll.scroll_to_end(),
            Action::Search | Action::FilesDown | Action::FilesUp => {}
        }
    }

    /// The file-name filter's prompt takes every key while it is open, the
    /// letters bound to commands included.
    pub(crate) fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.search.cancel(),
            KeyCode::Enter => self.search.accept(),
            KeyCode::Backspace => self.search.backspace(),
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search.push(character)
            }
            _ => {}
        }
        self.update_search();
    }

    /// Drop a standing file-name filter, saying whether there was one: Esc
    /// clears it before it means anything else to the host.
    pub(crate) fn clear_filter(&mut self) -> bool {
        if !(self.per_file && self.search.query().is_some()) {
            return false;
        }
        self.search.cancel();
        self.update_search();
        true
    }

    /// The file the view is on, and the line to open it at: the file whose
    /// patch the top visible row is in — the selected file, in the per-file
    /// view — and the new file's line at that row, if it is past the file's
    /// headers. A file the diff deletes is not there to open.
    pub fn location(&self) -> Result<(PathBuf, Option<u32>), String> {
        let text = self.shown_text().ok_or("no diff to open a file from")?;
        let rows = rows(text, self.hide_removed);
        let top = line_at(&rows, usize::from(self.scroll.clamped())).ok_or("no file to open")?;
        let (file, line) = position(text, top).ok_or("no file to open")?;
        let path = self.target.worktree.join(file);
        if !path.exists() {
            return Err(format!("{file} is deleted in the working tree"));
        }
        Ok((path, line))
    }

    /// The patch the view shows: the whole diff, or the selected file's in
    /// the per-file view — the first file shown when the selection is
    /// filtered out, as the view falls back to it.
    fn shown_text(&self) -> Option<&str> {
        let text = self.diff.as_ref().ok()?;
        if !self.per_file {
            return Some(text);
        }
        let files = filtered_file_diffs(text, self.search.query());
        files
            .iter()
            .find(|(index, _)| *index == self.selected_file)
            .or_else(|| files.first())
            .map(|(_, file)| file.patch)
    }

    fn visible_files(&self) -> Vec<(usize, FileDiff<'_>)> {
        self.diff
            .as_ref()
            .map(|text| filtered_file_diffs(text, self.search.query()))
            .unwrap_or_default()
    }

    /// Keep a visible selection as the filter changes, retaining its file
    /// identity when matches before it disappear or return.
    fn update_search(&mut self) {
        self.file_list_offset = 0;
        let files = self.visible_files();
        if files.iter().any(|(index, _)| *index == self.selected_file) {
            return;
        }
        let next = files.first().map(|(index, _)| *index);
        if let Some(next) = next {
            self.selected_file = next;
        }
        self.scroll.reset();
    }

    fn select_file(&mut self, down: bool, count: usize) {
        let files = self.visible_files();
        let position = files
            .iter()
            .position(|(index, _)| *index == self.selected_file)
            .unwrap_or(0);
        let next = if down {
            position
                .saturating_add(count)
                .min(files.len().saturating_sub(1))
        } else {
            position.saturating_sub(count)
        };
        let next = files.get(next).map(|(index, _)| *index);
        if let Some(next) = next.filter(|next| *next != self.selected_file) {
            self.selected_file = next;
            self.scroll.reset();
        }
    }
}

/// The offset that keeps what is at the top of `text`'s rows there once its
/// removed lines are shown again, if `hidden`, or hidden: the line at the
/// top, or the first after it that stays, with a rule above it kept too.
fn keep_top(text: &str, hidden: bool, top: u16) -> u16 {
    let before = rows(text, hidden);
    let after = rows(text, !hidden);
    let top = usize::from(top);
    let offset = match line_at(&before, top) {
        Some(line) => {
            let mut offset = after
                .iter()
                .position(|row| matches!(row, Row::Line(index) if *index >= line))
                .unwrap_or(after.len());
            if offset > 0 && before.get(top) == after.get(offset - 1) {
                offset -= 1;
            }
            offset
        }
        None => after.len(),
    };
    u16::try_from(offset).unwrap_or(u16::MAX)
}

/// The diff line at row `top`, or — on a rule — the first one after it.
fn line_at(rows: &[Row], top: usize) -> Option<usize> {
    rows.get(top..)?.iter().find_map(|row| match row {
        Row::Line(index) => Some(*index),
        Row::RuleAbove | Row::RuleBelow => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::removed_rows;

    fn two_files() -> CheckoutDiff {
        let mut diff = CheckoutDiff {
            target: DiffTarget {
                worktree: PathBuf::from("/w"),
                branch: None,
                base_branch: None,
                base: "4bf5c35d".into(),
                branch_point: true,
            },
            diff: Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                      diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n"
                .into()),
            base_names: Vec::new(),
            scroll: Scroll::default(),
            combined_scroll: Scroll::default(),
            per_file: false,
            hide_removed: false,
            compact_dirs: true,
            tint: Tint::Full,
            selected_file: 0,
            file_list_offset: 0,
            search: Search::default(),
        };
        diff.scroll.note_limit(100);
        diff
    }

    /// A file appearing ahead of the selected one moves its index, not the
    /// selection: the refreshed view stays on the file being read.
    #[test]
    fn a_refresh_picks_up_new_work_and_keeps_the_selected_file() {
        fn git(directory: &std::path::Path, args: &[&str]) -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }
        let root = std::env::temp_dir().join("styra-checkout-diff-refresh");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("b.txt"), "one\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "start"]);
        let start = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("b.txt"), "two\n").unwrap();
        let mut diff = CheckoutDiff::read(DiffTarget {
            worktree: root.clone(),
            branch: None,
            base_branch: None,
            base: start,
            branch_point: true,
        });
        diff.scroll.offset = 3;

        assert!(!diff.refresh());
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        assert!(diff.refresh());

        assert!(diff.diff.as_ref().unwrap().contains("a/a.txt"));
        assert_eq!(diff.selected_path().as_deref(), Some("b.txt"));
        assert_eq!(diff.scroll.offset, 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same keys scroll the combined diff and walk the files of the
    /// per-file one; the file-only actions do nothing in the combined diff.
    #[test]
    fn up_and_down_move_through_lines_or_files_by_view() {
        let mut diff = two_files();
        diff.apply(Action::Down);
        assert_eq!((diff.scroll.offset, diff.selected_file), (1, 0));
        diff.apply(Action::FilesDown);
        diff.apply(Action::Search);
        assert_eq!((diff.scroll.offset, diff.selected_file), (1, 0));
        assert!(!diff.search.typing());

        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.scroll.offset, 0);
        diff.apply(Action::Down);
        assert_eq!((diff.scroll.offset, diff.selected_file), (0, 1));
        diff.apply(Action::Up);
        assert_eq!(diff.selected_file, 0);
    }

    /// The file to open is the one the top row is in, at the new file's line
    /// there, counted the same with removed lines hidden; the per-file view
    /// opens its selected file; a deleted file is not opened.
    #[test]
    fn the_location_is_the_file_and_line_at_the_top() {
        let root = std::env::temp_dir().join("styra-diff-location");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.rs"), "").unwrap();
        let mut diff = two_files();
        diff.target.worktree = root.clone();
        // Rows: 0 a.rs's header, 1 its rule, 2-4 its metadata and hunk,
        // 5 " keep" (10), 6 "-old", 7 "+new" (11), 8 " after" (12), 9 the
        // rule above gone.rs's header, 10 that header.
        let text = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n\
                    @@ -1 +0,0 @@\n-gone\n";
        assert_eq!(
            removed_rows(text).len(),
            text.lines().count(),
            "the view draws a row for each line"
        );
        diff.diff = Ok(text.into());
        let a = root.join("a.rs");

        assert_eq!(diff.location(), Ok((a.clone(), None)));
        diff.scroll.offset = 1;
        assert_eq!(
            diff.location(),
            Ok((a.clone(), None)),
            "a rule is its file's"
        );
        diff.scroll.offset = 5;
        assert_eq!(diff.location(), Ok((a.clone(), Some(10))));
        diff.scroll.offset = 6;
        assert_eq!(diff.location(), Ok((a.clone(), Some(11))), "where -old was");
        diff.hide_removed = true;
        assert_eq!(
            diff.location(),
            Ok((a.clone(), Some(11))),
            "+new, row 6 of those shown"
        );
        diff.hide_removed = false;
        for top in [9, 10] {
            diff.scroll.offset = top;
            assert_eq!(
                diff.location(),
                Err("gone.rs is deleted in the working tree".into()),
                "the rule above a header belongs to the file below it"
            );
        }

        diff.apply(Action::ToggleFiles);
        diff.apply(Action::Down);
        assert_eq!(diff.selected_file, 1);
        assert!(
            diff.location().is_err(),
            "the per-file view opens its selected file"
        );
        diff.apply(Action::Up);
        assert_eq!(diff.location(), Ok((a, None)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Hiding and showing removed lines keeps the row at the top in place. A
    /// removed row that goes is replaced there by the next row that stays,
    /// and a rule at the top stays there.
    #[test]
    fn toggling_removed_lines_keeps_the_top_row() {
        let mut diff = two_files();
        // Rows: header, its rule, hunk, " a", "-b", "-c", "+B", " d".
        diff.diff = Ok("diff --git a/f b/f\n@@ -1,4 +1,3 @@\n a\n-b\n-c\n+B\n d\n".into());
        let mut toggled = |top: u16| {
            diff.scroll.offset = top;
            diff.apply(Action::ToggleRemoved);
            let hidden = diff.scroll.offset;
            diff.apply(Action::ToggleRemoved);
            (hidden, diff.scroll.offset)
        };
        assert_eq!(toggled(3), (3, 3), "\" a\" stays where it was");
        assert_eq!(toggled(4), (4, 6), "\"-b\" goes; \"+B\" takes the top");
        assert_eq!(toggled(5), (4, 6));
        assert_eq!(toggled(7), (5, 7));
        assert_eq!(toggled(1), (1, 1), "the header's rule");
        assert_eq!(toggled(0), (0, 0));
    }

    /// Going back to the combined diff from the per-file view returns to the
    /// row it was left at, hidden removed lines and all.
    #[test]
    fn the_combined_diff_keeps_its_place_across_the_per_file_view() {
        let mut diff = two_files();
        // Rows: a.txt's 0-4, the rule above b.txt's header 5, the header 6,
        // its rule 7, hunk 8, "-c" 9, "+d" 10.
        diff.scroll.offset = 8;
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.scroll.offset, 0, "a file's patch opens at its top");
        diff.apply(Action::Down);
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.scroll.offset, 8);

        diff.scroll.offset = 10;
        diff.apply(Action::ToggleFiles);
        diff.apply(Action::ToggleRemoved);
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.scroll.offset, 8, "\"+d\" is row 8 of those shown");
        diff.apply(Action::ToggleFiles);
        diff.apply(Action::ToggleRemoved);
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.scroll.offset, 10);
    }

    #[test]
    fn a_standing_filter_is_cleared_before_anything_else() {
        let mut diff = two_files();
        assert!(
            !diff.clear_filter(),
            "nothing to clear in the combined diff"
        );
        diff.apply(Action::ToggleFiles);
        diff.apply(Action::Search);
        diff.search_key(KeyEvent::from(KeyCode::Char('b')));
        assert_eq!(diff.selected_file, 1, "the selection follows the filter");
        diff.search_key(KeyEvent::from(KeyCode::Enter));

        assert!(diff.clear_filter());
        assert_eq!(diff.search.query(), None);
        assert!(!diff.clear_filter());
    }
}

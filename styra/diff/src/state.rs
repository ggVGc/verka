//! What a diff view compares, the diff it read, and how it is being looked
//! at: scrolled, split per file, filtered. [`crate::keys`] turns keys into
//! [`Action`]s, so what each one does is decided here once.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::Block;
use std::path::{Path, PathBuf};
use styra_ui::scroll::Scroll;
use styra_ui::search::Search;

use crate::files::{filtered_file_diffs, position, FileDiff};
use crate::layers::Layers;
use crate::report::ReportState;
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
    /// The commit or snapshot tree diffed against.
    pub base: String,
    /// Whether [`Self::base`] is the commit the checkout's branch was made at
    /// — the branch it was made from having moved on since — rather than
    /// just a commit to compare with.
    pub branch_point: bool,
}

/// Where a diff view's diff comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Read from Git, and read again on [`CheckoutDiff::refresh`].
    Checkout(DiffTarget),
    /// Handed to the view whole — a turn's snapshot diff, say, or a patch
    /// piped into `styra-diff` — and changed only by
    /// [`CheckoutDiff::replace`].
    Supplied(Supplied),
}

/// What a supplied diff is, for saying so, and where its files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Supplied {
    /// What the diff is of, said where a checkout's diff names its branches.
    pub label: String,
    /// The directory the diff's paths are relative to, for opening its files
    /// and measuring them; `None` when they are not on this host.
    pub root: Option<PathBuf>,
}

/// What a key can ask of the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Start typing a file-name filter. Per-file view only.
    Search,
    /// Switch between the combined diff and one file's patch at a time.
    ToggleFiles,
    ToggleRemoved,
    ToggleLayout,
    PanLeft,
    PanRight,
    /// Step the added and removed rows' bands through full, half, and none.
    CycleTint,
    ToggleWords,
    /// Put the per-file view's thumbnail away, or bring it back.
    ToggleThumbnail,
    /// Tint each change by the commits that made it, or by whether it adds
    /// or removes again.
    ToggleLayers,
    /// Put the per-file view's edit report away, or bring it back.
    ToggleReport,
    /// Fold the file tree's directories that hold no files of their own and
    /// a single subdirectory onto one row, or give each its own again.
    ToggleCompactDirs,
    /// Put a cursor on the line at the top of the patch shown, or take it
    /// away. While it is there, the moving keys move it rather than the view.
    ToggleLines,
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
    pub source: Source,
    /// `git diff`'s output, or the supplied diff, or why there is none.
    pub diff: Result<String, String>,
    /// The branches and tags at the base commit, read with the diff. Always
    /// empty for a supplied diff.
    pub base_names: Vec<String>,
    pub scroll: Scroll,
    /// The combined diff's scroll, put by while the per-file view is shown so
    /// going back to it returns to the same place.
    pub combined_scroll: Scroll,
    pub per_file: bool,
    /// Requested layout; temporarily unified when the patch area is narrow.
    pub side_by_side: bool,
    /// Layout used by the last frame, which defines the scroll coordinates.
    pub rendered_side: bool,
    /// Layout of the combined scroll saved while a single file is shown.
    pub combined_side: bool,
    pub horizontal_scroll: u16,
    pub hide_removed: bool,
    pub compact_dirs: bool,
    pub tint: Tint,
    pub word_highlights: bool,
    /// Whether the per-file view's thumbnail of the selected file's changes
    /// has been put away, where the host shows one.
    pub thumbnail_hidden: bool,
    /// The commits the diff was made in, or why they could not be read,
    /// while its changes are tinted by them; `None` otherwise.
    pub layers: Option<Result<Layers, String>>,
    pub selected_file: usize,
    pub file_list_offset: usize,
    pub search: Search,
    /// What each file was edited for, once the host has had it described.
    pub report: ReportState,
    /// The line the line cursor is on, by its place among the shown patch's
    /// lines; `None` unless lines are being selected.
    pub line_cursor: Option<usize>,
}

impl CheckoutDiff {
    /// Show the file list, retaining the combined diff's scroll position.
    pub fn show_files(&mut self) {
        if !self.per_file {
            self.apply(Action::ToggleFiles);
        }
    }

    /// Focus a changed file, optionally at a working-tree line present in a
    /// hunk. An unmatched location leaves the view untouched.
    pub fn jump_to(&mut self, path: &Path, line: Option<u32>) -> Result<(), String> {
        let root = self
            .root()
            .ok_or("this diff's files are not on this host")?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("{} is outside this diff's checkout", path.display()))?;
        let text = self.diff.as_deref().map_err(Clone::clone)?;
        let (index, file) = crate::files::file_diffs(text)
            .into_iter()
            .enumerate()
            .find(|(_, file)| Path::new(file.path) == relative)
            .ok_or_else(|| format!("{} has no change in this diff", relative.display()))?;
        let row = match line {
            Some(line) => crate::files::row_at_line(file.patch, line).ok_or_else(|| {
                format!(
                    "{}:{line} is outside this diff's changed hunks",
                    relative.display()
                )
            })?,
            None => 0,
        };
        let offset =
            crate::split::source_rows(file.patch, self.hide_removed, self.rendered_side, true)
                .iter()
                .position(|shown| matches!(shown, Row::Line(index) if *index == row))
                .unwrap_or(0);
        if !self.per_file {
            std::mem::swap(&mut self.scroll, &mut self.combined_scroll);
            self.combined_side = self.rendered_side;
        }
        self.per_file = true;
        self.search.cancel();
        self.selected_file = index;
        self.file_list_offset = 0;
        // The cursor's line was counted in the patch shown before.
        self.line_cursor = self.line_cursor.map(|_| row);
        self.scroll.offset = u16::try_from(offset).unwrap_or(u16::MAX);
        // The old viewport's limit belongs to a different patch. Let the
        // next render measure this one before clamping the requested row.
        self.scroll.note_limit(u16::MAX);
        Ok(())
    }

    /// Read the diff now. It is read when the view opens rather than on every
    /// frame: opening the view again refreshes it, and so can the host every
    /// few seconds while the checkout is being worked on (see
    /// [`Self::refresh`]).
    pub fn read(target: DiffTarget) -> Self {
        let diff = crate::git::diff(&target.worktree, &target.base);
        let base_names = crate::git::names_at(&target.worktree, &target.base);
        Self::new(Source::Checkout(target), diff, base_names)
    }

    /// Show `diff` — a Git-style patch, one `diff --git` header per file —
    /// rather than reading one from a checkout. [`Self::replace`] puts a
    /// newer one in its place.
    pub fn supplied(diff: String, supplied: Supplied) -> Self {
        Self::new(Source::Supplied(supplied), Ok(diff), Vec::new())
    }

    fn new(source: Source, diff: Result<String, String>, base_names: Vec<String>) -> Self {
        Self {
            source,
            diff,
            base_names,
            scroll: Scroll::default(),
            combined_scroll: Scroll::default(),
            per_file: false,
            side_by_side: false,
            rendered_side: false,
            combined_side: false,
            horizontal_scroll: 0,
            hide_removed: false,
            compact_dirs: true,
            tint: Tint::Full,
            word_highlights: true,
            layers: None,
            thumbnail_hidden: false,
            selected_file: 0,
            file_list_offset: 0,
            search: Default::default(),
            report: ReportState::default(),
            line_cursor: None,
        }
    }

    /// What a checkout's diff compares; `None` for a supplied diff.
    pub fn target(&self) -> Option<&DiffTarget> {
        match &self.source {
            Source::Checkout(target) => Some(target),
            Source::Supplied(_) => None,
        }
    }

    /// The directory the diff's paths are relative to, if it is on this host.
    pub fn root(&self) -> Option<&Path> {
        match &self.source {
            Source::Checkout(target) => Some(&target.worktree),
            Source::Supplied(supplied) => supplied.root.as_deref(),
        }
    }

    /// Read a checkout's diff again, keeping the view where it is: the same
    /// file stays selected while it is still in the diff, and the scroll is
    /// left for the renderer to hold to what now fits. Reports whether the
    /// diff, or the commits it is tinted by, changed: a commit can take in
    /// what was uncommitted and leave the diff as it was. A supplied diff has
    /// nothing to read again: it changes only by [`Self::replace`].
    pub fn refresh(&mut self) -> bool {
        let Source::Checkout(target) = &self.source else {
            return false;
        };
        let diff = crate::git::diff(&target.worktree, &target.base);
        self.base_names = crate::git::names_at(&target.worktree, &target.base);
        let mut layered = false;
        if self.layers.is_some() {
            if let Some(layers) =
                Layers::reread(self.layers.as_ref(), &target.worktree, &target.base)
            {
                self.layers = Some(layers);
                layered = true;
            }
        }
        self.replace(diff) || layered
    }

    /// Show `diff` in place of the one shown, keeping the view where it is as
    /// [`Self::refresh`] does. Reports whether the diff changed.
    pub fn replace(&mut self, diff: Result<String, String>) -> bool {
        if diff == self.diff {
            return false;
        }
        let selected = self.selected_path();
        // The cursor follows its file's line rather than its place in the
        // diff, which the new work may have moved.
        let cursor = self.line_cursor.and_then(|line| {
            let (path, number) = position(self.shown_text()?, line)?;
            Some((path.to_owned(), number?))
        });
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
        if self.line_cursor.is_some() {
            self.line_cursor = cursor.and_then(|(path, number)| self.line_at_number(&path, number));
        }
        true
    }

    /// Where line `number` of `path`'s new file is among the shown patch's
    /// lines, if a hunk of it is shown.
    fn line_at_number(&self, path: &str, number: u32) -> Option<usize> {
        let text = self.shown_text()?;
        let (start, file) = file_starts(text)
            .into_iter()
            .find(|(_, file)| file.path == path)?;
        Some(start + crate::files::row_at_line(file.patch, number)?)
    }

    fn selected_path(&self) -> Option<String> {
        self.visible_files()
            .into_iter()
            .find(|(index, _)| *index == self.selected_file)
            .map(|(_, file)| file.path.to_owned())
    }

    /// The view to draw this diff as, inside `block`.
    pub fn view<'a>(&'a self, block: Block<'a>) -> CheckoutDiffView<'a> {
        let target = self.target();
        let supplied = match &self.source {
            Source::Supplied(supplied) => Some(supplied),
            Source::Checkout(_) => None,
        };
        CheckoutDiffView {
            block,
            worktree: self.root().unwrap_or(Path::new("")),
            branch: target.and_then(|target| target.branch.as_deref()),
            base_branch: target.and_then(|target| target.base_branch.as_deref()),
            base_commit: target.map_or("", |target| &target.base),
            base_names: &self.base_names,
            branch_point: target.is_some_and(|target| target.branch_point),
            label: supplied.map(|supplied| supplied.label.as_str()),
            diff: match &self.diff {
                Ok(text) => Ok(text),
                Err(error) => Err(error),
            },
            requested_scroll: self.scroll.clamped(),
            per_file: self.per_file,
            side_by_side: self.side_by_side,
            rendered_side: self.rendered_side,
            horizontal_scroll: self.horizontal_scroll,
            thumbnail: false,
            thumbnail_hidden: self.thumbnail_hidden,
            hide_removed: self.hide_removed,
            compact_dirs: self.compact_dirs,
            tint: self.tint,
            word_highlights: self.word_highlights,
            layers: self.layers.as_ref().map(|layers| match layers {
                Ok(layers) => Ok(layers),
                Err(error) => Err(error.as_str()),
            }),
            selected_file: self.selected_file,
            file_list_offset: self.file_list_offset,
            search: self.search.view(),
            report: Some(&self.report),
            conversation: None,
            intro: None,
            line_cursor: self.line_cursor,
        }
    }

    /// Keep what drawing the view measured, for the next keys to move by.
    pub fn note(&mut self, measured: CheckoutDiffFeedback) {
        self.rendered_side = measured.side_by_side;
        self.horizontal_scroll = measured.horizontal_scroll;
        self.scroll.offset = measured.scroll.effective_scroll;
        self.scroll.note_limit(measured.scroll.limit);
        self.scroll.note_viewport(measured.scroll.viewport);
        self.file_list_offset = measured.file_list_offset;
    }

    pub(crate) fn apply(&mut self, action: Action) {
        let lines = self.line_cursor.is_some();
        match action {
            Action::Search if self.per_file => {
                self.line_cursor = None;
                self.search.open();
                self.update_search();
            }
            // Each file's patch opens at its top; the combined diff opens
            // where it was left. A line cursor goes along to the same line.
            Action::ToggleFiles => {
                let cursor = self.cursor_in_diff();
                self.per_file = !self.per_file;
                std::mem::swap(&mut self.rendered_side, &mut self.combined_side);
                std::mem::swap(&mut self.scroll, &mut self.combined_scroll);
                if self.per_file {
                    self.scroll.reset();
                }
                self.line_cursor = None;
                if let Some((file, line)) = cursor {
                    self.place_cursor(file, line);
                }
            }
            Action::ToggleLines if lines => self.line_cursor = None,
            Action::ToggleLines => {
                self.line_cursor = self.shown_text().and_then(|text| {
                    let rows = crate::split::source_rows(
                        text,
                        self.hide_removed,
                        self.rendered_side,
                        self.per_file,
                    );
                    line_at(&rows, usize::from(self.scroll.clamped()))
                });
                // Onto the first change from the top of the view.
                self.move_cursor(0);
            }
            Action::Down if lines => self.move_cursor(1),
            Action::Up if lines => self.move_cursor(-1),
            // The arrows go over every line drawn, headers and cuts too.
            Action::ScrollDown if lines => self.move_cursor_over(1, false),
            Action::ScrollUp if lines => self.move_cursor_over(-1, false),
            Action::HalfPageDown if lines => self.move_cursor(self.scroll.half_page() as isize),
            Action::HalfPageUp if lines => self.move_cursor(-(self.scroll.half_page() as isize)),
            Action::FilesDown if lines => self.move_to_block(true),
            Action::FilesUp if lines => self.move_to_block(false),
            Action::First if lines => self.move_cursor(isize::MIN),
            Action::Last if lines => self.move_cursor(isize::MAX),
            Action::ToggleRemoved => {
                let hidden = self.hide_removed;
                if let Some(text) = self.shown_text() {
                    self.scroll.offset = if self.rendered_side {
                        crate::split::remap(
                            text,
                            hidden,
                            true,
                            !hidden,
                            false,
                            self.scroll.clamped(),
                            self.per_file,
                        )
                    } else {
                        keep_top(text, hidden, self.scroll.clamped(), self.per_file)
                    };
                }
                self.rendered_side = false;
                if self.per_file {
                    if let Ok(text) = &self.diff {
                        self.combined_scroll.offset = if self.combined_side {
                            crate::split::remap(
                                text,
                                hidden,
                                true,
                                !hidden,
                                false,
                                self.combined_scroll.clamped(),
                                false,
                            )
                        } else {
                            keep_top(text, hidden, self.combined_scroll.clamped(), false)
                        };
                    }
                    self.combined_side = false;
                    self.combined_scroll.note_limit(u16::MAX);
                }
                self.hide_removed = !hidden;
                self.scroll.note_limit(u16::MAX);
                // Off a removed line that is now hidden.
                self.move_cursor_over(0, false);
            }
            Action::ToggleLayout => self.side_by_side = !self.side_by_side,
            Action::PanLeft => self.horizontal_scroll = self.horizontal_scroll.saturating_sub(8),
            Action::PanRight => self.horizontal_scroll = self.horizontal_scroll.saturating_add(8),
            // The selected file stays selected, and the list keeps it in view.
            Action::ToggleWords => self.word_highlights = !self.word_highlights,
            Action::ToggleThumbnail => self.thumbnail_hidden = !self.thumbnail_hidden,
            Action::ToggleLayers => {
                self.layers = match (&self.layers, self.target()) {
                    (Some(_), _) => None,
                    (None, Some(target)) => Some(Layers::read(&target.worktree, &target.base)),
                    (None, None) => Some(Err("a supplied diff has no commits to layer".into())),
                }
            }
            Action::CycleTint => self.tint = self.tint.next(),
            Action::ToggleCompactDirs => self.compact_dirs = !self.compact_dirs,
            Action::ToggleReport => self.report.hidden = !self.report.hidden,
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

    /// The file the view is on, and the line to open it at: the line
    /// cursor's, when lines are being selected; otherwise the file whose
    /// patch the top visible row is in — the selected file, in the per-file
    /// view — and the new file's line at that row, if it is past the file's
    /// headers. A file the diff deletes is not there to open.
    pub fn location(&self) -> Result<(PathBuf, Option<u32>), String> {
        let text = self.shown_text().ok_or("no diff to open a file from")?;
        let top = match self.line_cursor {
            Some(line) => line,
            None => {
                let rows = crate::split::source_rows(
                    text,
                    self.hide_removed,
                    self.rendered_side,
                    self.per_file,
                );
                line_at(&rows, usize::from(self.scroll.clamped())).ok_or("no file to open")?
            }
        };
        let (file, line) = position(text, top).ok_or("no file to open")?;
        let root = self
            .root()
            .ok_or("this diff's files are not on this host")?;
        let path = root.join(file);
        if !path.exists() {
            return Err(format!("{file} is deleted in the working tree"));
        }
        Ok((path, line))
    }

    /// What the view is on, for a message written about it: the file the
    /// line cursor is in and the new file's line there, or else the per-file
    /// view's file. `None` in the combined diff with no line selected, which
    /// is on no file in particular. The path is the worktree's.
    pub fn source(&self) -> Option<(&str, Option<u32>)> {
        match self.line_cursor {
            Some(line) => position(self.shown_text()?, line),
            None if self.per_file => self.shown_file().map(|(_, file)| (file.path, None)),
            None => None,
        }
    }

    /// Leave line selection, saying whether lines were being selected: Esc
    /// does that before it means anything else to the host.
    pub(crate) fn leave_lines(&mut self) -> bool {
        self.line_cursor.take().is_some()
    }

    /// Move the line cursor `step` lines down, or up when negative, through
    /// the added and removed lines drawn, and scroll it into view: unchanged
    /// lines, file headers and hunk cuts are stepped over, so going on from
    /// one block of changes is going to the next. A step of zero only puts
    /// it back on such a line — the next one, from anywhere else — and in
    /// view.
    fn move_cursor(&mut self, step: isize) {
        self.move_cursor_over(step, true);
    }

    /// [`Self::move_cursor`], through the changed lines only or through every
    /// line drawn.
    fn move_cursor_over(&mut self, step: isize, changes_only: bool) {
        let (Some(cursor), Some(text)) = (self.line_cursor, self.shown_text()) else {
            return;
        };
        let (hidden, side, per_file) = (self.hide_removed, self.rendered_side, self.per_file);
        let changed = changed_lines(text);
        let drawn: Vec<(usize, usize)> = crate::split::source_rows(text, hidden, side, per_file)
            .into_iter()
            .enumerate()
            .filter_map(|(row, shown)| match shown {
                Row::Line(line) if !changes_only || changed.get(line).copied().unwrap_or(false) => {
                    Some((row, line))
                }
                Row::Line(_) | Row::RuleAbove | Row::RuleBelow => None,
            })
            .collect();
        let at = crate::split::row_of(text, hidden, side, per_file, cursor);
        let last = drawn.len().saturating_sub(1);
        let current = drawn.iter().position(|(row, _)| *row >= at).unwrap_or(last);
        // From a line not among them, reaching the next one is a step down.
        let step = match drawn.get(current) {
            Some(&(row, _)) if row != at && step > 0 => step - 1,
            _ => step,
        };
        let Some(&(row, line)) = drawn.get(current.saturating_add_signed(step).min(last)) else {
            return;
        };
        self.line_cursor = Some(line);
        self.scroll.reveal(u16::try_from(row).unwrap_or(u16::MAX));
    }

    /// Move the line cursor to the first line of the next block of changes,
    /// or when `down` is false of the previous one — of the block it is in,
    /// from inside one — and scroll it into view. A block is a run of added
    /// and removed lines; its first line is the first of them drawn. With no
    /// block that way the cursor stays.
    fn move_to_block(&mut self, down: bool) {
        let (Some(cursor), Some(text)) = (self.line_cursor, self.shown_text()) else {
            return;
        };
        let (hidden, side, per_file) = (self.hide_removed, self.rendered_side, self.per_file);
        let blocks = change_blocks(text);
        let mut starts: Vec<(usize, usize)> = Vec::new();
        let mut last_block = None;
        for (row, shown) in crate::split::source_rows(text, hidden, side, per_file)
            .into_iter()
            .enumerate()
        {
            let Row::Line(line) = shown else { continue };
            let Some(block) = blocks.get(line).copied().flatten() else {
                continue;
            };
            if last_block != Some(block) {
                starts.push((row, line));
                last_block = Some(block);
            }
        }
        let at = crate::split::row_of(text, hidden, side, per_file, cursor);
        let target = if down {
            starts.into_iter().find(|&(row, _)| row > at)
        } else {
            starts.into_iter().filter(|&(row, _)| row < at).last()
        };
        if let Some((row, line)) = target {
            self.line_cursor = Some(line);
            self.scroll.reveal(u16::try_from(row).unwrap_or(u16::MAX));
        }
    }

    /// The file the line cursor is in, by its place in the whole diff, and
    /// the cursor's line within that file's patch.
    fn cursor_in_diff(&self) -> Option<(usize, usize)> {
        let line = self.line_cursor?;
        if self.per_file {
            return self.shown_file().map(|(index, _)| (index, line));
        }
        file_starts(self.diff.as_ref().ok()?)
            .into_iter()
            .enumerate()
            .find(|(_, (start, file))| (*start..start + file.patch.lines().count()).contains(&line))
            .map(|(index, (start, _))| (index, line - start))
    }

    /// Put the line cursor on line `line` of file `file`'s patch, in the
    /// whole diff or as that file's own, unless the file filter hides it.
    fn place_cursor(&mut self, file: usize, line: usize) {
        if self.per_file {
            if !self.visible_files().iter().any(|(index, _)| *index == file) {
                return;
            }
            self.selected_file = file;
            self.scroll.note_limit(u16::MAX);
            self.line_cursor = Some(line);
        } else {
            let start = self
                .diff
                .as_ref()
                .ok()
                .and_then(|text| file_starts(text).get(file).map(|(start, _)| *start));
            let Some(start) = start else {
                return;
            };
            self.line_cursor = Some(start + line);
        }
        self.move_cursor_over(0, false);
    }

    /// The patch the view shows: the whole diff, or the selected file's in
    /// the per-file view — the first file shown when the selection is
    /// filtered out, as the view falls back to it.
    fn shown_text(&self) -> Option<&str> {
        if !self.per_file {
            return self.diff.as_deref().ok();
        }
        self.shown_file().map(|(_, file)| file.patch)
    }

    /// The per-file view's file, by its place in the whole diff.
    fn shown_file(&self) -> Option<(usize, FileDiff<'_>)> {
        let mut files = self.visible_files();
        let shown = files
            .iter()
            .position(|(index, _)| *index == self.selected_file)
            .unwrap_or(0);
        (shown < files.len()).then(|| files.swap_remove(shown))
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
fn keep_top(text: &str, hidden: bool, top: u16, per_file: bool) -> u16 {
    let before = rows(text, hidden, per_file);
    let after = rows(text, !hidden, per_file);
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

/// Each file of `text` with the line its patch starts at among `text`'s.
fn file_starts(text: &str) -> Vec<(usize, FileDiff<'_>)> {
    let mut start = text
        .lines()
        .take_while(|line| !line.starts_with("diff --git "))
        .count();
    crate::files::file_diffs(text)
        .into_iter()
        .map(|file| {
            let at = start;
            start += file.patch.lines().count();
            (at, file)
        })
        .collect()
}

/// For each line of `text`, whether it is an added or removed line: within
/// a hunk, where a file's `---` and `+++` markers cannot be.
fn changed_lines(text: &str) -> Vec<bool> {
    let mut in_hunk = false;
    text.lines()
        .map(|line| {
            if line.starts_with("diff --git ") {
                in_hunk = false;
            } else if line.starts_with("@@") {
                in_hunk = true;
            }
            in_hunk && line.starts_with(['+', '-'])
        })
        .collect()
}

/// For each line of `text`, the block of changes it is in, counted from the
/// top: a run of added and removed lines, which the note that a side has no
/// newline at its end does not break. `None` for any other line.
fn change_blocks(text: &str) -> Vec<Option<usize>> {
    let changed = changed_lines(text);
    let mut blocks = 0;
    let mut previous = false;
    text.lines()
        .zip(changed)
        .map(|(line, changed)| {
            if line.starts_with('\\') && previous {
                return None;
            }
            if changed && !previous {
                blocks += 1;
            }
            previous = changed;
            changed.then_some(blocks)
        })
        .collect()
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
        let mut diff = CheckoutDiff::new(
            Source::Checkout(DiffTarget {
                worktree: PathBuf::from("/w"),
                branch: None,
                base_branch: None,
                base: "4bf5c35d".into(),
                branch_point: true,
            }),
            Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n"
                .into()),
            Vec::new(),
        );
        diff.scroll.note_limit(100);
        diff
    }

    #[test]
    fn layout_changes_and_resizing_preserve_source_locations() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut diff = two_files();
        diff.diff = Ok(format!(
            "diff --git a/a.txt b/a.txt\n@@ -1,60 +1,60 @@\n{}",
            (0..30)
                .map(|i| format!("-old{i}\n+new{i}\n keep{i}\n"))
                .collect::<String>()
        ));
        let draw = |diff: &mut CheckoutDiff, width| {
            let mut terminal = Terminal::new(TestBackend::new(width, 16)).unwrap();
            let mut feedback = CheckoutDiffFeedback::default();
            terminal
                .draw(|frame| {
                    feedback = crate::render(frame, &diff.view(Block::default()), frame.area());
                })
                .unwrap();
            diff.note(feedback);
        };
        let location = |diff: &CheckoutDiff| {
            let text = diff.shown_text().unwrap();
            let plan = crate::split::source_rows(
                text,
                diff.hide_removed,
                diff.rendered_side,
                diff.per_file,
            );
            position(
                text,
                line_at(&plan, usize::from(diff.scroll.clamped())).unwrap(),
            )
            .unwrap()
            .1
        };
        draw(&mut diff, 120);
        diff.scroll.offset = 13;
        let before = location(&diff);
        diff.apply(Action::ToggleLayout);
        draw(&mut diff, 120);
        assert!(diff.rendered_side);
        assert_eq!(location(&diff), before);
        draw(&mut diff, 60);
        assert!(!diff.rendered_side);
        assert!(diff.side_by_side);
        assert_eq!(location(&diff), before);
        draw(&mut diff, 120);
        assert!(diff.rendered_side);
        assert_eq!(location(&diff), before);
        diff.apply(Action::ToggleRemoved);
        draw(&mut diff, 120);
        assert!(!diff.rendered_side);
        assert_eq!(location(&diff), before);
        diff.apply(Action::ToggleRemoved);
        draw(&mut diff, 120);
        assert!(diff.rendered_side);
        assert_eq!(location(&diff), before);
        diff.jump_to(Path::new("/w/a.txt"), Some(41)).unwrap();
        draw(&mut diff, 160);
        assert_eq!(location(&diff), Some(41));
    }

    #[test]
    fn jump_focuses_the_matching_file_and_hunk_line() {
        let mut diff = two_files();
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
            diff --git a/b.txt b/b.txt\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\
            @@ -30 +30 @@\n-last\n+end\n"
            .into());
        diff.scroll.offset = 2;
        diff.jump_to(Path::new("/w/b.txt"), Some(30)).unwrap();
        assert!(diff.per_file);
        assert_eq!(diff.selected_file, 1);
        assert_eq!(diff.combined_scroll.offset, 2);
        let shown = rows(diff.shown_text().unwrap(), false, true);
        let row = line_at(&shown, usize::from(diff.scroll.clamped())).unwrap();
        assert_eq!(diff.shown_text().unwrap().lines().nth(row), Some("+end"));

        diff.hide_removed = true;
        diff.jump_to(Path::new("/w/b.txt"), Some(11)).unwrap();
        let shown = rows(diff.shown_text().unwrap(), true, true);
        let row = line_at(&shown, usize::from(diff.scroll.clamped())).unwrap();
        assert_eq!(diff.shown_text().unwrap().lines().nth(row), Some("+new"));
        diff.jump_to(Path::new("/w/b.txt"), Some(10)).unwrap();
        let offset = diff.scroll.offset;
        for (path, line) in [
            ("/w/b.txt", Some(20)),
            ("/w/unchanged", None),
            ("/else/b.txt", None),
        ] {
            assert!(diff.jump_to(Path::new(path), line).is_err());
            assert_eq!(diff.scroll.offset, offset);
            assert_eq!(diff.selected_file, 1);
        }
        diff.jump_to(Path::new("/w/a.txt"), None).unwrap();
        assert_eq!((diff.selected_file, diff.scroll.offset), (0, 0));
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
        let added = diff
            .diff
            .as_ref()
            .unwrap()
            .lines()
            .position(|line| line == "+two");
        diff.line_cursor = added;

        assert!(!diff.refresh());
        git(&root, &["tag", "baseline"]);
        assert!(!diff.refresh());
        assert!(diff.base_names.iter().any(|name| name == "tag: baseline"));
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        assert!(diff.refresh());

        assert!(diff.diff.as_ref().unwrap().contains("a/a.txt"));
        assert_eq!(diff.selected_path().as_deref(), Some("b.txt"));
        assert_eq!(diff.scroll.offset, 3);
        assert_ne!(diff.line_cursor, added, "a.txt's patch now comes first");
        assert_eq!(
            diff.source(),
            Some(("b.txt", Some(1))),
            "the cursor stays on b.txt"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A supplied diff is only ever changed by the host: refreshing reads
    /// nothing, and a replacement keeps the selected file as a refresh does.
    /// Its files open from its root, and not at all without one.
    #[test]
    fn a_supplied_diff_changes_only_when_replaced() {
        let one = "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n";
        let mut diff = CheckoutDiff::supplied(
            one.into(),
            Supplied {
                label: "turn 1".into(),
                root: Some(PathBuf::from("/w")),
            },
        );
        assert!(!diff.refresh());
        assert_eq!(diff.root(), Some(Path::new("/w")));
        diff.apply(Action::ToggleFiles);
        diff.scroll.offset = 3;

        let two = format!("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n{one}");
        assert!(diff.replace(Ok(two.clone())));
        assert!(!diff.replace(Ok(two)));
        assert_eq!(diff.selected_path().as_deref(), Some("b.txt"));
        assert_eq!(diff.scroll.offset, 3);
        assert!(diff.jump_to(Path::new("/w/a.txt"), None).is_ok());

        let Source::Supplied(supplied) = &mut diff.source else {
            unreachable!("the diff was supplied");
        };
        supplied.root = None;
        assert!(diff.location().is_err());
        assert!(diff.jump_to(Path::new("/w/a.txt"), None).is_err());
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
        let Source::Checkout(target) = &mut diff.source else {
            unreachable!("two_files reads a checkout");
        };
        target.worktree = root.clone();
        // Rows: 0 a.rs's header, 1 its rule, 2 its hunk, 3 " keep" (10),
        // 4 "-old", 5 "+new" (11), 6 " after" (12), 7 the rule above
        // gone.rs's header, 8 that header. Git path markers are hidden.
        let text = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n\
                    @@ -1 +0,0 @@\n-gone\n";
        assert_eq!(
            removed_rows(text).len(),
            text.lines().count(),
            "removal flags retain raw source indices"
        );
        diff.diff = Ok(text.into());
        let a = root.join("a.rs");

        assert_eq!(diff.location(), Ok((a.clone(), None)));
        diff.scroll.offset = 1;
        assert_eq!(
            diff.location(),
            Ok((a.clone(), Some(10))),
            "the rule anchors to the first visible hunk"
        );
        diff.scroll.offset = 3;
        assert_eq!(diff.location(), Ok((a.clone(), Some(10))));
        diff.scroll.offset = 4;
        assert_eq!(diff.location(), Ok((a.clone(), Some(11))), "where -old was");
        diff.hide_removed = true;
        assert_eq!(
            diff.location(),
            Ok((a.clone(), Some(11))),
            "+new, row 4 of those shown"
        );
        diff.hide_removed = false;
        for top in [7, 8] {
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
        assert_eq!(diff.location(), Ok((a, Some(10))));
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

    /// The line cursor starts on the line at the top, moves a drawn line at
    /// a time — over rules, and over removed lines while they are hidden —
    /// and is what `v` and a message start from.
    #[test]
    fn the_line_cursor_moves_over_drawn_lines() {
        let mut diff = two_files();
        // Lines: a.txt's header 0, hunk 1, "-a" 2, "+b" 3, b.txt's header 4,
        // hunk 5, "-c" 6, "+d" 7. The headers' rules are rows of their own:
        // the rows are a.txt's header 0, its rule 1, hunk 2, "-a" 3, "+b" 4,
        // the rule above b.txt's header 5, the header 6, its rule 7, hunk 8,
        // "-c" 9, "+d" 10.
        diff.scroll.note_viewport(4);
        assert_eq!(diff.source(), None, "the combined diff is on no one file");
        diff.scroll.offset = 1;
        diff.apply(Action::ToggleLines);
        assert_eq!(
            diff.line_cursor,
            Some(2),
            "the first line of a hunk from the rule at the top"
        );
        diff.apply(Action::Down);
        assert_eq!(diff.line_cursor, Some(3));
        assert_eq!(diff.source(), Some(("a.txt", Some(1))));
        assert_eq!(diff.scroll.offset, 1, "still on screen");
        diff.apply(Action::Down);
        assert_eq!(diff.line_cursor, Some(6), "over b.txt's header and hunk");
        assert_eq!(diff.scroll.offset, 6, "scrolled to keep the cursor in view");
        assert_eq!(diff.source(), Some(("b.txt", Some(1))));
        diff.apply(Action::Last);
        assert_eq!(diff.line_cursor, Some(7));
        assert_eq!(diff.source(), Some(("b.txt", Some(1))));

        diff.apply(Action::Up);
        assert_eq!(diff.line_cursor, Some(6), "-c");
        diff.apply(Action::ToggleRemoved);
        assert_eq!(diff.line_cursor, Some(7), "-c is hidden; +d is next");
        diff.apply(Action::Up);
        assert_eq!(
            diff.line_cursor,
            Some(3),
            "over -c, the header and hunk to +b"
        );
        diff.apply(Action::First);
        assert_eq!(diff.line_cursor, Some(3), "-a is hidden too");

        diff.apply(Action::ToggleLines);
        assert_eq!(diff.line_cursor, None);
        let offset = diff.scroll.offset;
        diff.apply(Action::Down);
        assert_eq!(diff.scroll.offset, offset + 1, "j scrolls again");
    }

    /// In line selection the cursor keeps to added and removed lines,
    /// across files in the combined diff and within the file in the per-file
    /// view, stepping over removed lines too while they are hidden.
    #[test]
    fn line_selection_keeps_to_changed_lines() {
        let mut diff = two_files();
        // Lines: a.txt's header 0, hunk 1, " x" 2, "-a" 3, "+b" 4, hunk 5,
        // "-c" 6, " y" 7, b.txt's header 8, hunk 9, " z" 10, "+d" 11.
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n x\n-a\n+b\n\
                        @@ -9,2 +9 @@\n-c\n y\n\
                        diff --git a/b.txt b/b.txt\n@@ -1 +1,2 @@\n z\n+d\n"
            .into());
        diff.scroll.note_viewport(20);
        fn lines(diff: &mut CheckoutDiff, action: Action, expected: &[usize]) {
            for &expected in expected {
                diff.apply(action);
                assert_eq!(diff.line_cursor, Some(expected), "{action:?}");
            }
        }
        diff.apply(Action::ToggleLines);
        assert_eq!(diff.line_cursor, Some(3), "the first change from the top");
        lines(&mut diff, Action::Down, &[4, 6, 11, 11]);
        lines(&mut diff, Action::Up, &[6, 4, 3, 3]);

        // J and K go between the first lines of blocks of changes.
        lines(&mut diff, Action::FilesDown, &[6, 11, 11]);
        lines(&mut diff, Action::FilesUp, &[6, 3, 3]);

        diff.apply(Action::ToggleRemoved);
        assert_eq!(diff.line_cursor, Some(4), "off -a, now hidden");
        lines(&mut diff, Action::Down, &[11]);
        lines(&mut diff, Action::Up, &[4, 4]);
        // "-c" is a block of its own, hidden whole; "-a" "+b" starts at +b.
        lines(&mut diff, Action::FilesDown, &[11]);
        lines(&mut diff, Action::FilesUp, &[4]);
        diff.apply(Action::ToggleRemoved);

        diff.apply(Action::First);
        diff.apply(Action::ToggleFiles);
        assert_eq!((diff.selected_file, diff.line_cursor), (0, Some(3)));
        lines(&mut diff, Action::Last, &[6]);
        lines(&mut diff, Action::Down, &[6]);
        assert_eq!(diff.selected_file, 0, "the per-file view keeps to its file");
    }

    /// The cursor's row is marked in either layout, on the line it is on.
    #[test]
    fn the_line_cursor_is_drawn_on_its_line() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut diff = two_files();
        diff.line_cursor = Some(7);
        for side_by_side in [false, true] {
            diff.side_by_side = side_by_side;
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal
                .draw(|frame| {
                    crate::render(frame, &diff.view(Block::default()), frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let marked: Vec<String> = (0..30)
                .filter(|y| buffer[(0, *y)].symbol() == "▌")
                .map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            assert_eq!(marked.len(), 1, "side by side: {side_by_side}");
            assert!(marked[0].contains('d'), "{}", marked[0]);
        }
    }

    /// Switching between the combined diff and the per-file view keeps the
    /// cursor on its line, counted in whichever patch is shown.
    #[test]
    fn the_line_cursor_goes_between_the_combined_and_per_file_views() {
        let mut diff = two_files();
        diff.line_cursor = Some(7);
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.selected_file, 1);
        assert_eq!(diff.line_cursor, Some(3), "+d in b.txt's own patch");
        assert_eq!(diff.source(), Some(("b.txt", Some(1))));
        diff.apply(Action::Up);
        assert_eq!(diff.selected_file, 1, "j/k move the cursor, not the file");
        assert_eq!(diff.line_cursor, Some(2));
        diff.apply(Action::ToggleFiles);
        assert_eq!(diff.line_cursor, Some(6));

        diff.apply(Action::ToggleFiles);
        assert!(diff.leave_lines());
        assert!(!diff.leave_lines());
        assert_eq!(
            diff.source(),
            Some(("b.txt", None)),
            "the file being viewed"
        );
        diff.line_cursor = Some(1);
        diff.apply(Action::Search);
        assert_eq!(diff.line_cursor, None, "the filter may change the file");
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

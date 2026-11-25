//! What a diff view compares, the diff it read, and how it is being looked
//! at: scrolled, split per file, filtered. [`crate::keys`] turns keys into
//! [`Action`]s, so what each one does is decided here once.

use crossterm::event::{KeyCode, KeyEvent};
#[cfg(feature = "renderer")]
use ratatui::widgets::Block;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use styra_ui::files::FileContent;
use styra_ui::query::Edit;
use styra_ui::scroll::Scroll;
use styra_ui::search::Search;

use crate::files::{filtered_file_diffs, position, FileDiff};
use crate::layers::Layers;
use crate::report::{ReportPage, ReportState};
use crate::view::{rows, CheckoutDiffFeedback, CheckoutDiffView, Hidden, Row, Tint};

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
    /// Start typing text to find. Line selection only.
    Find,
    /// The next line with the found text on it, from the line cursor's,
    /// wrapping past the last. Line selection only.
    FindNext,
    /// The previous line with the found text on it. Line selection only.
    FindPrevious,
    /// Switch between the combined diff and one file's patch at a time.
    ToggleFiles,
    /// Move the per-file view's file list between the side and bottom.
    ToggleFilesBelow,
    /// Hide or show the per-file view's file list.
    ToggleFileList,
    ToggleRemoved,
    /// Show only the changed lines, under their files' headers and hunk
    /// cuts, or their context again.
    ToggleUltraCompact,
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
    /// Restrict the file list to the currently highlighted commits.
    ToggleInvolved,
    /// Put the per-file view's edit report away, or bring it back.
    ToggleReport,
    /// Fold the file tree's directories that hold no files of their own and
    /// a single subdirectory onto one row, or give each its own again.
    ToggleCompactDirs,
    /// Put a cursor on the line at the top of the patch shown, or take it
    /// away. While it is there, the moving keys move it rather than the view.
    ToggleLines,
    /// Show the whole file the line cursor is in beside the diff, or put it
    /// away. Line selection only.
    TogglePreview,
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

/// The commit browser. Row zero restores the complete comparison.
#[derive(Default)]
pub struct Commits {
    pub entries: Vec<crate::git::Commit>,
    pub open: bool,
    pub focused: bool,
    pub selected: usize,
}

/// A diff read for the view, and how far it is scrolled.
pub struct CheckoutDiff {
    pub source: Source,
    pub commits: Commits,
    /// `git diff`'s output, or the supplied diff, or why there is none.
    pub diff: Result<String, String>,
    /// The branches and tags at the base commit, read with the diff. Always
    /// empty for a supplied diff.
    pub base_names: Vec<String>,
    pub scroll: Scroll,
    /// The combined diff's scroll, put by while the per-file view is shown so
    /// going back to it returns to the same place.
    pub combined_scroll: Scroll,
    /// Scroll position of the shortcut legend while it is open.
    pub help: Option<u16>,
    pub per_file: bool,
    /// Requested layout; temporarily unified when the patch area is narrow.
    pub side_by_side: bool,
    /// Layout used by the last frame, which defines the scroll coordinates.
    pub rendered_side: bool,
    /// Layout of the combined scroll saved while a single file is shown.
    pub combined_side: bool,
    pub horizontal_scroll: u16,
    pub hide_removed: bool,
    /// Whether unchanged context is left out, showing only the changed lines.
    pub ultra_compact: bool,
    pub compact_dirs: bool,
    pub tint: Tint,
    pub word_highlights: bool,
    /// Whether the per-file view's thumbnail of the selected file's changes
    /// has been put away, where the host shows one.
    pub thumbnail_hidden: bool,
    /// The commits the diff was made in, or why they could not be read,
    /// while its changes are tinted by them; `None` otherwise.
    pub layers: Option<Result<Layers, String>>,
    /// Commits pinned when the involved-files filter was enabled.
    pub involved_layers: Option<Vec<u16>>,
    pub selected_file: usize,
    /// The line of its patch each file the per-file view has left was
    /// scrolled to, by path, so going back to a file returns to it. A line
    /// rather than a row, so it holds when removed lines or the layout
    /// change in the meantime.
    pub file_tops: HashMap<String, usize>,
    pub file_list_offset: usize,
    /// Whether the per-file view lists files below the patch.
    pub files_below: bool,
    /// Whether the per-file view's file list is hidden.
    pub file_list_hidden: bool,
    pub search: Search,
    /// What each file was edited for, once the host has had it described.
    pub report: ReportState,
    /// The page of every file's description, while it is open.
    pub report_page: Option<ReportPage>,
    /// Whether the report page shows the selected file's patch below it,
    /// kept for the next time it opens.
    pub report_preview: bool,
    /// The line the line cursor is on, by its place among the shown patch's
    /// lines; `None` unless lines are being selected.
    pub line_cursor: Option<usize>,
    /// Whether the whole file the line cursor is in is shown beside the
    /// diff, its line marked, while lines are selected; kept when they are
    /// not, for the next time they are.
    pub file_preview: bool,
    /// The file that preview shows, read when the cursor comes into it.
    pub preview_file: Option<PreviewFile>,
    /// Text to find in the shown patch, marked while lines are selected.
    pub find: Search,
    /// Where the line cursor was, and the view scrolled, when the text to
    /// find began to be typed: what it moves on from as the text changes,
    /// and goes back to if the text is cancelled.
    pub find_from: Option<(usize, u16)>,
}

/// An empty supplied diff, shown as a diff view opens: combined, unified,
/// with removed lines, words highlighted, and single-subdirectory
/// directories folded.
impl Default for CheckoutDiff {
    fn default() -> Self {
        Self {
            source: Source::Supplied(Supplied {
                label: String::new(),
                root: None,
            }),
            commits: Commits::default(),
            diff: Ok(String::new()),
            base_names: Vec::new(),
            scroll: Scroll::default(),
            combined_scroll: Scroll::default(),
            help: None,
            per_file: false,
            side_by_side: false,
            rendered_side: false,
            combined_side: false,
            horizontal_scroll: 0,
            hide_removed: false,
            ultra_compact: false,
            compact_dirs: true,
            tint: Tint::default(),
            word_highlights: true,
            thumbnail_hidden: false,
            layers: None,
            involved_layers: None,
            selected_file: 0,
            file_tops: HashMap::new(),
            file_list_offset: 0,
            files_below: false,
            file_list_hidden: false,
            search: Search::default(),
            report: ReportState::default(),
            report_page: None,
            report_preview: false,
            line_cursor: None,
            file_preview: false,
            preview_file: None,
            find: Search::default(),
            find_from: None,
        }
    }
}

/// A file as the preview shows it whole: its path in the diff, the commit
/// it was read at — `None` for the working tree — and its text, or why there
/// is none.
pub struct PreviewFile {
    pub path: String,
    pub commit: Option<String>,
    pub content: Result<String, String>,
}

impl CheckoutDiff {
    /// Whether a prompt — for text to find, or for the file-name filter —
    /// has the keys, so a letter is typed into it rather than a command to
    /// the view or the host.
    pub fn typing(&self) -> bool {
        self.find.typing() || self.search.typing()
    }

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
        let offset = crate::split::source_rows(file.patch, self.hidden(), self.rendered_side, true)
            .iter()
            .position(|shown| matches!(shown, Row::Line(index) if *index == row))
            .unwrap_or(0);
        if self.per_file {
            self.keep_file_top();
        } else {
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
        self.sync_file_preview(false);
        Ok(())
    }

    /// Read the diff now. It is read when the view opens rather than on every
    /// frame: opening the view again refreshes it, and so can the host every
    /// few seconds while the checkout is being worked on (see
    /// [`Self::refresh`]).
    pub fn read(target: DiffTarget) -> Self {
        let diff = crate::git::diff(&target.worktree, &target.base);
        let base_names = crate::git::names_at(&target.worktree, &target.base);
        let entries = crate::git::commits_between(&target.worktree, &target.base);
        let mut view = Self::new(Source::Checkout(target), diff, base_names);
        view.commits.entries = entries;
        view
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
            ..Self::default()
        }
    }

    pub(crate) fn select_commit(&mut self, selected: usize) {
        if selected == self.commits.selected {
            return;
        }
        let Some(target) = self.target() else {
            return;
        };
        let patch = if selected == 0 {
            crate::git::diff(&target.worktree, &target.base)
        } else if let Some(commit) = self.commits.entries.get(selected - 1) {
            crate::git::commit_diff(&target.worktree, &commit.id)
        } else {
            return;
        };
        self.commits.selected = selected;
        self.involved_layers = None;
        self.search.cancel();
        self.find.cancel();
        self.line_cursor = None;
        self.replace(patch);
        self.selected_file = 0;
        self.file_tops.clear();
        self.file_list_offset = 0;
        self.scroll.reset();
        self.combined_scroll.reset();
        self.horizontal_scroll = 0;
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
        let selected = self
            .commits
            .selected
            .checked_sub(1)
            .and_then(|i| self.commits.entries.get(i))
            .map(|c| c.id.clone());
        let entries = crate::git::commits_between(&target.worktree, &target.base);
        let changed = entries != self.commits.entries;
        self.commits.selected = selected
            .and_then(|id| entries.iter().position(|c| c.id == id))
            .map_or(0, |i| i + 1);
        self.commits.entries = entries;
        let diff = match self.commits.selected.checked_sub(1) {
            Some(i) => crate::git::commit_diff(&target.worktree, &self.commits.entries[i].id),
            None => crate::git::diff(&target.worktree, &target.base),
        };
        self.base_names = crate::git::names_at(&target.worktree, &target.base);
        let mut layered = false;
        if self.layers.is_some() {
            if let Some(layers) =
                Layers::reread(self.layers.as_ref(), &target.worktree, &target.base)
            {
                self.layers = Some(layers);
                self.involved_layers = None;
                layered = true;
            }
        }
        self.replace(diff) || layered || changed
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
                    self.restore_file_top();
                }
            }
            None => self.selected_file = 0,
        }
        if self.line_cursor.is_some() {
            self.line_cursor = cursor.and_then(|(path, number)| self.line_at_number(&path, number));
        }
        // The work that changed the diff may have changed the file too.
        self.sync_file_preview(true);
        true
    }

    /// Read the file the preview shows, if it is open and the line cursor
    /// has gone into a file other than the one read — or into any file, when
    /// `again` — and drop it once there is nothing to show. A checkout's
    /// commit is shown as that commit left the file, which is what its
    /// patch's lines are numbered by; anything else as the working tree has
    /// it.
    pub(crate) fn sync_file_preview(&mut self, again: bool) {
        let shown = self
            .line_cursor
            .filter(|_| self.file_preview)
            .and_then(|line| position(self.shown_text()?, line))
            .map(|(path, _)| path.to_owned());
        let Some(path) = shown else {
            self.preview_file = None;
            return;
        };
        let commit = self
            .commits
            .selected
            .checked_sub(1)
            .and_then(|index| self.commits.entries.get(index))
            .map(|commit| commit.id.clone());
        let read = self
            .preview_file
            .as_ref()
            .is_some_and(|file| file.path == path && file.commit == commit);
        if read && !again {
            return;
        }
        let content = match (self.root(), &commit) {
            (None, _) => Err("this diff's files are not on this host".into()),
            (Some(root), Some(commit)) => crate::git::file_at(root, commit, &path),
            (Some(root), None) => match std::fs::read_to_string(root.join(&path)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err(format!("{path} is deleted in the working tree"))
                }
                read => read.map_err(|error| error.to_string()),
            },
        };
        self.preview_file = Some(PreviewFile {
            path,
            commit,
            content,
        });
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
    #[cfg(feature = "renderer")]
    pub fn view<'a>(&'a self, block: Block<'a>) -> CheckoutDiffView<'a> {
        let mut view = self.view_data();
        view.block = block;
        view
    }

    /// Map state to presentation data without constructing terminal widgets.
    pub fn view_model(&self, chrome: styra_ui::chrome::PanelChrome) -> CheckoutDiffView<'_> {
        let mut view = self.view_data();
        view.chrome = Some(chrome);
        view
    }

    fn view_data(&self) -> CheckoutDiffView<'_> {
        let target = self.target();
        let supplied = match &self.source {
            Source::Supplied(supplied) => Some(supplied),
            Source::Checkout(_) => None,
        };
        CheckoutDiffView {
            commits: Some(&self.commits),
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
            help: self.help,
            per_file: self.per_file,
            side_by_side: self.side_by_side,
            rendered_side: self.rendered_side,
            horizontal_scroll: self.horizontal_scroll,
            thumbnail_hidden: self.thumbnail_hidden,
            hide_removed: self.hide_removed,
            ultra_compact: self.ultra_compact,
            compact_dirs: self.compact_dirs,
            tint: self.tint,
            word_highlights: self.word_highlights,
            layers: self
                .layers
                .as_ref()
                .filter(|_| self.commits.selected == 0)
                .map(|layers| match layers {
                    Ok(layers) => Ok(layers),
                    Err(error) => Err(error.as_str()),
                }),
            involved_layers: self.involved_layers.as_deref(),
            selected_file: self.selected_file,
            file_list_offset: self.file_list_offset,
            files_below: self.files_below,
            file_list_hidden: self.file_list_hidden,
            search: self.search.view(),
            report: (self.commits.selected == 0).then_some(&self.report),
            report_page: self.report_page,
            line_cursor: self.line_cursor,
            file_preview: self.shown_file_preview(),
            find: match self.line_cursor {
                Some(_) => self.find.view(),
                None => Default::default(),
            },
            ..CheckoutDiffView::default()
        }
    }

    /// The file the line cursor is in, whole, with the new file's line there
    /// to mark, while the preview is open over lines being selected.
    fn shown_file_preview(&self) -> Option<crate::view::FilePreview<'_>> {
        let cursor = self.line_cursor.filter(|_| self.file_preview)?;
        let (path, line) = position(self.shown_text()?, cursor)?;
        let file = self
            .preview_file
            .as_ref()
            .filter(|file| file.path == path)?;
        Some(crate::view::FilePreview {
            path: &file.path,
            content: match &file.content {
                Ok(content) if content.is_empty() => FileContent::Empty,
                Ok(content) => FileContent::Ready(content),
                Err(error) => FileContent::Failed(error),
            },
            line,
        })
    }

    /// Keep what drawing the view measured, for the next keys to move by.
    pub fn note(&mut self, measured: CheckoutDiffFeedback) {
        self.rendered_side = measured.side_by_side;
        self.horizontal_scroll = measured.horizontal_scroll;
        self.scroll.offset = measured.scroll.effective_scroll;
        self.scroll.note_limit(measured.scroll.limit);
        self.scroll.note_viewport(measured.scroll.viewport);
        self.file_list_offset = measured.file_list_offset;
        if let (Some(page), Some(measured)) = (self.report_page.as_mut(), measured.report_page) {
            *page = measured;
        }
    }

    /// Show file `index` of the whole diff in the per-file view, where it
    /// was left, clearing a file filter that would hide it.
    pub(crate) fn open_file(&mut self, index: usize) {
        self.leave_lines();
        self.show_files();
        self.keep_file_top();
        self.search.cancel();
        self.selected_file = index;
        self.file_list_offset = 0;
        self.restore_file_top();
    }

    pub(crate) fn apply(&mut self, action: Action) {
        let lines = self.line_cursor.is_some();
        match action {
            Action::Find if lines => {
                self.find.open();
                self.find_from = self.line_cursor.map(|line| (line, self.scroll.offset));
            }
            Action::FindNext if lines => self.find_next(true),
            Action::FindPrevious if lines => self.find_next(false),
            Action::Search if self.per_file => {
                self.line_cursor = None;
                self.search.open();
                self.update_search();
            }
            // Each file's patch and the combined diff open where they were
            // left. A line cursor goes along to the same line.
            Action::ToggleFiles => {
                let cursor = self.cursor_in_diff();
                self.keep_file_top();
                self.per_file = !self.per_file;
                std::mem::swap(&mut self.rendered_side, &mut self.combined_side);
                std::mem::swap(&mut self.scroll, &mut self.combined_scroll);
                if self.per_file {
                    self.restore_file_top();
                }
                self.line_cursor = None;
                if let Some((file, line)) = cursor {
                    self.place_cursor(file, line);
                }
            }
            Action::ToggleLines if lines => {
                self.line_cursor = None;
                self.find.cancel();
            }
            Action::ToggleLines => {
                self.line_cursor = self.shown_text().and_then(|text| {
                    let rows = crate::split::source_rows(
                        text,
                        self.hidden(),
                        self.rendered_side,
                        self.per_file,
                    );
                    line_at(&rows, usize::from(self.scroll.clamped()))
                });
                // Onto the first change from the top of the view.
                self.move_cursor(0);
            }
            Action::TogglePreview if lines => self.file_preview = !self.file_preview,
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
            Action::ToggleRemoved => self.set_hidden(Hidden {
                removed: !self.hide_removed,
                ..self.hidden()
            }),
            Action::ToggleUltraCompact => self.set_hidden(Hidden {
                context: !self.ultra_compact,
                ..self.hidden()
            }),
            Action::ToggleLayout => self.side_by_side = !self.side_by_side,
            Action::ToggleFilesBelow => self.files_below = !self.files_below,
            Action::ToggleFileList if self.per_file => {
                self.file_list_hidden = !self.file_list_hidden
            }
            Action::ToggleFileList => {}
            Action::PanLeft => self.horizontal_scroll = self.horizontal_scroll.saturating_sub(8),
            Action::PanRight => self.horizontal_scroll = self.horizontal_scroll.saturating_add(8),
            // The selected file stays selected, and the list keeps it in view.
            Action::ToggleWords => self.word_highlights = !self.word_highlights,
            Action::ToggleThumbnail => self.thumbnail_hidden = !self.thumbnail_hidden,
            Action::ToggleLayers => {
                self.involved_layers = None;
                self.layers = match (&self.layers, self.target()) {
                    (Some(_), _) => None,
                    (None, Some(target)) => Some(Layers::read(&target.worktree, &target.base)),
                    (None, None) => Some(Err("a supplied diff has no commits to layer".into())),
                }
            }
            Action::ToggleInvolved => {
                if self.involved_layers.take().is_none()
                    && self.per_file
                    && self.commits.selected == 0
                {
                    if let (Some(Ok(layers)), Some(text)) = (&self.layers, self.shown_text()) {
                        let touched = match self.line_cursor {
                            Some(cursor) => crate::view::block_layers(layers, text, cursor),
                            None => layers.file_layers(text),
                        };
                        if !touched.is_empty() {
                            self.involved_layers = Some(touched);
                        }
                    }
                }
                self.update_search();
            }
            Action::CycleTint => self.tint = self.tint.next(),
            Action::ToggleCompactDirs => self.compact_dirs = !self.compact_dirs,
            Action::ToggleReport => self.report.hidden = !self.report.hidden,
            Action::Down if self.per_file => self.select_file(true, 1),
            Action::Up if self.per_file => self.select_file(false, 1),
            Action::FilesDown if self.per_file => self.select_file(true, 5),
            Action::FilesUp if self.per_file => self.select_file(false, 5),
            Action::First if self.per_file => self.select_file(false, usize::MAX),
            Action::Last if self.per_file => self.select_file(true, usize::MAX),
            Action::Down => self.scroll.line_down(),
            Action::Up => self.scroll.line_up(),
            Action::ScrollDown => self.scroll.page_down(),
            Action::ScrollUp => self.scroll.page_up(),
            Action::HalfPageDown => self.scroll.half_page_down(),
            Action::HalfPageUp => self.scroll.half_page_up(),
            Action::First => self.scroll.reset(),
            Action::Last => self.scroll.scroll_to_end(),
            Action::Search
            | Action::Find
            | Action::FindNext
            | Action::FindPrevious
            | Action::TogglePreview
            | Action::FilesDown
            | Action::FilesUp => {}
        }
    }

    /// The file-name filter's prompt takes every key while it is open, the
    /// letters bound to commands included.
    pub(crate) fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.search.cancel(),
            KeyCode::Enter => self.search.accept(),
            _ => {
                if let Some(edit) = Edit::from_key(key) {
                    self.search.edit(edit);
                }
            }
        }
        self.update_search();
    }

    /// The prompt for text to find takes every key while it is open. The
    /// line cursor goes to the next line the text is on, from where it was
    /// when the prompt opened, as the text is typed, and back there if the
    /// text is cancelled or backspaced away.
    pub(crate) fn find_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.find.cancel(),
            KeyCode::Enter => {
                self.find.accept();
                self.find_from = None;
                return;
            }
            _ => match Edit::from_key(key) {
                Some(edit) => self.find.edit(edit),
                None => return,
            },
        }
        let Some((line, offset)) = self.find_from else {
            return;
        };
        self.line_cursor = Some(line);
        self.scroll.offset = offset;
        if self.find.typing() {
            self.find_next(true);
        } else {
            self.find_from = None;
        }
    }

    /// Move the line cursor to the next line drawn with the text to find on
    /// it, or when `down` is false the previous one, going round past the
    /// end, and scroll it into view. With no such line the cursor stays.
    fn find_next(&mut self, down: bool) {
        let (Some(cursor), Some(text), Some(term)) =
            (self.line_cursor, self.shown_text(), self.find.query())
        else {
            return;
        };
        let (hidden, side, per_file) = (self.hidden(), self.rendered_side, self.per_file);
        let found = crate::find::match_rows(text, term, hidden, side, per_file);
        let at = crate::split::row_of(text, hidden, side, per_file, cursor);
        let target = if down {
            found.iter().find(|&&(row, _)| row > at).or(found.first())
        } else {
            found
                .iter()
                .rev()
                .find(|&&(row, _)| row < at)
                .or(found.last())
        };
        if let Some(&(row, line)) = target {
            self.line_cursor = Some(line);
            self.scroll.reveal(u16::try_from(row).unwrap_or(u16::MAX));
        }
    }

    /// Drop standing text to find, saying whether there was any: Esc clears
    /// it before it leaves line selection.
    pub(crate) fn clear_find(&mut self) -> bool {
        if self.find.query().is_none() {
            return false;
        }
        self.find.cancel();
        true
    }

    /// Drop a standing file-name filter, saying whether there was one: Esc
    /// clears it before it means anything else to the host.
    pub(crate) fn clear_filter(&mut self) -> bool {
        if self.per_file && self.involved_layers.take().is_some() {
            self.update_search();
            return true;
        }
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
                    self.hidden(),
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
        self.find.cancel();
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
        let (hidden, side, per_file) = (self.hidden(), self.rendered_side, self.per_file);
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
        let (hidden, side, per_file) = (self.hidden(), self.rendered_side, self.per_file);
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
        let mut files = self
            .diff
            .as_ref()
            .map(|text| filtered_file_diffs(text, self.search.query()))
            .unwrap_or_default();
        if let (Some(touched), Some(Ok(layers))) = (&self.involved_layers, &self.layers) {
            files.retain(|(_, file)| {
                layers
                    .file_layers(file.patch)
                    .iter()
                    .any(|i| touched.contains(i))
            });
        }
        files
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
        drop(files);
        if let Some(next) = next {
            self.selected_file = next;
        }
        self.restore_file_top();
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
        drop(files);
        if let Some(next) = next.filter(|next| *next != self.selected_file) {
            self.keep_file_top();
            self.selected_file = next;
            self.restore_file_top();
        }
    }

    /// Put by the line the per-file view's file is scrolled to, for
    /// [`Self::restore_file_top`] to return to. Nothing in the combined diff.
    fn keep_file_top(&mut self) {
        if !self.per_file {
            return;
        }
        let Some((_, file)) = self.shown_file() else {
            return;
        };
        let path = file.path.to_owned();
        let top = self.scroll.clamped();
        let line = (top > 0)
            .then(|| {
                let rows =
                    crate::split::source_rows(file.patch, self.hidden(), self.rendered_side, true);
                line_at(&rows, usize::from(top))
            })
            .flatten();
        match line {
            Some(line) => self.file_tops.insert(path, line),
            None => self.file_tops.remove(&path),
        };
    }

    /// Scroll the per-file view's file to where it was left, or its top.
    fn restore_file_top(&mut self) {
        let offset = self.shown_file().and_then(|(_, file)| {
            let line = *self.file_tops.get(file.path)?;
            crate::split::source_rows(file.patch, self.hidden(), self.rendered_side, true)
                .iter()
                .position(|row| matches!(row, Row::Line(index) if *index >= line))
        });
        self.scroll.offset = offset.map_or(0, |offset| u16::try_from(offset).unwrap_or(u16::MAX));
        // The limit measured belongs to the patch shown before.
        self.scroll.note_limit(u16::MAX);
    }

    /// The lines left out of the patch shown.
    pub(crate) fn hidden(&self) -> Hidden {
        Hidden {
            removed: self.hide_removed,
            context: self.ultra_compact,
        }
    }

    /// Leave out the lines `next` says instead, keeping what is at the top
    /// of the view there, here and in the combined diff put by, and the line
    /// cursor on a line still drawn.
    fn set_hidden(&mut self, next: Hidden) {
        let hidden = self.hidden();
        // Removed lines are only ever hidden from the unified layout.
        let side = |side: bool| side && !next.removed;
        if let Some(text) = self.shown_text() {
            self.scroll.offset = if self.rendered_side {
                crate::split::remap(
                    text,
                    hidden,
                    true,
                    next,
                    side(true),
                    self.scroll.clamped(),
                    self.per_file,
                )
            } else {
                keep_top(text, hidden, next, self.scroll.clamped(), self.per_file)
            };
        }
        self.rendered_side = side(self.rendered_side);
        if self.per_file {
            if let Ok(text) = &self.diff {
                self.combined_scroll.offset = if self.combined_side {
                    crate::split::remap(
                        text,
                        hidden,
                        true,
                        next,
                        side(true),
                        self.combined_scroll.clamped(),
                        false,
                    )
                } else {
                    keep_top(text, hidden, next, self.combined_scroll.clamped(), false)
                };
            }
            self.combined_side = side(self.combined_side);
            self.combined_scroll.note_limit(u16::MAX);
        }
        self.hide_removed = next.removed;
        self.ultra_compact = next.context;
        self.scroll.note_limit(u16::MAX);
        // Off a line that is now hidden.
        self.move_cursor_over(0, false);
    }
}

/// The offset that keeps what is at the top of `text`'s rows there once the
/// lines `before` hid are hidden as `after` says instead: the line at the top,
/// or the first after it that stays, with a rule above it kept too.
fn keep_top(text: &str, before: Hidden, after: Hidden, top: u16, per_file: bool) -> u16 {
    let before = rows(text, before, per_file);
    let after = rows(text, after, per_file);
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
mod tests;

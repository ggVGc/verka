//! The view's keys, in one table both hosts read: Styra while the view is
//! open over a session, and the `styra-diff` program on its own. A key either
//! acts on the view here or comes back as an [`Outcome`] only the host can
//! carry out; a key in neither is nothing to the view.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::state::{Action, CheckoutDiff};

/// What a key asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    /// Done to the view itself.
    View(Action),
    /// Clear the text to find, or leave line selection, or clear a standing
    /// file filter, or else close the view.
    ClearFilterOrClose,
    /// Styra's `d`, which opened the view, closing it again.
    Toggle,
    ToggleHelp,
    /// Open the page of every file's description.
    ReportPage,
    ToggleCommits,
    OpenFile,
    GenerateReport,
}

/// One row of the table: the keys, how a reference shows them and what it
/// says they do, and the command they give.
#[derive(Debug, PartialEq, Eq)]
pub struct Binding {
    codes: &'static [KeyCode],
    pub label: &'static str,
    pub description: &'static str,
    command: Command,
}

const fn bind(
    codes: &'static [KeyCode],
    label: &'static str,
    description: &'static str,
    command: Command,
) -> Binding {
    Binding {
        codes,
        label,
        description,
        command,
    }
}

/// Every key the view answers, in the order a reference lists them.
pub const BINDINGS: &[Binding] = &[
    bind(&[KeyCode::Char('h')], "h", "open / close commits (j/k: preview, Enter: focus patch, Tab: focus list, Esc: all changes)", Command::ToggleCommits),

    bind(
        &[KeyCode::Char('?')],
        "?",
        "show / hide the shortcut legend (↑/↓ or PgUp/PgDn to scroll)",
        Command::ToggleHelp,
    ),
    bind(
        &[KeyCode::Char('s')],
        "s",
        "toggle unified / side-by-side",
        Command::View(Action::ToggleLayout),
    ),
    bind(
        &[KeyCode::Left],
        "←",
        "pan both sides left (side-by-side)",
        Command::View(Action::PanLeft),
    ),
    bind(
        &[KeyCode::Right],
        "→",
        "pan both sides right (side-by-side)",
        Command::View(Action::PanRight),
    ),
    bind(
        &[KeyCode::Char('/')],
        "/ (per-file view)",
        "filter diff files by name (Enter keeps filter, Esc clears)",
        Command::View(Action::Search),
    ),
    bind(
        &[KeyCode::Char('f')],
        "f (line selection)",
        "find text in the diff, marked wherever it is (Enter keeps it, Esc clears)",
        Command::View(Action::Find),
    ),
    bind(
        &[KeyCode::Char('n')],
        "n (line selection)",
        "next line with the found text",
        Command::View(Action::FindNext),
    ),
    bind(
        &[KeyCode::Char('N')],
        "N (line selection)",
        "previous line with the found text",
        Command::View(Action::FindPrevious),
    ),
    bind(
        &[KeyCode::Char('p')],
        "p (line selection)",
        "show / hide the whole file beside the diff, the line cursor's line marked",
        Command::View(Action::TogglePreview),
    ),
    bind(
        &[KeyCode::Char('l')],
        "l",
        "toggle combined / per-file diffs",
        Command::View(Action::ToggleFiles),
    ),
    bind(
        &[KeyCode::Char('b')],
        "b (per-file view)",
        "move the file list between the side and bottom",
        Command::View(Action::ToggleFilesBelow),
    ),
    bind(
        &[KeyCode::Char('B')],
        "B (per-file view)",
        "hide / show the file list",
        Command::View(Action::ToggleFileList),
    ),
    bind(
        &[KeyCode::Char('w')],
        "w",
        "toggle word highlights",
        Command::View(Action::ToggleWords),
    ),
    bind(
        &[KeyCode::Char('L')],
        "L",
        "tint changes by the commit that made them, blending where they overlap",
        Command::View(Action::ToggleLayers),
    ),
    bind(
        &[KeyCode::Char('c')],
        "c (per-file view)",
        "toggle only files involved in the highlighted commits (Esc clears)",
        Command::View(Action::ToggleInvolved),
    ),
    bind(
        &[KeyCode::Char('r')],
        "r",
        "toggle visibility of removed diff lines",
        Command::View(Action::ToggleRemoved),
    ),
    bind(
        &[KeyCode::Char('x')],
        "x",
        "toggle ultra-compact: only the changed lines, without their context",
        Command::View(Action::ToggleUltraCompact),
    ),
    bind(
        &[KeyCode::Char('C')],
        "C (per-file view)",
        "fold / unfold directories holding no files of their own",
        Command::View(Action::ToggleCompactDirs),
    ),
    bind(
        &[KeyCode::Enter, KeyCode::Char('V')],
        "Enter / V",
        "select lines: j/k move a line cursor, which v and a message start from",
        Command::View(Action::ToggleLines),
    ),
    bind(
        &[KeyCode::Char('m')],
        "m (per-file view)",
        "show / hide the thumbnail of the file's changes",
        Command::View(Action::ToggleThumbnail),
    ),
    bind(
        &[KeyCode::Char('v')],
        "v",
        "open the file at the top of the diff, or at the line cursor, in the editor",
        Command::OpenFile,
    ),
    bind(
        &[KeyCode::Char('E')],
        "E",
        "describe each file's edits and why, in an edit report",
        Command::GenerateReport,
    ),
    bind(
        &[KeyCode::Char('e')],
        "e (per-file view)",
        "show / hide the edit report's description of the file",
        Command::View(Action::ToggleReport),
    ),
    bind(
        &[KeyCode::Char('o')],
        "o",
        "show / hide every file's description on one page (j/k pick a file, Enter opens it, e describes it again, p shows its diff below, J/K scroll that)",
        Command::ReportPage,
    ),
    bind(
        &[KeyCode::Char('t')],
        "t",
        "switch the unchanged rows' tint between full and half",
        Command::View(Action::CycleTint),
    ),
    bind(
        &[KeyCode::Char('j')],
        "j",
        "next file (per-file view), or scroll down",
        Command::View(Action::Down),
    ),
    bind(
        &[KeyCode::Char('k')],
        "k",
        "previous file (per-file view), or scroll up",
        Command::View(Action::Up),
    ),
    bind(
        &[KeyCode::Char('J')],
        "J",
        "next block of changes (line selection), or move down 5 files (per-file view)",
        Command::View(Action::FilesDown),
    ),
    bind(
        &[KeyCode::Char('K')],
        "K",
        "previous block of changes (line selection), or move up 5 files (per-file view)",
        Command::View(Action::FilesUp),
    ),
    bind(
        &[KeyCode::Down],
        "↓",
        "scroll diff down 10 lines, or the next line, headers too (line selection)",
        Command::View(Action::ScrollDown),
    ),
    bind(
        &[KeyCode::Up],
        "↑",
        "scroll diff up 10 lines, or the previous line, headers too (line selection)",
        Command::View(Action::ScrollUp),
    ),
    bind(
        &[KeyCode::PageDown],
        "PgDn",
        "scroll diff down half a screen",
        Command::View(Action::HalfPageDown),
    ),
    bind(
        &[KeyCode::PageUp],
        "PgUp",
        "scroll diff up half a screen",
        Command::View(Action::HalfPageUp),
    ),
    bind(
        &[KeyCode::Char('g')],
        "g",
        "go to the top, or the first file (per-file view)",
        Command::View(Action::First),
    ),
    bind(
        &[KeyCode::Char('G')],
        "G",
        "go to the bottom, or the last file (per-file view)",
        Command::View(Action::Last),
    ),
    bind(
        &[KeyCode::Char('d')],
        "d",
        "close the diff",
        Command::Toggle,
    ),
    bind(
        &[KeyCode::Esc],
        "Esc",
        "clear the found text, leave line selection, clear the file filter, or close the diff",
        Command::ClearFilterOrClose,
    ),
];

/// What a key leaves the host to do once the view has had it.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The view took the key; there is only drawing it again.
    Handled,
    /// Not one of the view's keys.
    Ignored,
    /// Leave the view.
    Close,
    /// Leave the view if `d` opened it, as in Styra; nothing where the view is
    /// all there is, as in `styra-diff`.
    Toggle,
    /// Open the file at the top of the view in the editor, at
    /// [`CheckoutDiff::location`].
    OpenFile,
    /// Have each file's edits described, and put the report in
    /// [`CheckoutDiff::report`] when it comes.
    GenerateReport,
    /// Have the edits of the file at this path described again, keeping the
    /// report's other descriptions.
    DescribeFile(String),
}

impl CheckoutDiff {
    fn clear_commit(&mut self) -> bool {
        if self.commits.selected == 0 {
            return false;
        }
        self.select_commit(0);
        true
    }

    /// Answer one key. The prompts for text to find and for the file-name
    /// filter take every key while they are open; otherwise the key is looked
    /// up in [`BINDINGS`]. The file preview then follows the line cursor.
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let outcome = self.answer_key(key);
        self.sync_file_preview(false);
        outcome
    }

    fn answer_key(&mut self, key: KeyEvent) -> Outcome {
        if self.find.typing() {
            self.find_key(key);
            return Outcome::Handled;
        }
        if self.search.typing() {
            self.search_key(key);
            return Outcome::Handled;
        }
        // Shift is not checked: a capital already arrives as its own
        // character.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Outcome::Ignored;
        }
        if let Some(scroll) = self.help.as_mut() {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => self.help = None,
                code => scroll_page(scroll, code),
            }
            return Outcome::Handled;
        }
        // The page scrolls like the legend, and j/k pick a file on it to open,
        // whose patch p shows below and J/K scroll. A report, or the picked
        // file's description alone, can be asked for from it, and comes in
        // where it is read.
        if let Some(mut page) = self.report_page {
            let paths: Vec<String> = self.diff.as_deref().map_or_else(
                |_| Vec::new(),
                |text| {
                    crate::files::file_diffs(text)
                        .iter()
                        .map(|file| file.path.to_owned())
                        .collect()
                },
            );
            let last = paths.len().saturating_sub(1);
            let selected = match key.code {
                KeyCode::Char('o') | KeyCode::Esc => {
                    self.report_page = None;
                    return Outcome::Handled;
                }
                KeyCode::Char('E') => return Outcome::GenerateReport,
                KeyCode::Char('e') => {
                    return paths
                        .get(page.selected)
                        .map_or(Outcome::Handled, |path| Outcome::DescribeFile(path.clone()))
                }
                KeyCode::Enter => {
                    self.report_page = None;
                    self.open_file(page.selected.min(last));
                    return Outcome::Handled;
                }
                KeyCode::Char('j') => Some(page.selected.saturating_add(1).min(last)),
                KeyCode::Char('k') => Some(page.selected.saturating_sub(1)),
                KeyCode::Char('g') => Some(0),
                KeyCode::Char('G') => Some(last),
                KeyCode::Char('p') => {
                    page.preview = match page.preview {
                        Some(_) => None,
                        None => Some(0),
                    };
                    self.report_preview = page.preview.is_some();
                    None
                }
                KeyCode::Char('J') => {
                    page.preview = page
                        .preview
                        .map(|scroll| scroll.saturating_add(PREVIEW_STEP));
                    None
                }
                KeyCode::Char('K') => {
                    page.preview = page
                        .preview
                        .map(|scroll| scroll.saturating_sub(PREVIEW_STEP));
                    None
                }
                code => {
                    scroll_page(&mut page.scroll, code);
                    page.reveal = false;
                    None
                }
            };
            if let Some(selected) = selected {
                page.selected = selected;
                page.reveal = true;
                // Each file's patch is shown from its top.
                page.preview = page.preview.map(|_| 0);
            }
            self.report_page = Some(page);
            return Outcome::Handled;
        }
        if self.commits.open && key.code == KeyCode::Tab {
            self.commits.focused = !self.commits.focused;
            return Outcome::Handled;
        }
        if self.commits.open && self.commits.focused {
            let selected = self.commits.selected;
            let last = self.commits.entries.len();
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.select_commit((selected + 1).min(last)),
                KeyCode::Up | KeyCode::Char('k') => self.select_commit(selected.saturating_sub(1)),
                KeyCode::PageDown => self.select_commit(selected.saturating_add(5).min(last)),
                KeyCode::PageUp => self.select_commit(selected.saturating_sub(5)),
                KeyCode::Char('g') => self.select_commit(0),
                KeyCode::Char('G') => self.select_commit(last),
                KeyCode::Enter => self.commits.focused = false,
                KeyCode::Esc => {
                    self.select_commit(0);
                    self.commits.focused = false;
                }
                KeyCode::Char('h') | KeyCode::Char('?') => {}
                _ => return Outcome::Handled,
            }
            if !matches!(key.code, KeyCode::Char('h') | KeyCode::Char('?')) {
                return Outcome::Handled;
            }
        }
        let Some(binding) = BINDINGS
            .iter()
            .find(|binding| binding.codes.contains(&key.code))
        else {
            return Outcome::Ignored;
        };
        match binding.command {
            Command::ToggleCommits => {
                if self.target().is_some() {
                    self.commits.open = !self.commits.open;
                    self.commits.focused = self.commits.open;
                    if !self.commits.open {
                        self.select_commit(0);
                    }
                }
                Outcome::Handled
            }
            Command::ToggleHelp => {
                self.help = Some(0);
                Outcome::Handled
            }
            Command::ReportPage => {
                // On the file the view is on.
                self.report_page = Some(crate::ReportPage {
                    selected: self.selected_file,
                    scroll: 0,
                    reveal: true,
                    preview: self.report_preview.then_some(0),
                });
                Outcome::Handled
            }
            Command::View(action) => {
                self.apply(action);
                Outcome::Handled
            }
            Command::ClearFilterOrClose
                if self.clear_find()
                    || self.leave_lines()
                    || self.clear_filter()
                    || self.clear_commit() =>
            {
                Outcome::Handled
            }
            Command::ClearFilterOrClose => Outcome::Close,
            Command::Toggle => Outcome::Toggle,
            Command::OpenFile => Outcome::OpenFile,
            Command::GenerateReport => Outcome::GenerateReport,
        }
    }
}

/// Rows `J` and `K` scroll the report page's patch by.
const PREVIEW_STEP: u16 = 3;

/// Scroll a page drawn over the view — the shortcut legend, or the edit
/// report's — by the reading keys; the drawing holds it to what fits.
fn scroll_page(scroll: &mut u16, code: KeyCode) {
    match code {
        KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
        KeyCode::PageDown => *scroll = scroll.saturating_add(10),
        KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
        KeyCode::Char('g') => *scroll = 0,
        _ => {}
    }
}

#[cfg(test)]
mod tests;

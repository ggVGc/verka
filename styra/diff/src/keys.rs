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
        &[KeyCode::Char('l')],
        "l",
        "toggle combined / per-file diffs",
        Command::View(Action::ToggleFiles),
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
        "go to the top",
        Command::View(Action::First),
    ),
    bind(
        &[KeyCode::Char('G')],
        "G",
        "go to the bottom",
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
    /// Answer one key. The prompts for text to find and for the file-name
    /// filter take every key while they are open; otherwise the key is looked
    /// up in [`BINDINGS`].
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
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
        let Some(binding) = BINDINGS
            .iter()
            .find(|binding| binding.codes.contains(&key.code))
        else {
            return Outcome::Ignored;
        };
        match binding.command {
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
                if self.clear_find() || self.leave_lines() || self.clear_filter() =>
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
mod tests {
    use super::*;
    use crate::state::DiffTarget;

    fn diff() -> CheckoutDiff {
        let mut diff = CheckoutDiff::read(DiffTarget {
            worktree: std::env::temp_dir().join("styra-diff-keys-no-such-repository"),
            branch: None,
            base_branch: None,
            base: "HEAD".into(),
            branch_point: false,
        });
        diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                        diff --git a/q.txt b/q.txt\n@@ -1 +1 @@\n-c\n+d\n"
            .into());
        diff
    }

    #[test]
    fn shortcut_legend_is_modal_and_preserves_selection() {
        let mut diff = diff();
        diff.line_cursor = Some(3);
        assert_eq!(press(&mut diff, KeyCode::Char('?')), Outcome::Handled);
        assert_eq!(diff.help, Some(0));
        press(&mut diff, KeyCode::Down);
        assert_eq!(diff.help, Some(1));
        press(&mut diff, KeyCode::Char('l'));
        assert!(!diff.per_file);
        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
        assert_eq!(diff.help, None);
        assert_eq!(diff.line_cursor, Some(3));
        press(&mut diff, KeyCode::Char('?'));
        press(&mut diff, KeyCode::Char('?'));
        assert_eq!(diff.help, None);
    }

    /// The report page is modal like the legend: j/k pick a file on it,
    /// which it opens on, the other reading keys scroll it, a report can be
    /// asked for from it, and Enter opens the picked file's patch.
    #[test]
    fn the_report_page_picks_a_file_to_open() {
        let mut diff = diff();
        let page = |diff: &CheckoutDiff| diff.report_page.unwrap();
        press(&mut diff, KeyCode::Char('l'));
        press(&mut diff, KeyCode::Char('j'));
        press(&mut diff, KeyCode::Char('l'));
        assert_eq!(press(&mut diff, KeyCode::Char('o')), Outcome::Handled);
        assert_eq!((page(&diff).selected, page(&diff).reveal), (1, true));
        press(&mut diff, KeyCode::PageDown);
        assert_eq!((page(&diff).scroll, page(&diff).reveal), (10, false));
        press(&mut diff, KeyCode::Char('j'));
        assert_eq!((page(&diff).selected, page(&diff).reveal), (1, true));
        press(&mut diff, KeyCode::Char('k'));
        assert_eq!(page(&diff).selected, 0);
        press(&mut diff, KeyCode::Char('G'));
        assert_eq!(page(&diff).selected, 1);
        press(&mut diff, KeyCode::Char('l'));
        assert!(!diff.per_file, "the page has the keys");
        assert_eq!(
            press(&mut diff, KeyCode::Char('E')),
            Outcome::GenerateReport
        );
        assert!(diff.report_page.is_some(), "open for the report to come");
        assert_eq!(
            press(&mut diff, KeyCode::Char('e')),
            Outcome::DescribeFile("q.txt".into()),
            "the picked file alone"
        );
        assert!(
            diff.report_page.is_some(),
            "open for its description to come"
        );

        assert_eq!(page(&diff).preview, None);
        press(&mut diff, KeyCode::Char('K'));
        assert_eq!(page(&diff).preview, None, "nothing to scroll");
        press(&mut diff, KeyCode::Char('p'));
        press(&mut diff, KeyCode::Char('J'));
        press(&mut diff, KeyCode::Char('J'));
        assert_eq!(page(&diff).preview, Some(6));
        press(&mut diff, KeyCode::Char('K'));
        assert_eq!(page(&diff).preview, Some(3));
        press(&mut diff, KeyCode::Char('k'));
        assert_eq!(page(&diff).preview, Some(0), "the next file from its top");
        press(&mut diff, KeyCode::Char('j'));

        assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
        assert_eq!(diff.report_page, None);
        assert!(diff.per_file);
        assert_eq!(diff.selected_file, 1);

        press(&mut diff, KeyCode::Char('o'));
        press(&mut diff, KeyCode::Char('g'));
        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
        assert_eq!(diff.report_page, None);
        assert_eq!(diff.selected_file, 1, "closed without opening");

        press(&mut diff, KeyCode::Char('o'));
        assert_eq!(page(&diff).preview, Some(0), "it opens as it was left");
        press(&mut diff, KeyCode::Char('p'));
        press(&mut diff, KeyCode::Esc);
        press(&mut diff, KeyCode::Char('o'));
        assert_eq!(page(&diff).preview, None);
    }

    fn press(diff: &mut CheckoutDiff, code: KeyCode) -> Outcome {
        diff.handle_key(KeyEvent::from(code))
    }

    /// Esc clears a file filter before it closes the view, and a letter typed
    /// into the filter is part of it rather than a command.
    #[test]
    fn esc_closes_once_a_filter_is_cleared() {
        let mut diff = diff();
        assert_eq!(press(&mut diff, KeyCode::Char('l')), Outcome::Handled);
        assert_eq!(press(&mut diff, KeyCode::Char('/')), Outcome::Handled);
        assert_eq!(press(&mut diff, KeyCode::Char('q')), Outcome::Handled);
        assert_eq!(diff.search.query(), Some("q"));
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);

        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
        assert_eq!(diff.search.query(), None);
        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Close);

        assert_eq!(press(&mut diff, KeyCode::Char('/')), Outcome::Handled);
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Handled);
        assert_eq!(diff.search.query(), Some("d"));
        assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Toggle);
    }

    #[test]
    fn view_keys_act_and_the_rest_go_to_the_host() {
        let mut diff = diff();
        for key in ['r', 'l', 'j', 'C', 't', 'e', 'w', 'L', 'm'] {
            assert_eq!(press(&mut diff, KeyCode::Char(key)), Outcome::Handled);
        }
        assert!(!diff.word_highlights);
        assert!(diff.thumbnail_hidden);
        assert!(
            matches!(diff.layers, Some(Err(_))),
            "layered, saying why there is no repository to read them from"
        );
        assert!(diff.report.hidden);
        assert!(diff.hide_removed);
        assert!(diff.per_file);
        assert!(!diff.compact_dirs);
        assert_eq!(diff.tint, crate::Tint::Half);
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Char('v')), Outcome::OpenFile);
        assert_eq!(
            press(&mut diff, KeyCode::Char('E')),
            Outcome::GenerateReport
        );
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Toggle);
        for key in ['q', 'i', 'y'] {
            assert_eq!(press(&mut diff, KeyCode::Char(key)), Outcome::Ignored);
        }
        let control_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(diff.handle_key(control_r), Outcome::Ignored);
    }

    /// Esc leaves line selection before it clears a filter or closes, and
    /// Enter or `V` leaves it as it entered.
    #[test]
    fn esc_leaves_line_selection_first() {
        let mut diff = diff();
        assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
        assert_eq!(diff.line_cursor, Some(2), "-a, the first line of a hunk");
        assert_eq!(press(&mut diff, KeyCode::Char('j')), Outcome::Handled);
        assert_eq!(diff.line_cursor, Some(3));
        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
        assert_eq!(diff.line_cursor, None);
        assert_eq!(press(&mut diff, KeyCode::Char('V')), Outcome::Handled);
        assert_eq!(press(&mut diff, KeyCode::Char('V')), Outcome::Handled);
        assert_eq!(diff.line_cursor, None);
        assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Close);
    }

    /// In line selection j and k keep the cursor to added and removed
    /// lines, stepping over unchanged lines, file headers and hunk cuts to
    /// the next change; J and K go to the first line of the next and previous
    /// block of changes, and g and G to the first and last change. The arrows
    /// go over every line.
    #[test]
    fn line_selection_moves_through_changes_only() {
        let mut diff = diff();
        // Lines: a.txt's header 0, hunk 1, " x" 2, "-a" 3, "+b" 4, hunk 5,
        // "-c" 6, then twelve added lines 7 to 18, b.txt's header 19, hunk
        // 20, " y" 21, "+d" 22.
        diff.diff = Ok(format!(
            "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n x\n-a\n+b\n\
             @@ -9 +9,12 @@\n-c\n{}\
             diff --git a/b.txt b/b.txt\n@@ -1 +1,2 @@\n y\n+d\n",
            (0..12).map(|n| format!("+add{n}\n")).collect::<String>()
        ));
        diff.scroll.note_viewport(40);
        let mut keys = |code, expected: usize, why: &str| {
            assert_eq!(press(&mut diff, code), Outcome::Handled);
            assert_eq!(diff.line_cursor, Some(expected), "{code:?}: {why}");
        };
        keys(
            KeyCode::Enter,
            3,
            "the first change, past headers and \" x\"",
        );
        keys(KeyCode::Char('j'), 4, "+b");
        keys(KeyCode::Char('j'), 6, "over the second hunk's cut to -c");
        keys(KeyCode::Char('J'), 22, "the next block, b.txt's +d");
        keys(KeyCode::Char('J'), 22, "no block further down");
        keys(KeyCode::Char('K'), 6, "the previous block's first line");
        keys(KeyCode::Char('K'), 3, "and the one before");
        keys(KeyCode::Char('K'), 3, "no block further up");
        keys(KeyCode::Char('j'), 4, "+b");
        keys(KeyCode::Char('J'), 6, "from inside a block, the next one");
        keys(KeyCode::Char('G'), 22, "the last change, over \" y\"");
        keys(
            KeyCode::Char('k'),
            18,
            "over b.txt's header, cut and \" y\"",
        );
        keys(
            KeyCode::Char('K'),
            6,
            "from inside a block, its own first line",
        );
        keys(KeyCode::Char('g'), 3, "the first change");
        keys(KeyCode::Char('k'), 3, "nowhere further up");
        // The arrows go over every line, unchanged ones, headers and cuts too.
        keys(KeyCode::Up, 2, "\" x\"");
        keys(KeyCode::Up, 1, "the first hunk's cut");
        keys(KeyCode::Up, 0, "a.txt's header");
        keys(KeyCode::Up, 0, "nowhere further up");
        keys(KeyCode::Down, 1, "the cut again");
        keys(KeyCode::Char('j'), 3, "from the cut, the first change");
        keys(KeyCode::Char('j'), 4, "+b");
        keys(KeyCode::Down, 5, "the second hunk's cut");
        keys(KeyCode::Char('k'), 4, "back to +b");
        let control = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(diff.handle_key(control), Outcome::Ignored);
    }

    /// A key answers to one command, so the table cannot shadow a row.
    #[test]
    fn no_key_is_bound_twice() {
        let codes: Vec<_> = BINDINGS.iter().flat_map(|binding| binding.codes).collect();
        for (index, code) in codes.iter().enumerate() {
            assert!(!codes[index + 1..].contains(code), "{code:?} bound twice");
        }
    }
}

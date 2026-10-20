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
    /// Leave line selection, or clear a standing file filter, or else close
    /// the view.
    ClearFilterOrClose,
    Close,
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
        "f",
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
        &[KeyCode::Char('r')],
        "r",
        "toggle visibility of removed diff lines",
        Command::View(Action::ToggleRemoved),
    ),
    bind(
        &[KeyCode::Char('c')],
        "c (per-file view)",
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
    bind(&[KeyCode::Char('d')], "d", "close the diff", Command::Close),
    bind(
        &[KeyCode::Esc],
        "Esc",
        "leave line selection, clear the file filter, or close the diff",
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
    /// Open the file at the top of the view in the editor, at
    /// [`CheckoutDiff::location`].
    OpenFile,
    /// Have each file's edits described, and put the report in
    /// [`CheckoutDiff::report`] when it comes.
    GenerateReport,
}

impl CheckoutDiff {
    /// Answer one key. The file-name filter's prompt takes every key while it
    /// is open; otherwise the key is looked up in [`BINDINGS`].
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
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
        let Some(binding) = BINDINGS
            .iter()
            .find(|binding| binding.codes.contains(&key.code))
        else {
            return Outcome::Ignored;
        };
        match binding.command {
            Command::View(action) => {
                self.apply(action);
                Outcome::Handled
            }
            Command::ClearFilterOrClose if self.leave_lines() || self.clear_filter() => {
                Outcome::Handled
            }
            Command::ClearFilterOrClose | Command::Close => Outcome::Close,
            Command::OpenFile => Outcome::OpenFile,
            Command::GenerateReport => Outcome::GenerateReport,
        }
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

    fn press(diff: &mut CheckoutDiff, code: KeyCode) -> Outcome {
        diff.handle_key(KeyEvent::from(code))
    }

    /// Esc clears a file filter before it closes the view, and a letter typed
    /// into the filter is part of it rather than a command.
    #[test]
    fn esc_closes_once_a_filter_is_cleared() {
        let mut diff = diff();
        assert_eq!(press(&mut diff, KeyCode::Char('f')), Outcome::Handled);
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
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Close);
    }

    #[test]
    fn view_keys_act_and_the_rest_go_to_the_host() {
        let mut diff = diff();
        for key in ['r', 'f', 'j', 'c', 't', 'e', 'w', 'L', 'm'] {
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
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Close);
        for key in ['q', 'i', 'y', '?'] {
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

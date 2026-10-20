//! `styra-diff`: the diff view Styra opens with `d`, as a program of its own,
//! over whichever repository it is run in, or over a patch it is given.

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use clap::Parser;
// The terminal is set up with the same crossterm keys are read with, rather
// than ratatui's own: crossterm reads ctrl-j as Enter unless it is the one
// that put the terminal in raw mode.
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::DefaultTerminal;
use styra_diff::{git, render, CheckoutDiff, CheckoutDiffFeedback, Outcome, Supplied};
use styra_ui::theme;

/// Browse a Git working tree's changes, or a patch, as Styra's diff view
/// shows them: syntax-highlighted, whole or one file at a time.
#[derive(Parser)]
#[command(name = "styra-diff", version)]
struct Cli {
    /// Show the work since the current branch left BASE — their merge base —
    /// uncommitted changes included. Without it, what is not committed yet.
    #[arg(conflicts_with = "patch")]
    base: Option<String>,
    /// Show the Git-style patch in FILE, or on standard input for `-`, rather
    /// than reading one from the repository: `git show | styra-diff -p -`.
    #[arg(short, long, value_name = "FILE")]
    patch: Option<PathBuf>,
    /// Read the repository DIR is in rather than the current directory's. With
    /// a patch, where its files are opened from.
    #[arg(short = 'C', value_name = "DIR", default_value = ".")]
    directory: PathBuf,
    /// List the per-file view's files in a strip under the patch rather than
    /// in a column beside it.
    #[arg(long)]
    files_below: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Resolved before the terminal is taken over, so a mistyped base, a
    // directory outside any repository, or a patch that cannot be read is an
    // ordinary error message.
    let mut diff = match &cli.patch {
        Some(patch) => supplied(patch, &cli.directory)?,
        None => {
            let target =
                git::target(&cli.directory, cli.base.as_deref()).map_err(anyhow::Error::msg)?;
            CheckoutDiff::read(target)
        }
    };
    let mut terminal = init().context("taking over the terminal")?;
    let result = run(&mut terminal, &mut diff, cli.files_below);
    restore();
    result
}

/// Put the terminal in raw mode on the alternate screen, and have a panic
/// put it back before saying what went wrong.
fn init() -> io::Result<DefaultTerminal> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        hook(info);
    }));
    ratatui::Terminal::new(CrosstermBackend::new(io::stdout()))
}

/// Give the terminal back as it was before [`init`].
fn restore() {
    disable_raw_mode().ok();
    execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show).ok();
}

/// The patch at `path`, or on standard input for `-`, with its files opened
/// from the top of the repository `directory` is in — where Git's paths start
/// — or from `directory` itself outside one. Keys are still read from the
/// terminal when standard input is the patch.
fn supplied(path: &Path, directory: &Path) -> anyhow::Result<CheckoutDiff> {
    let (text, label) = if path == Path::new("-") {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .context("reading the patch from standard input")?;
        (text, "patch from standard input".to_owned())
    } else {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the patch {}", path.display()))?;
        (text, format!("patch {}", path.display()))
    };
    let root = git::toplevel(directory).map_or_else(
        |_| std::path::absolute(directory).unwrap_or_else(|_| directory.to_owned()),
        PathBuf::from,
    );
    Ok(CheckoutDiff::supplied(
        text,
        Supplied {
            label,
            root: Some(root),
        },
    ))
}

fn run(
    terminal: &mut DefaultTerminal,
    diff: &mut CheckoutDiff,
    files_below: bool,
) -> anyhow::Result<()> {
    // Why the last key did nothing — a `v` that opened no file, say — until
    // the next key.
    let mut notice: Option<String> = None;
    loop {
        let mut measured = CheckoutDiffFeedback::default();
        terminal
            .draw(|frame| {
                let mut view = diff.view(block(notice.as_deref()));
                view.thumbnail = true;
                view.files_below = files_below;
                // There is no agent here to write an edit report with.
                view.report = None;
                measured = render(frame, &view, frame.area());
            })
            .context("drawing the diff")?;
        diff.note(measured);
        let Event::Key(key) = event::read().context("reading the terminal")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        notice = None;
        match handle(diff, key) {
            Step::Stay => {}
            Step::Quit => return Ok(()),
            Step::Notice(text) => notice = Some(text.to_owned()),
            Step::Edit => {
                let edited = diff
                    .location()
                    .and_then(|(path, line)| edit(terminal, &path, line));
                notice = edited.err();
            }
        }
    }
}

/// The frame around the view, saying how to leave it — there is no Styra
/// around it to go back to — and why the last key did nothing.
fn block(notice: Option<&str>) -> Block<'static> {
    let muted = Style::default().fg(theme::MUTED_TEXT);
    let mut title = vec![
        Span::styled(" styra-diff", Style::default().fg(theme::TEXT)),
        Span::styled(" · q: quit ", muted),
    ];
    if let Some(notice) = notice {
        title.push(Span::styled(
            format!("· {notice} "),
            Style::default().fg(theme::ERROR),
        ));
    }
    Block::bordered()
        .border_style(Style::default().fg(theme::ACCENT))
        .title(Line::from(title))
}

/// Open `path` at `line` in the editor, in this terminal: the view steps out
/// of the way until the editor is done, then is drawn afresh.
fn edit(terminal: &mut DefaultTerminal, path: &Path, line: Option<u32>) -> Result<(), String> {
    let mut command = editor(|name| std::env::var(name).ok());
    let program = command.get_program().to_string_lossy().into_owned();
    // `+N` is how vi, Emacs, nano and their kin are told where to start.
    command.args(line.map(|line| format!("+{line}"))).arg(path);
    // A patch read from standard input leaves the editor nothing there to
    // read keys from; the terminal is where they come from.
    if !io::stdin().is_terminal() {
        if let Ok(tty) = std::fs::File::open("/dev/tty") {
            command.stdin(tty);
        }
    }
    let leave = || -> io::Result<()> {
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)
    };
    let enter = |terminal: &mut DefaultTerminal| -> io::Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        terminal.clear()
    };
    leave().map_err(|error| format!("could not leave the terminal to {program}: {error}"))?;
    let status = command.status();
    enter(terminal).map_err(|error| format!("could not take the terminal back: {error}"))?;
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{program} {status}")),
        Err(error) => Err(format!("could not run {program}: {error}")),
    }
}

/// The editor `$VISUAL` names, or else `$EDITOR`, its words split on white
/// space so it may carry arguments of its own; Neovim when neither is set.
fn editor(variable: impl Fn(&str) -> Option<String>) -> Command {
    let configured = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(variable)
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "nvim".to_owned());
    let mut words = configured.split_whitespace();
    let mut command = Command::new(words.next().unwrap_or("nvim"));
    command.args(words);
    command
}

/// What a key leaves the program to do once the view has had it.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Stay,
    Quit,
    /// Open the file at the top of the view in the editor.
    Edit,
    /// Say why a key did nothing.
    Notice(&'static str),
}

/// The view's own keys, as Styra answers them, with Esc closing the view —
/// which here is the program. `q` and ctrl-c quit too: they are the program's,
/// not the view's, as there is no Styra around it to go back to.
fn handle(diff: &mut CheckoutDiff, key: KeyEvent) -> Step {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Step::Quit;
    }
    match diff.handle_key(key) {
        Outcome::Handled => Step::Stay,
        Outcome::Close => Step::Quit,
        // There is no Styra to go back to, so `d` does not quit.
        Outcome::Toggle => Step::Stay,
        Outcome::OpenFile => Step::Edit,
        Outcome::GenerateReport => {
            Step::Notice("edit reports are written in Styra, which has an agent to ask")
        }
        Outcome::Ignored if key.code == KeyCode::Char('q') => Step::Quit,
        Outcome::Ignored => Step::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use styra_diff::DiffTarget;

    fn diff() -> CheckoutDiff {
        let mut diff = CheckoutDiff::read(DiffTarget {
            worktree: std::env::temp_dir().join("styra-diff-no-such-repository"),
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

    fn press(diff: &mut CheckoutDiff, code: KeyCode) -> Step {
        handle(diff, KeyEvent::from(code))
    }

    /// Esc clears a file filter before it quits, and `q` typed into the
    /// filter is part of it rather than a way out. The view's keys are
    /// [`styra_diff::keys`]'s; these only check how the program answers them.
    #[test]
    fn q_and_esc_quit_once_nothing_else_wants_them() {
        let mut diff = diff();
        assert_eq!(press(&mut diff, KeyCode::Char('l')), Step::Stay);
        assert_eq!(press(&mut diff, KeyCode::Char('/')), Step::Stay);
        assert_eq!(press(&mut diff, KeyCode::Char('q')), Step::Stay);
        assert_eq!(diff.search.query(), Some("q"));
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Enter), Step::Stay);

        assert_eq!(press(&mut diff, KeyCode::Esc), Step::Stay);
        assert_eq!(diff.search.query(), None);
        assert_eq!(press(&mut diff, KeyCode::Char('d')), Step::Stay);
        assert_eq!(press(&mut diff, KeyCode::Esc), Step::Quit);
        assert_eq!(press(&mut diff, KeyCode::Char('q')), Step::Quit);
        assert_eq!(
            handle(
                &mut diff,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
            ),
            Step::Quit
        );
    }

    #[test]
    fn the_view_keys_are_styras() {
        let mut diff = diff();
        for key in ['r', 'l', 'j', 'c', 't'] {
            assert_eq!(press(&mut diff, KeyCode::Char(key)), Step::Stay);
        }
        assert!(diff.hide_removed);
        assert!(diff.per_file);
        assert!(!diff.compact_dirs);
        assert_eq!(diff.tint, styra_diff::Tint::Half);
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Char('v')), Step::Edit);
        assert!(matches!(
            press(&mut diff, KeyCode::Char('E')),
            Step::Notice(_)
        ));
    }

    /// `$VISUAL` before `$EDITOR`, either with arguments of its own, and
    /// Neovim when neither says.
    #[test]
    fn the_editor_is_visual_then_editor_then_neovim() {
        let argv = |command: Command| {
            std::iter::once(command.get_program().to_owned())
                .chain(command.get_args().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        };
        let set = |visual: &'static str, editor: &'static str| {
            move |name: &str| {
                Some(if name == "VISUAL" { visual } else { editor }.to_owned())
                    .filter(|value| !value.is_empty())
            }
        };
        assert_eq!(argv(editor(set("", ""))), ["nvim"]);
        assert_eq!(argv(editor(set("", "vim"))), ["vim"]);
        assert_eq!(argv(editor(set("code -w", "vim"))), ["code", "-w"]);
        assert_eq!(argv(editor(set("  ", "hx"))), ["hx"]);
    }

    /// A directory outside any repository is said to be one before the
    /// terminal is touched, rather than drawn as an empty diff.
    #[test]
    fn outside_a_repository_there_is_nothing_to_show() {
        let outside = std::env::temp_dir().join("styra-diff-outside");
        std::fs::create_dir_all(&outside).unwrap();
        let error = git::target(&outside, None).unwrap_err();
        assert!(error.starts_with("git rev-parse failed"), "{error}");
    }

    #[test]
    fn the_command_line_parses() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        let cli = Cli::parse_from(["styra-diff", "-C", "/repo", "main"]);
        assert_eq!(cli.base.as_deref(), Some("main"));
        assert_eq!(cli.directory, PathBuf::from("/repo"));
        let cli = Cli::parse_from(["styra-diff", "-p", "-"]);
        assert_eq!(cli.patch, Some(PathBuf::from("-")));
        assert!(Cli::try_parse_from(["styra-diff", "-p", "-", "main"]).is_err());
    }

    /// A patch file is shown as given, with its files opened from where it
    /// was run, and is not read again from Git.
    #[test]
    fn a_patch_file_is_shown_as_given() {
        let directory = std::env::temp_dir().join("styra-diff-patch-file");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("turn.patch");
        let patch = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n";
        std::fs::write(&file, patch).unwrap();
        std::fs::write(directory.join("a.txt"), "b\n").unwrap();

        let mut diff = supplied(&file, &directory).unwrap();
        assert_eq!(diff.diff.as_deref(), Ok(patch));
        assert!(diff.target().is_none());
        assert!(!diff.refresh());
        assert_eq!(diff.location(), Ok((directory.join("a.txt"), None)));
        assert!(supplied(&directory.join("missing.patch"), &directory).is_err());
        let _ = std::fs::remove_dir_all(&directory);
    }
}

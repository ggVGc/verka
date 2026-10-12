//! `styra-diff`: the diff view Styra opens with `d`, as a program of its own,
//! over whichever repository it is run in.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::DefaultTerminal;
use styra_diff::{git, render, CheckoutDiff, CheckoutDiffFeedback, Outcome};
use styra_ui::theme;

/// Browse a Git working tree's changes as Styra's diff view shows them:
/// syntax-highlighted, whole or one file at a time.
#[derive(Parser)]
#[command(name = "styra-diff", version)]
struct Cli {
    /// Show the work since the current branch left BASE — their merge base —
    /// uncommitted changes included. Without it, what is not committed yet.
    base: Option<String>,
    /// Read the repository DIR is in rather than the current directory's.
    #[arg(short = 'C', value_name = "DIR", default_value = ".")]
    directory: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Resolved before the terminal is taken over, so a mistyped base or a
    // directory outside any repository is an ordinary error message.
    let target = git::target(&cli.directory, cli.base.as_deref()).map_err(anyhow::Error::msg)?;
    let mut diff = CheckoutDiff::read(target);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut diff);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, diff: &mut CheckoutDiff) -> anyhow::Result<()> {
    // Why the last `v` opened nothing, until the next key.
    let mut notice: Option<String> = None;
    loop {
        let mut measured = CheckoutDiffFeedback::default();
        terminal
            .draw(|frame| {
                let view = diff.view(block(notice.as_deref()));
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
/// around it to go back to — and why a file did not open.
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
        Outcome::OpenFile => Step::Edit,
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
        assert_eq!(press(&mut diff, KeyCode::Char('f')), Step::Stay);
        assert_eq!(press(&mut diff, KeyCode::Char('/')), Step::Stay);
        assert_eq!(press(&mut diff, KeyCode::Char('q')), Step::Stay);
        assert_eq!(diff.search.query(), Some("q"));
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Enter), Step::Stay);

        assert_eq!(press(&mut diff, KeyCode::Esc), Step::Stay);
        assert_eq!(diff.search.query(), None);
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
        for key in ['r', 'f', 'j', 'c', 't'] {
            assert_eq!(press(&mut diff, KeyCode::Char(key)), Step::Stay);
        }
        assert!(diff.hide_removed);
        assert!(diff.per_file);
        assert!(!diff.compact_dirs);
        assert_eq!(diff.tint, styra_diff::Tint::Half);
        assert_eq!(diff.selected_file, 1);
        assert_eq!(press(&mut diff, KeyCode::Char('v')), Step::Edit);
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
    }
}

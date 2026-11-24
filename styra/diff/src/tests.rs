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
    for key in ['r', 'l', 'j', 'C', 't'] {
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

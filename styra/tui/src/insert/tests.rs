use super::*;
use crate::keybindings::{self as keys, press};
use crossterm::event::KeyEvent;
use styra_protocol::{AttributedMount, MountAccess, MountOrigin};

/// A scratch tree to complete and resolve against, named per test so the
/// cases stay independent of each other.
fn tree(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("styra-insert-{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("reports/quarterly")).unwrap();
    std::fs::create_dir_all(base.join("readme.d")).unwrap();
    std::fs::write(base.join("reports/summary.md"), "x").unwrap();
    std::fs::write(base.join("notes.txt"), "x").unwrap();
    base
}

#[test]
fn completion_extends_only_as_far_as_the_candidates_agree() {
    let base = tree("completion");

    // `reports` and `readme.d` agree on no more than the `re` already
    // typed, so `Tab` has nothing to add rather than picking one of them.
    assert_eq!(complete(Some(&base), "re"), None);
    // A sole directory match gains its separator, so the next Tab descends.
    assert_eq!(complete(Some(&base), "rep").unwrap(), "reports/");
    // Inside it the two entries share nothing, so again nothing is added.
    assert_eq!(complete(Some(&base), "reports/"), None);
    assert_eq!(
        complete(Some(&base), "reports/s").unwrap(),
        "reports/summary.md"
    );
    // Nothing matches, and nothing is invented.
    assert_eq!(complete(Some(&base), "zz"), None);
    // The text before the last `/` is left exactly as it was typed.
    let relative = format!("{}/rep", base.display());
    assert_eq!(
        complete(None, &relative).unwrap(),
        format!("{}/reports/", base.display())
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_typed_path_resolves_against_the_workspace_and_must_exist() {
    let base = tree("resolve");

    assert_eq!(
        resolve(Some(&base), "notes.txt").unwrap(),
        std::fs::canonicalize(base.join("notes.txt")).unwrap()
    );
    assert_eq!(
        resolve(Some(&base), " reports/../notes.txt ").unwrap(),
        std::fs::canonicalize(base.join("notes.txt")).unwrap()
    );
    assert!(resolve(Some(&base), "").is_err());
    // Not there is not mountable, so it is refused here rather than at the
    // launch that would have carried it.
    assert!(resolve(Some(&base), "absent.txt").is_err());
    // With nowhere to resolve against, a relative path is not a path.
    assert!(resolve(None, "notes.txt").is_err());

    let _ = std::fs::remove_dir_all(&base);
}

/// A sandbox that binds `root` at `/workspace` and nothing else.
fn driva(root: &Path) -> DrivaOptions {
    DrivaOptions {
        isolation_backend: "bwrap".into(),
        command: vec!["codex".into()],
        working_directory: PathBuf::from("/workspace"),
        network: false,
        base: Vec::new(),
        mounts: vec![AttributedMount {
            origin: MountOrigin::Workspace,
            mount: Mount::Bind {
                source: root.to_path_buf(),
                destination: PathBuf::from("/workspace"),
                access: MountAccess::ReadWrite,
            },
        }],
        ..Default::default()
    }
}

/// A prompt over that sandbox, with the policy still open to being edited.
fn prompt(root: &Path) -> Prompt {
    Prompt::new(Some(root.to_path_buf()), Some(&driva(root)), true)
}

/// Type `text` and accept it, returning what the prompt decided.
fn typed(prompt: &mut Prompt, text: &str) -> Outcome {
    for ch in text.chars() {
        prompt.key(KeyEvent::from(KeyCode::Char(ch)));
    }
    prompt.key(KeyEvent::from(KeyCode::Enter))
}

/// A path the sandbox already carries is decided straight away — and in the
/// agent's terms, not the operator's, since those are the same file under
/// two names and only one of them means anything to the agent.
#[test]
fn a_mounted_path_is_inserted_in_the_agents_terms() {
    let root = std::fs::canonicalize(tree("mounted")).unwrap();
    let mut prompt = prompt(&root);

    let outcome = typed(&mut prompt, "reports/summary.md");

    let Outcome::Insert { path, .. } = outcome else {
        panic!("expected an insert, got {outcome:?}");
    };
    assert_eq!(path, PathBuf::from("/workspace/reports/summary.md"));

    let _ = std::fs::remove_dir_all(&root);
}

/// A path outside every mount stops for the second question, and answering
/// it grants the path and inserts it.
#[test]
fn an_unmounted_path_asks_before_it_is_inserted() {
    let root = std::fs::canonicalize(tree("unmounted")).unwrap();
    let outside = std::fs::canonicalize(tree("unmounted-elsewhere")).unwrap();
    let mut prompt = prompt(&root);
    let host = outside.join("notes.txt");

    let outcome = typed(&mut prompt, &host.display().to_string());

    assert_eq!(
        (outcome, prompt.state()),
        (Outcome::Open, &Insert::Grant(host.clone())),
        "an unmounted path is not inserted until the question is answered"
    );

    assert_eq!(
        prompt.key(press(&keys::EDITOR_MOUNT_WRITABLE)),
        Outcome::Grant {
            mount: LaunchMount {
                source: host.clone(),
                destination: None,
                writable: true,
            },
            // Bound at its own name, so what the agent will call it is what
            // the operator typed.
            path: host,
        }
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// Mounting neither is the answer that grants nothing: the path is still
/// inserted, because naming a file the agent cannot open is a legitimate
/// thing to do.
#[test]
fn declining_the_grant_still_inserts_the_path() {
    let root = std::fs::canonicalize(tree("declined")).unwrap();
    let outside = std::fs::canonicalize(tree("declined-elsewhere")).unwrap();
    let mut prompt = prompt(&root);
    let host = outside.join("notes.txt");

    typed(&mut prompt, &host.display().to_string());
    let outcome = prompt.key(press(&keys::EDITOR_MOUNT_NEITHER));

    let Outcome::Insert { path, notice } = outcome else {
        panic!("expected an insert, got {outcome:?}");
    };
    assert_eq!(path, host);
    assert!(notice.unwrap().contains("cannot reach it"));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// A working agent's sandbox cannot be restarted under it, so there is no
/// grant to offer. The path is inserted and the limit is said out loud,
/// rather than a question being asked whose answers would all fail.
#[test]
fn a_fixed_sandbox_is_told_rather_than_asked() {
    let root = std::fs::canonicalize(tree("running")).unwrap();
    let outside = std::fs::canonicalize(tree("running-elsewhere")).unwrap();
    let mut prompt = Prompt::new(Some(root.clone()), Some(&driva(&root)), false);
    let host = outside.join("notes.txt");

    let outcome = typed(&mut prompt, &host.display().to_string());

    let Outcome::Insert { path, notice } = outcome else {
        panic!("expected an insert, got {outcome:?}");
    };
    assert_eq!(path, host);
    assert!(notice.unwrap().contains("outside the sandbox"));

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// With no policy to check against there is nothing to rewrite and nothing
/// to grant, so the path goes in as it is and the box says why.
#[test]
fn without_a_sandbox_the_path_goes_in_unchanged() {
    let root = std::fs::canonicalize(tree("nosandbox")).unwrap();
    let mut prompt = Prompt::new(Some(root.clone()), None, true);

    let outcome = typed(&mut prompt, "notes.txt");

    let Outcome::Insert { path, notice } = outcome else {
        panic!("expected an insert, got {outcome:?}");
    };
    assert_eq!(path, root.join("notes.txt"));
    assert!(notice.unwrap().contains("no sandbox policy known"));

    let _ = std::fs::remove_dir_all(&root);
}

/// A path that is not there cannot be mounted, so the prompt says so and
/// stays open on the text — which is usually one keystroke from right.
#[test]
fn a_path_that_does_not_exist_leaves_the_prompt_open() {
    let root = std::fs::canonicalize(tree("absent")).unwrap();
    let mut prompt = prompt(&root);

    let outcome = typed(&mut prompt, "reprots/summary.md");

    assert!(matches!(outcome, Outcome::Notice(_)), "{outcome:?}");
    assert_eq!(prompt.state(), &Insert::Typing("reprots/summary.md".into()));

    // Tab completion is what fixes it, and Esc abandons it either way.
    assert_eq!(prompt.key(KeyEvent::from(KeyCode::Esc)), Outcome::Closed);

    let _ = std::fs::remove_dir_all(&root);
}

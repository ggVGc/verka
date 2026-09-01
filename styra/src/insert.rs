//! Putting a host path into the message being composed — and, when the sandbox
//! cannot see that path, granting it before the agent is asked about it.
//!
//! Naming a file to an isolated agent has two halves that are easy to get
//! wrong separately. The path the operator knows is a *host* path, while the
//! agent only ever sees the destination its mount carries; and a path outside
//! every mount is a path the agent will simply report as missing, several
//! seconds and one wasted turn later. So the same key does both: it rewrites
//! what it inserts through the mount that carries it, and where nothing does,
//! it asks whether to mount it rather than letting the message go out naming
//! something the sandbox has never heard of.
//!
//! The state machine is the two questions in that order — which path, then on
//! what terms — and nothing else; the pure parts of it (resolving a path,
//! deciding whether a mount carries it) live in [`crate::mount`].

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::mount;
use styra_server::{DrivaOptions, LaunchMount};

/// The open path prompt, and which of its two questions is being answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Insert {
    /// Typing a path, with `Tab` completing it against the filesystem.
    Typing(String),
    /// The typed path resolved to a host path no mount carries. `host` is that
    /// path, canonical, waiting on the access to grant it with.
    Grant(PathBuf),
}

/// Open the prompt. Bound in the message editor, since what it produces is
/// message text: an operator who wanted to change the policy for its own sake
/// would be in the driva view instead.
pub fn open(app: &mut App) {
    app.insert = Some(Insert::Typing(String::new()));
}

pub fn cancel(app: &mut App) {
    app.insert = None;
}

/// The prompt is modal in both of its states — while typing, every printable
/// key is part of a path, and while granting, the single letter *is* the
/// answer — so the event loop routes keys here ahead of everything else.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match &app.insert {
        None => {}
        Some(Insert::Typing(_)) => match key.code {
            KeyCode::Esc => cancel(app),
            KeyCode::Enter => confirm(app),
            KeyCode::Tab => complete_typed(app),
            KeyCode::Backspace => {
                if let Some(Insert::Typing(text)) = app.insert.as_mut() {
                    text.pop();
                }
            }
            KeyCode::Char(ch) if !ch.is_control() => {
                if let Some(Insert::Typing(text)) = app.insert.as_mut() {
                    text.push(ch);
                }
            }
            _ => {}
        },
        Some(Insert::Grant(host)) => {
            let host = host.clone();
            match key.code {
                KeyCode::Char('r') => grant(app, host, false),
                KeyCode::Char('w') => grant(app, host, true),
                // Naming a path the sandbox cannot reach is a legitimate thing
                // to do — describing where a file *should* end up, or asking
                // about one the agent is expected to fail on — so the question
                // has an answer that grants nothing and still inserts.
                KeyCode::Char('n') => {
                    app.insert = None;
                    insert_path(app, &host);
                    app.show_action_message(format!(
                        "inserted {} — the sandbox cannot reach it",
                        host.display()
                    ));
                }
                KeyCode::Esc => cancel(app),
                _ => {}
            }
        }
    }
}

/// Where a relative path is relative to: the host directory backing the
/// agent's workspace, and failing that the directory this client was started
/// in. Deliberately not the interaction's working directory, which for a live
/// Codex session is a path *inside* the sandbox and would resolve against the
/// wrong filesystem.
fn base(app: &App) -> Option<PathBuf> {
    app.workspace_root
        .clone()
        .or_else(|| std::env::current_dir().ok())
}

/// Turn what was typed into a canonical host path.
///
/// Existence is required rather than assumed. A path that is not there cannot
/// be mounted — Driva canonicalizes a mount source and rejects one that does
/// not resolve — so accepting it here would only defer the same refusal to the
/// launch, by which time the message naming it has already gone.
pub fn resolve(base: Option<&Path>, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("give a path to insert".into());
    }
    let path = mount::expand_home(text);
    let path = match (path.is_absolute(), base) {
        (true, _) => path,
        (false, Some(base)) => base.join(path),
        (false, None) => return Err(format!("{} must be an absolute path", path.display())),
    };
    std::fs::canonicalize(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Accept the typed path: rewrite it into the agent's terms if a mount carries
/// it, and otherwise move on to the second question.
fn confirm(app: &mut App) {
    let Some(Insert::Typing(text)) = app.insert.clone() else {
        return;
    };
    // A path that does not resolve leaves the prompt open with the text still
    // in it, the way the mount prompt does: it is nearly always a typo one
    // keystroke from being right.
    let host = match resolve(base(app).as_deref(), &text) {
        Ok(host) => host,
        Err(problem) => return app.show_action_message(problem),
    };
    let Some(mounts) = app.launch.driva.as_ref().map(DrivaOptions::plain_mounts) else {
        // A replayed journal has no sandbox to describe and none planned, so
        // there is nothing to check the path against and nothing to grant.
        app.insert = None;
        insert_path(app, &host);
        return app.show_action_message("no sandbox policy known — inserted the path as it is");
    };
    match mount::visibility(&mounts, &host) {
        Some(visible) => {
            app.insert = None;
            let message = format!("{} ({})", visible.path.display(), visible.access);
            insert_path(app, &visible.path);
            app.show_action_message(message);
        }
        // Nothing carries it, and the policy is open to being changed, so ask.
        None if app.can_edit_launch() => app.insert = Some(Insert::Grant(host)),
        // Nothing carries it and nothing can, because a running sandbox's
        // mounts are fixed for its lifetime. Say so instead of offering a
        // grant that would be refused, and insert the path anyway — the
        // operator asked for it and may well mean it.
        None => {
            app.insert = None;
            insert_path(app, &host);
            app.show_action_message(
                "outside the sandbox — stop the interaction to mount it (S, then d)",
            );
        }
    }
}

/// Grant `host` to this interaction and insert it.
///
/// Always this interaction's own layer, never the Workspace's: the path came up
/// in one message, which is the smallest and shortest-lived claim available. `U`
/// in the driva view moves it up a layer once it turns out to be a property of
/// the work rather than of this conversation.
///
/// With no destination, Driva binds a source at its own name, so the path the
/// agent will use is the host path unchanged — which is what gets inserted.
fn grant(app: &mut App, host: PathBuf, writable: bool) {
    app.insert = None;
    let request = LaunchMount {
        source: host.clone(),
        destination: None,
        writable,
    };
    let label = mount::label(&request);
    let message = match app.launch.add_interaction_mount(request) {
        // The mount is a request, not a live change: nothing rebinds the
        // sandbox of an interaction that has already started, so the message
        // says when it will actually apply rather than only that it was added.
        Ok(()) => format!("added {label} — applies when this Session next launches"),
        Err(reason) => reason.to_owned(),
    };
    insert_path(app, &host);
    app.show_action_message(message);
}

/// Put `path` into the message, separated from whatever is already there.
fn insert_path(app: &mut App, path: &Path) {
    app.composer.insert(&path.display().to_string());
}

/// Extend the typed path as far as the filesystem leaves no choice.
fn complete_typed(app: &mut App) {
    let Some(Insert::Typing(text)) = app.insert.clone() else {
        return;
    };
    if let Some(completed) = complete(base(app).as_deref(), &text) {
        app.insert = Some(Insert::Typing(completed));
    }
}

/// The typed text extended by the longest prefix every candidate shares, or
/// `None` when that adds nothing.
///
/// The text before the last `/` is kept verbatim rather than rebuilt from the
/// resolved directory, so a `~/` the operator typed stays a `~/` on screen
/// instead of silently becoming their home directory spelled out. A single
/// directory match gains its trailing `/`, so the next `Tab` descends into it
/// without a keystroke in between.
pub fn complete(base: Option<&Path>, text: &str) -> Option<String> {
    let split = text.rfind('/').map(|index| index + 1).unwrap_or(0);
    let (typed_directory, prefix) = text.split_at(split);
    let directory = if typed_directory.is_empty() {
        base?.to_path_buf()
    } else {
        let directory = mount::expand_home(typed_directory);
        if directory.is_absolute() {
            directory
        } else {
            base?.join(directory)
        }
    };

    let mut matches: Vec<(String, bool)> = std::fs::read_dir(&directory)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.starts_with(prefix)
                .then(|| (name, entry.path().is_dir()))
        })
        .collect();
    matches.sort();
    let (first, _) = matches.first()?;
    let shared: String = first
        .char_indices()
        .take_while(|&(index, ch)| {
            matches
                .iter()
                .all(|(name, _)| name[index..].starts_with(ch))
        })
        .map(|(_, ch)| ch)
        .collect();

    let single_directory = matches.len() == 1 && matches[0].1;
    if shared.len() == prefix.len() && !single_directory {
        return None;
    }
    let mut completed = format!("{typed_directory}{shared}");
    if single_directory {
        completed.push('/');
    }
    (completed != text).then_some(completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Status;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use styra_server::{AttributedMount, Mount, MountAccess, MountOrigin};

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

    /// An app whose sandbox binds `root` at `/workspace` and nothing else, with
    /// nothing launched — so the launch policy is still open to being edited.
    fn app(root: &Path) -> App {
        let mut app = App::pending(styra_server::agent::Selection::parse("codex").unwrap());
        app.set_workspace_root(root.to_path_buf());
        app.launch.record(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into()],
            working_directory: PathBuf::from("/workspace"),
            network: false,
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: root.to_path_buf(),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
        });
        app.enter_input();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn typed(app: &mut App, text: &str) {
        open(app);
        for ch in text.chars() {
            press(app, KeyCode::Char(ch));
        }
        press(app, KeyCode::Enter);
    }

    /// A path the sandbox already carries goes straight into the message — and
    /// goes in as the agent's path, not the operator's, since those are the same
    /// file under two names and only one of them means anything to the agent.
    #[test]
    fn a_mounted_path_is_inserted_in_the_agents_terms() {
        let root = std::fs::canonicalize(tree("mounted")).unwrap();
        let mut app = app(&root);

        typed(&mut app, "reports/summary.md");

        assert_eq!(app.insert, None);
        assert_eq!(app.composer.text, "/workspace/reports/summary.md");
        assert!(app.launch.interaction.mounts.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path outside every mount stops for the second question, and answering
    /// it grants the path to this interaction and inserts it.
    #[test]
    fn an_unmounted_path_asks_before_it_is_inserted() {
        let root = std::fs::canonicalize(tree("unmounted")).unwrap();
        let outside = std::fs::canonicalize(tree("unmounted-elsewhere")).unwrap();
        let mut app = app(&root);

        typed(&mut app, &outside.join("notes.txt").display().to_string());

        assert_eq!(
            app.insert,
            Some(Insert::Grant(outside.join("notes.txt"))),
            "an unmounted path is not inserted until the question is answered"
        );
        assert!(app.composer.text.is_empty());

        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.insert, None);
        assert_eq!(
            app.launch.interaction.mounts,
            vec![LaunchMount {
                source: outside.join("notes.txt"),
                destination: None,
                writable: true,
            }]
        );
        // Bound at its own name, so what the agent will call it is what the
        // operator typed.
        assert_eq!(
            app.composer.text,
            outside.join("notes.txt").display().to_string()
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// `n` is the answer that grants nothing: the path still goes into the
    /// message, because naming a file the agent cannot open is a legitimate
    /// thing to do.
    #[test]
    fn declining_the_grant_still_inserts_the_path() {
        let root = std::fs::canonicalize(tree("declined")).unwrap();
        let outside = std::fs::canonicalize(tree("declined-elsewhere")).unwrap();
        let mut app = app(&root);

        typed(&mut app, &outside.join("notes.txt").display().to_string());
        press(&mut app, KeyCode::Char('n'));

        assert_eq!(app.insert, None);
        assert!(app.launch.interaction.mounts.is_empty());
        assert_eq!(
            app.composer.text,
            outside.join("notes.txt").display().to_string()
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A running sandbox's mounts are fixed for its lifetime, so there is no
    /// grant to offer. The path is inserted and the limit is said out loud,
    /// rather than a question being asked whose answers would all fail.
    #[test]
    fn a_running_interaction_is_told_rather_than_asked() {
        let root = std::fs::canonicalize(tree("running")).unwrap();
        let outside = std::fs::canonicalize(tree("running-elsewhere")).unwrap();
        let mut app = app(&root);
        app.status = Status::Running;

        typed(&mut app, &outside.join("notes.txt").display().to_string());

        assert_eq!(app.insert, None);
        assert!(app.launch.interaction.mounts.is_empty());
        assert_eq!(
            app.composer.text,
            outside.join("notes.txt").display().to_string()
        );
        assert!(app
            .action_messages
            .iter()
            .any(|message| message.text.contains("outside the sandbox")));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A path that is not there cannot be mounted, so the prompt says so and
    /// stays open on the text — which is usually one keystroke from right.
    #[test]
    fn a_path_that_does_not_exist_leaves_the_prompt_open() {
        let root = std::fs::canonicalize(tree("absent")).unwrap();
        let mut app = app(&root);

        typed(&mut app, "reprots/summary.md");

        assert_eq!(
            app.insert,
            Some(Insert::Typing("reprots/summary.md".into()))
        );
        assert!(app.composer.text.is_empty());

        // Tab completion is what fixes it, and Esc abandons it either way.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.insert, None);

        let _ = std::fs::remove_dir_all(&root);
    }
}

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
//!
//! [`Prompt`] holds what it needs to answer both questions — where relative
//! paths start, the sandbox it is checking against, and whether that sandbox
//! can still be changed — and reports what it decided as an [`Outcome`] rather
//! than reaching into a session. Both message boxes open it: the session view
//! from its own launch policy, and the live-interactions picker from the
//! summary of the interaction it is sending to.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use std::path::{Path, PathBuf};

use crate::mount;
use styra_server::{DrivaOptions, LaunchMount, Mount};

/// Which of the prompt's two questions is being answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Insert {
    /// Typing a path, with `Tab` completing it against the filesystem.
    Typing(String),
    /// The typed path resolved to a host path no mount carries. `host` is that
    /// path, canonical, waiting on the access to grant it with.
    Grant(PathBuf),
}

/// The open path prompt: its question, and what it is answering against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    state: Insert,
    /// Where a relative path is relative to; see [`Prompt::new`].
    base: Option<PathBuf>,
    /// The sandbox to check a path against, or `None` when there is no policy
    /// to check it against at all — a replayed journal has neither a sandbox
    /// nor one planned.
    mounts: Option<Vec<Mount>>,
    /// Whether a path outside the sandbox can still be granted. False once the
    /// sandbox is running, whose mounts are fixed for its lifetime.
    can_grant: bool,
}

/// What answering the prompt decided, for the message box that opened it to
/// carry out. Every variant that names a path also closes the prompt: the
/// path was decided, and what is left is putting it in the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still open, with nothing yet to do.
    Open,
    /// Closed with nothing to insert.
    Closed,
    /// Put `path` in the message. `notice` says how it was arrived at, when
    /// that is worth saying — the mount it came through, or the limit that
    /// stopped it from being granted.
    Insert {
        path: PathBuf,
        notice: Option<String>,
    },
    /// Add `mount` to this interaction's launch policy, then insert `path`.
    /// Bound at its own name, so the path the agent will use is the host path
    /// unchanged — which is what gets inserted.
    Grant { mount: LaunchMount, path: PathBuf },
    /// Something to tell the operator, with the prompt still open on it.
    Notice(String),
}

impl Prompt {
    /// Open the prompt. Bound in the message editor, since what it produces is
    /// message text: an operator who wanted to change the policy for its own
    /// sake would be in the driva view instead.
    ///
    /// `base` is where a relative path is relative to: the host directory
    /// backing the agent's workspace, and failing that the directory this
    /// client was started in. Deliberately not the interaction's working
    /// directory, which for a live Codex session is a path *inside* the
    /// sandbox and would resolve against the wrong filesystem.
    pub fn new(base: Option<PathBuf>, driva: Option<&DrivaOptions>, can_grant: bool) -> Self {
        Self {
            state: Insert::Typing(String::new()),
            base: base.or_else(|| std::env::current_dir().ok()),
            mounts: driva.map(DrivaOptions::plain_mounts),
            can_grant,
        }
    }

    /// Which question is open, for the renderer.
    pub fn state(&self) -> &Insert {
        &self.state
    }

    /// The prompt is modal in both of its states — while typing, every
    /// printable key is part of a path, and while granting, the single letter
    /// *is* the answer — so both message boxes route keys here ahead of
    /// everything else.
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match &mut self.state {
            Insert::Typing(text) => match key.code {
                KeyCode::Esc => Outcome::Closed,
                KeyCode::Enter => self.confirm(),
                KeyCode::Tab => {
                    if let Some(completed) = complete(self.base.as_deref(), text) {
                        *text = completed;
                    }
                    Outcome::Open
                }
                KeyCode::Backspace => {
                    text.pop();
                    Outcome::Open
                }
                KeyCode::Char(ch) if !ch.is_control() => {
                    text.push(ch);
                    Outcome::Open
                }
                _ => Outcome::Open,
            },
            Insert::Grant(host) => {
                let host = host.clone();
                match key.code {
                    KeyCode::Char('r') => self.grant(host, false),
                    KeyCode::Char('w') => self.grant(host, true),
                    // Naming a path the sandbox cannot reach is a legitimate
                    // thing to do — describing where a file *should* end up, or
                    // asking about one the agent is expected to fail on — so
                    // the question has an answer that grants nothing and still
                    // inserts.
                    KeyCode::Char('n') => Outcome::Insert {
                        notice: Some(format!(
                            "inserted {} — the sandbox cannot reach it",
                            host.display()
                        )),
                        path: host,
                    },
                    KeyCode::Esc => Outcome::Closed,
                    _ => Outcome::Open,
                }
            }
        }
    }

    /// Accept the typed path: rewrite it into the agent's terms if a mount
    /// carries it, and otherwise move on to the second question.
    fn confirm(&mut self) -> Outcome {
        let Insert::Typing(text) = &self.state else {
            return Outcome::Open;
        };
        // A path that does not resolve leaves the prompt open with the text
        // still in it, the way the mount prompt does: it is nearly always a
        // typo one keystroke from being right.
        let host = match resolve(self.base.as_deref(), text) {
            Ok(host) => host,
            Err(problem) => return Outcome::Notice(problem),
        };
        let Some(mounts) = &self.mounts else {
            return Outcome::Insert {
                path: host,
                notice: Some("no sandbox policy known — inserted the path as it is".into()),
            };
        };
        match mount::visibility(mounts, &host) {
            Some(visible) => Outcome::Insert {
                notice: Some(format!("{} ({})", visible.path.display(), visible.access)),
                path: visible.path,
            },
            // Nothing carries it, and the policy is open to being changed, so
            // ask.
            None if self.can_grant => {
                self.state = Insert::Grant(host);
                Outcome::Open
            }
            // Nothing carries it and nothing can, because a running sandbox's
            // mounts are fixed for its lifetime. Say so instead of offering a
            // grant that would be refused, and insert the path anyway — the
            // operator asked for it and may well mean it.
            None => Outcome::Insert {
                path: host,
                notice: Some(
                    "outside the sandbox — stop the interaction to mount it (S, then d)".into(),
                ),
            },
        }
    }

    /// Grant `host` to the interaction and insert it.
    ///
    /// Always the interaction's own layer, never the Workspace's: the path came
    /// up in one message, which is the smallest and shortest-lived claim
    /// available. `U` in the driva view moves it up a layer once it turns out
    /// to be a property of the work rather than of this conversation.
    fn grant(&mut self, host: PathBuf, writable: bool) -> Outcome {
        Outcome::Grant {
            mount: LaunchMount {
                source: host.clone(),
                destination: None,
                writable,
            },
            path: host,
        }
    }
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
    use crossterm::event::{KeyEvent, KeyModifiers};
    use styra_server::{AttributedMount, MountAccess, MountOrigin};

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
            mounts: vec![AttributedMount {
                origin: MountOrigin::Workspace,
                mount: Mount::Bind {
                    source: root.to_path_buf(),
                    destination: PathBuf::from("/workspace"),
                    access: MountAccess::ReadWrite,
                },
            }],
        }
    }

    /// A prompt over that sandbox, with the policy still open to being edited.
    fn prompt(root: &Path) -> Prompt {
        Prompt::new(Some(root.to_path_buf()), Some(&driva(root)), true)
    }

    fn press(prompt: &mut Prompt, code: KeyCode) -> Outcome {
        prompt.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Type `text` and accept it, returning what the prompt decided.
    fn typed(prompt: &mut Prompt, text: &str) -> Outcome {
        for ch in text.chars() {
            press(prompt, KeyCode::Char(ch));
        }
        press(prompt, KeyCode::Enter)
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
            press(&mut prompt, KeyCode::Char('w')),
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

    /// `n` is the answer that grants nothing: the path is still inserted,
    /// because naming a file the agent cannot open is a legitimate thing to do.
    #[test]
    fn declining_the_grant_still_inserts_the_path() {
        let root = std::fs::canonicalize(tree("declined")).unwrap();
        let outside = std::fs::canonicalize(tree("declined-elsewhere")).unwrap();
        let mut prompt = prompt(&root);
        let host = outside.join("notes.txt");

        typed(&mut prompt, &host.display().to_string());
        let outcome = press(&mut prompt, KeyCode::Char('n'));

        let Outcome::Insert { path, notice } = outcome else {
            panic!("expected an insert, got {outcome:?}");
        };
        assert_eq!(path, host);
        assert!(notice.unwrap().contains("cannot reach it"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A running sandbox's mounts are fixed for its lifetime, so there is no
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
        assert_eq!(press(&mut prompt, KeyCode::Esc), Outcome::Closed);

        let _ = std::fs::remove_dir_all(&root);
    }
}

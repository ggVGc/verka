use super::*;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use std::thread;
use std::time::Duration;

/// A server that answers nothing, and says what it was asked.
#[derive(Default)]
struct Unreachable {
    asked: std::sync::Mutex<Vec<styra_protocol::Request>>,
}

impl styra_server::InProcessServer for Unreachable {
    fn handle(&self, request: styra_protocol::Request) -> anyhow::Result<styra_protocol::Response> {
        self.asked.lock().unwrap().push(request);
        anyhow::bail!("this test's server answers nothing")
    }

    fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn navigator_keeps_focus_when_toggling_diff_and_switching_interactions() {
    struct DiffServer(Vec<styra_protocol::InteractionSummary>);
    impl styra_server::InProcessServer for DiffServer {
        fn handle(
            &self,
            request: styra_protocol::Request,
        ) -> anyhow::Result<styra_protocol::Response> {
            match request {
                styra_protocol::Request::DiffBase { id } => {
                    assert!(self.0.iter().any(|row| row.id == id));
                    if id == "no-snapshot" {
                        return Ok(styra_protocol::Response::DiffBase(None));
                    }
                    if id == "diff-error" {
                        anyhow::bail!("could not read diff baseline");
                    }
                    Ok(styra_protocol::Response::DiffBase(Some(
                        styra_protocol::DiffBase {
                            base: "HEAD".into(),
                            branch: None,
                        },
                    )))
                }
                styra_protocol::Request::LoadInteraction { id } => {
                    Ok(styra_protocol::Response::InteractionLoaded(
                        styra_protocol::LoadedInteraction {
                            summary: self.0.iter().find(|row| row.id == id).unwrap().clone(),
                            updates: styra_protocol::Updates {
                                updates: Vec::new(),
                                next: 0,
                            },
                            queued: Vec::new(),
                        },
                    ))
                }
                _ => anyhow::bail!("unexpected request"),
            }
        }

        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "s1",
    );
    app.enter_list();
    let mut interaction =
        crate::interactions::tests::interaction("s1", styra_protocol::InteractionActivity::Pending);
    let checkout = tempfile::tempdir().unwrap();
    interaction.checkout = Some(styra_protocol::CheckoutState {
        worktree: checkout.path().to_owned(),
        repository: checkout.path().to_owned(),
        branch: None,
    });
    let mut other = interaction.clone();
    other.id = "s2".into();
    let mut no_snapshot = interaction.clone();
    no_snapshot.id = "no-snapshot".into();
    let mut no_checkout = interaction.clone();
    no_checkout.id = "no-checkout".into();
    no_checkout.checkout = None;
    let mut diff_error = interaction.clone();
    diff_error.id = "diff-error".into();
    let interactions = vec![interaction, other, no_snapshot, no_checkout, diff_error];
    app.interactions.open(interactions.clone(), Vec::new());
    let mut terminal = crate::test_ui::TestUi::default();
    let client = Client::in_process(std::sync::Arc::new(DiffServer(interactions)));
    let mut live = Attachment::Detached;
    let mut history = InteractionHistory::default();
    let launch = LaunchPolicy::default();
    let preferences = checkout.path().join("preferences.toml");
    let mut event_loop = Loop::new(
        &mut terminal,
        &mut app,
        &client,
        &mut live,
        RunContext {
            standing_launch: &launch,
            preferences_path: &preferences,
            config: &crate::config::Defaults,
            history: &mut history,
        },
    )
    .unwrap();

    for visible in [true, false, true, false] {
        event_loop
            .handle_key(KeyCode::Char('d').into(), "")
            .unwrap();
        event_loop.carry_out_requests("").unwrap();
        assert_eq!(event_loop.app.diff_visible(), visible);
        assert!(event_loop.app.interactions.open);
        assert_eq!(event_loop.app.focus, crate::app::Focus::List);
        assert!(!event_loop.app.diff_focused());
        assert!(event_loop.app.take_request().is_none());
    }

    event_loop
        .handle_key(KeyCode::Char('d').into(), "")
        .unwrap();
    event_loop.carry_out_requests("").unwrap();
    for id in ["s2", "s1"] {
        let key = if id == "s2" {
            KeyCode::Char('j')
        } else {
            KeyCode::Char('k')
        };
        event_loop.handle_key(key.into(), "").unwrap();
        assert_eq!(
            event_loop
                .app
                .interactions
                .cursor(&event_loop.app.session_id),
            id
        );
        navigation::load_cursored_interaction(
            event_loop.app,
            event_loop.live,
            event_loop.client,
            event_loop.standing_launch,
        );
        assert_eq!(event_loop.app.session_id, id);
        assert!(event_loop.app.diff_interaction_focus);
        event_loop.carry_out_requests("").unwrap();
        assert!(event_loop.app.diff_visible());
        assert!(event_loop.app.interactions.open);
        assert_eq!(event_loop.app.focus, crate::app::Focus::List);
        assert!(!event_loop.app.diff_focused());
    }

    // Missing baselines, checkouts and server failures must not disable
    // the pane for the next interaction with a diff.
    for id in ["no-snapshot", "no-checkout", "diff-error", "s1"] {
        let current = event_loop.app.session_id.clone();
        event_loop
            .app
            .interactions
            .move_cursor_to(id.into(), &current);
        navigation::load_cursored_interaction(
            event_loop.app,
            event_loop.live,
            event_loop.client,
            event_loop.standing_launch,
        );
        event_loop.carry_out_requests("").unwrap();
        assert_eq!(event_loop.app.session_id, id);
        assert!(event_loop.app.diff_visible(), "{id}");
        assert_eq!(event_loop.app.checkout_diff.is_some(), id == "s1");
        assert!(event_loop.app.interactions.open);
        assert!(!event_loop.app.diff_focused());
    }

    // An explicit toggle still disables the pane on later rows. It can
    // also be enabled on a row without a checkout for the next valid row.
    for (id, toggle, visible, loaded) in [
        ("no-checkout", true, false, false),
        ("s1", false, false, false),
        ("no-checkout", true, true, false),
        ("s1", false, true, true),
    ] {
        let current = event_loop.app.session_id.clone();
        event_loop
            .app
            .interactions
            .move_cursor_to(id.into(), &current);
        navigation::load_cursored_interaction(
            event_loop.app,
            event_loop.live,
            event_loop.client,
            event_loop.standing_launch,
        );
        event_loop.carry_out_requests("").unwrap();
        if toggle {
            event_loop
                .handle_key(KeyCode::Char('d').into(), "")
                .unwrap();
            event_loop.carry_out_requests("").unwrap();
        }
        assert_eq!(event_loop.app.diff_visible(), visible, "{id}");
        assert_eq!(event_loop.app.checkout_diff.is_some(), loaded, "{id}");
        assert!(event_loop.app.interactions.open);
        assert!(!event_loop.app.diff_focused());
    }

    // With the navigator closed, as `n` steps, whichever side had the
    // keys keeps them on the next interaction.
    event_loop.app.interactions.close();
    for interaction_focus in [true, false] {
        event_loop.app.diff_interaction_focus = interaction_focus;
        for id in ["s2", "s1"] {
            let current = event_loop.app.session_id.clone();
            event_loop
                .app
                .interactions
                .move_cursor_to(id.into(), &current);
            navigation::load_cursored_interaction(
                event_loop.app,
                event_loop.live,
                event_loop.client,
                event_loop.standing_launch,
            );
            event_loop.carry_out_requests("").unwrap();
            assert_eq!(event_loop.app.session_id, id);
            assert!(event_loop.app.diff_visible(), "{id}");
            assert!(!event_loop.app.interactions.open);
            assert_eq!(event_loop.app.diff_focused(), !interaction_focus, "{id}");
        }
    }
}

#[test]
fn only_the_navigator_close_keys_dismiss_it() {
    use crossterm::event::KeyModifiers;
    for (key, stays_open) in [
        (KeyCode::Char('b').into(), true),
        (KeyCode::F(12).into(), true),
        (KeyCode::Left.into(), true),
        (KeyCode::Down.into(), true),
        (KeyCode::Char('j').into(), true),
        // Rename opens its prompt over the navigator rather than
        // dismissing it.
        (
            crossterm::event::KeyEvent::new(KeyCode::Char('r'), KeyModifiers::ALT),
            true,
        ),
        (KeyCode::Enter.into(), false),
        (KeyCode::Esc.into(), false),
        (KeyCode::Char('a').into(), false),
    ] {
        let key: crossterm::event::KeyEvent = key;
        let code = key.code;
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_list();
        app.interactions.show(Vec::new());
        let mut terminal = crate::test_ui::TestUi::default();
        terminal.script([Event::Key(key)]);
        let mut history = InteractionHistory::default();
        history.record("previous");
        let result = run(
            &mut terminal,
            &mut app,
            &Client::in_process(std::sync::Arc::new(Unreachable::default())),
            &mut Attachment::Detached,
            RunContext {
                standing_launch: &LaunchPolicy::default(),
                preferences_path: &std::env::temp_dir()
                    .join("styra-event-loop-no-preferences.toml"),
                config: &crate::config::Defaults,
                history: &mut history,
            },
        );
        assert!(result.is_err(), "the input script should be exhausted");
        assert_eq!(app.interactions.open, stays_open, "{code:?}");
        // Only Enter records the selected row; closing or browsing does not.
        history.arrive(0);
        assert_eq!(
            history.target("previous", true).map(|(_, id)| id),
            (code == KeyCode::Enter).then_some("s1"),
            "{code:?}"
        );
    }
}

#[test]
fn history_picker_returns_a_visit_without_rewriting_history() {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "s1",
    );
    app.enter_list();
    let mut history = InteractionHistory::default();
    history.record("earlier");
    history.record("s1");
    let mut terminal = crate::test_ui::TestUi::default();
    terminal.script([
        Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::ALT,
        )),
        Event::Key(KeyCode::Char('e').into()),
        Event::Key(KeyCode::Enter.into()),
    ]);
    let result = run(
        &mut terminal,
        &mut app,
        &Client::in_process(std::sync::Arc::new(Unreachable::default())),
        &mut Attachment::Detached,
        RunContext {
            standing_launch: &LaunchPolicy::default(),
            preferences_path: &std::env::temp_dir()
                .join("styra-history-picker-no-preferences.toml"),
            config: &crate::config::Defaults,
            history: &mut history,
        },
    );
    assert!(matches!(result, Ok(RunOutcome::JumpTo(0))));
    assert_eq!(history.entries(), ["earlier", "s1"]);
    assert_eq!(history.target("s1", false), Some((0, "earlier")));
}

/// Run the loop over a diff pane of two files, pressing `keys`.
/// The loop ends with what the keys asked for or, once the script runs
/// out, with an error.
fn run_over_diff(keys: &str, server: std::sync::Arc<Unreachable>) -> Result<RunOutcome> {
    let mut app = App::new(
        styra_protocol::agent::Selection::parse("codex").unwrap(),
        "s1",
    );
    app.enter_list();
    app.diff_preview = true;
    let mut diff = styra_diff::CheckoutDiff::read(styra_diff::DiffTarget {
        worktree: std::env::temp_dir().join("styra-event-loop-no-such-checkout"),
        branch: None,
        base_branch: None,
        base: "4bf5c35d".into(),
        branch_point: true,
    });
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n".into());
    app.checkout_diff = Some(diff);
    assert!(app.diff_focused());
    let mut terminal = crate::test_ui::TestUi::default();
    terminal.script(
        keys.chars()
            .map(|key| Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(key)))),
    );
    let preferences = std::env::temp_dir().join("styra-event-loop-no-preferences.toml");
    run(
        &mut terminal,
        &mut app,
        &Client::in_process(server),
        &mut Attachment::Detached,
        RunContext {
            standing_launch: &LaunchPolicy::default(),
            preferences_path: &preferences,
            config: &crate::config::Defaults,
            history: &mut InteractionHistory::default(),
        },
    )
}

/// The diff owns the keyboard, and its key handler never falls through
/// to the rest of the round; what its keys ask for is still done on the
/// key, not left queued behind keys that never come.
#[test]
fn what_a_diff_key_asks_for_is_done_on_that_key() {
    let quit = run_over_diff("q", Default::default());
    assert!(
        matches!(quit, Ok(RunOutcome::Quit)),
        "q quits from the diff"
    );

    let server = std::sync::Arc::new(Unreachable::default());
    assert!(matches!(
        run_over_diff("Eq", server.clone()),
        Ok(RunOutcome::Quit)
    ));
    // The report is asked for off the loop's thread.
    let asked = || {
        server.asked.lock().unwrap().iter().any(|request| {
            matches!(
                request,
                styra_protocol::Request::GenerateEditReport { id, files, .. }
                    if id == "s1" && files.len() == 1
            )
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !asked() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(asked(), "E asked the server for an edit report");
}

/// A screen with no interaction yet diffs the checkout it will start in
/// against a local branch the operator picks, read on this host, and `i`
/// from that diff goes to the message box that starts the interaction.
#[test]
fn a_new_interaction_diffs_its_checkout_against_a_chosen_branch() {
    let checkout = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout.path())
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(checkout.path().join("a.txt"), "a\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "--quiet", "-m", "first"]);
    git(&["switch", "--quiet", "-c", "topic"]);
    std::fs::write(checkout.path().join("b.txt"), "b\n").unwrap();
    git(&["add", "b.txt"]);
    git(&["commit", "--quiet", "-m", "second"]);
    std::fs::write(checkout.path().join("c.txt"), "c\n").unwrap();

    let mut app = App::pending(styra_protocol::agent::Selection::parse("codex").unwrap());
    app.workspace.enter(checkout.path().to_owned());
    let mut terminal = crate::test_ui::TestUi::default();
    let server = std::sync::Arc::new(Unreachable::default());
    let client = Client::in_process(server.clone());
    let mut live = Attachment::Detached;
    let mut history = InteractionHistory::default();
    let launch = LaunchPolicy::default();
    let preferences = checkout.path().join("preferences.toml");
    let mut event_loop = Loop::new(
        &mut terminal,
        &mut app,
        &client,
        &mut live,
        RunContext {
            standing_launch: &launch,
            preferences_path: &preferences,
            config: &crate::config::Defaults,
            history: &mut history,
        },
    )
    .unwrap();
    let press = |event_loop: &mut Loop, key: KeyCode| {
        event_loop.handle_key(key.into(), "").unwrap();
        event_loop.carry_out_requests("").unwrap();
    };

    press(&mut event_loop, KeyCode::Char('d'));
    let picker = event_loop.app.diff_branch_picker.as_ref().unwrap();
    let mut branches = picker.rows[1..].to_vec();
    branches.sort();
    assert_eq!(branches, ["main", "topic"]);
    assert!(!event_loop.app.diff_visible());
    for key in "main".chars() {
        press(&mut event_loop, KeyCode::Char(key));
    }
    press(&mut event_loop, KeyCode::Enter);
    assert!(event_loop.app.diff_branch_picker.is_none());
    assert!(event_loop.app.diff_focused());
    let diff = event_loop.app.checkout_diff.as_ref().unwrap();
    assert_eq!(diff.target().unwrap().base_branch.as_deref(), Some("main"));
    let text = diff.diff.as_ref().unwrap();
    assert!(
        text.contains("b/b.txt") && text.contains("b/c.txt"),
        "{text}"
    );
    assert!(!text.contains("a.txt"), "{text}");

    // `b` offers the branches again; the first row is the head alone.
    press(&mut event_loop, KeyCode::Char('b'));
    press(&mut event_loop, KeyCode::Enter);
    let diff = event_loop.app.checkout_diff.as_ref().unwrap();
    let text = diff.diff.as_ref().unwrap();
    assert!(
        text.contains("b/c.txt") && !text.contains("b.txt"),
        "{text}"
    );

    // Closed and opened again, the same comparison is read once more.
    press(&mut event_loop, KeyCode::Char('d'));
    assert!(!event_loop.app.diff_visible());
    press(&mut event_loop, KeyCode::Char('d'));
    assert!(event_loop.app.diff_branch_picker.is_none());
    assert!(event_loop.app.diff_focused());

    press(&mut event_loop, KeyCode::Char('i'));
    assert_eq!(event_loop.app.focus, crate::app::Focus::Input);
    assert!(server.asked.lock().unwrap().is_empty());
}

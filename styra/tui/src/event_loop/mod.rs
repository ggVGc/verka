//! The interactive loop. Each round carries out what the last one asked for
//! ([`requests`]), catches up with the server ([`sync`]), paints the frame
//! if anything changed and waits for the next key ([`screen`]), and hands
//! that key to whichever window owns it ([`keys`]).

use anyhow::Result;
use crossterm::event::KeyEvent;
use std::collections::VecDeque;
use std::path::Path;
use std::time::Instant;

use crate::app::{App, LaunchPolicy};
use crate::audio::AudioInput;
use crate::config::Configuration;
use crate::edit_report::EditReports;
use crate::interaction_history::InteractionHistory;
use crate::preferences;
use crate::presentation;
use crate::session::{self, Attachment};
use styra_protocol::WorkspaceSummary;
use styra_server::Client;
use styra_ui::Ui;

mod keys;
mod navigation;
mod requests;
mod screen;
mod sync;
mod tags;
mod workers;

pub use navigation::open_interaction_navigator;
pub use sync::refresh_quota;

use workers::{InteractionsPoll, LaunchEffects};

/// What the interactive loop returned control to `main` for.
pub enum RunOutcome {
    Quit,
    OpenWorkspace {
        workspace: Box<WorkspaceSummary>,
        session_id: Option<String>,
        /// Land in the Workspace with the live Interactions navigator open,
        /// as entering one that has work in flight does.
        open_interactions: bool,
    },
    OpenSession(String),
    OpenBranch(String),
    /// Move backward through explicitly entered interactions.
    JumpBack,
    /// Move forward through explicitly entered interactions.
    JumpForward,
    /// Jump to an existing visit without rewriting the history.
    JumpTo(usize),
    Reset,
    NewSession,
}

pub struct RunContext<'a> {
    pub history: &'a mut InteractionHistory,
    pub standing_launch: &'a LaunchPolicy,
    pub preferences_path: &'a Path,
    pub config: &'a dyn Configuration,
}

/// Return the running interaction an in-client transition explicitly stops.
pub fn stops_current_interaction(outcome: &RunOutcome, live: &Attachment) -> bool {
    matches!(
        (outcome, live),
        (RunOutcome::Reset, Attachment::Attached { .. })
    )
}

/// The event loop: apply pending session updates, render, and handle input
/// until the operator quits or asks to switch sessions.
pub fn run(
    terminal: &mut dyn Ui,
    app: &mut App,
    client: &Client,
    live: &mut Attachment,
    context: RunContext<'_>,
) -> Result<RunOutcome> {
    tracing::debug!(
        target: "styra_tui::event_loop",
        pid = std::process::id(),
        session_id = %app.session_id,
        "entered event loop"
    );
    let outcome = Loop::new(terminal, app, client, live, context).and_then(Loop::run);
    // Every way out of the loop is a screen about to be replaced or a client
    // about to close, so what was typed on this one is stored first.
    session::save_composer(app, client, true);
    outcome
}

/// Everything one run of the loop holds between rounds.
struct Loop<'a> {
    terminal: &'a mut dyn Ui,
    app: &'a mut App,
    client: &'a Client,
    live: &'a mut Attachment,
    history: &'a mut InteractionHistory,
    standing_launch: &'a LaunchPolicy,
    preferences_path: &'a Path,
    config: &'a dyn Configuration,
    launch_effects: LaunchEffects,
    interactions_poll: InteractionsPoll,
    edit_reports: EditReports,
    audio: AudioInput,
    pending_fold: bool,
    /// Keys the action menu chose, still to be handled as if they were typed.
    replay: VecDeque<KeyEvent>,
    interactions_refreshed: Instant,
    quota_refreshed: Instant,
    processes_refreshed: Instant,
    diff_refreshed: Instant,
    ticked: Instant,
    /// What the clock read on the frame currently on screen, so that a reading
    /// which has not changed since cannot ask for another one.
    clock: Option<String>,
    /// Whether anything has happened since the last frame was painted. The
    /// round is a poll, not an event: it comes back every
    /// [`KEY_POLL`](screen::KEY_POLL) whether or not the server, the terminal
    /// or the operator had anything to say, and almost always they did not.
    /// Every step that can change what is on screen reports whether it did,
    /// and the frame is painted only if one of them did — so a Styra nobody
    /// is using stops drawing rather than repainting the same screen ten times
    /// a second.
    ///
    /// It starts set: the first round has a blank terminal to fill.
    dirty: bool,
    bracketed_paste: bool,
}

impl<'a> Loop<'a> {
    fn new(
        terminal: &'a mut dyn Ui,
        app: &'a mut App,
        client: &'a Client,
        live: &'a mut Attachment,
        context: RunContext<'a>,
    ) -> Result<Self> {
        let RunContext {
            standing_launch,
            preferences_path,
            config,
            history,
        } = context;
        // The model column's ordering is remembered across runs, so pick it up
        // before the picker can be opened.
        app.recent_models = preferences::load_recent_models(preferences_path);
        terminal.set_bracketed_paste(false)?;
        let now = Instant::now();
        Ok(Self {
            clock: presentation::clock_reading(app),
            terminal,
            app,
            client,
            live,
            history,
            standing_launch,
            preferences_path,
            config,
            launch_effects: LaunchEffects::new(client.clone()),
            interactions_poll: InteractionsPoll::new(client.clone()),
            edit_reports: EditReports::new(),
            audio: AudioInput::new(),
            pending_fold: false,
            replay: VecDeque::new(),
            interactions_refreshed: now,
            quota_refreshed: now,
            processes_refreshed: now,
            diff_refreshed: now,
            ticked: now,
            dirty: true,
            bracketed_paste: false,
        })
    }

    fn run(mut self) -> Result<RunOutcome> {
        loop {
            if let Some(outcome) = self.round()? {
                return Ok(outcome);
            }
        }
    }

    /// One round: requests, server, frame, key. Returns the outcome that ends
    /// the loop, if this round reached one.
    fn round(&mut self) -> Result<Option<RunOutcome>> {
        session::save_composer(self.app, self.client, false);
        let workspace_id = self.app.workspace.id.clone().unwrap_or_default();
        if let Some(outcome) = self.carry_out_requests(&workspace_id)? {
            return Ok(Some(outcome));
        }
        self.sync(&workspace_id)?;
        self.tick_clock();
        self.draw()?;
        let Some(key) = self.next_key()? else {
            return Ok(None);
        };
        // Every key is answered on screen — by what it does, or by the notice
        // saying why it did nothing — and the handlers are spread across a
        // dozen modal windows. Marking the frame here, once, is what keeps
        // that from being a promise each of them has to remember to keep.
        self.dirty = true;
        self.handle_key(key, &workspace_id)
    }

    /// Load the interaction the list or grid cursor has moved onto; see
    /// [`navigation::load_cursored_interaction`].
    fn load_cursored_interaction(&mut self) {
        navigation::load_cursored_interaction(
            self.app,
            self.live,
            self.client,
            self.standing_launch,
        );
    }

    /// Make `interaction` current; see
    /// [`navigation::make_interaction_current`].
    fn make_interaction_current(&mut self, interaction: styra_protocol::InteractionSummary) {
        navigation::make_interaction_current(
            self.app,
            self.live,
            self.client,
            self.standing_launch,
            interaction,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
    use crossterm::event::{Event, KeyCode, KeyModifiers};
    use std::thread;
    use std::time::Duration;

    /// A server that answers nothing, and says what it was asked.
    #[derive(Default)]
    struct Unreachable {
        asked: std::sync::Mutex<Vec<styra_protocol::Request>>,
    }

    impl styra_server::InProcessServer for Unreachable {
        fn handle(
            &self,
            request: styra_protocol::Request,
        ) -> anyhow::Result<styra_protocol::Response> {
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
        let mut interaction = crate::interactions::tests::interaction(
            "s1",
            styra_protocol::InteractionActivity::Pending,
        );
        let checkout = tempfile::tempdir().unwrap();
        interaction.checkout = Some(styra_protocol::CheckoutState {
            worktree: checkout.path().to_owned(),
            repository: checkout.path().to_owned(),
            branch: None,
        });
        let mut other = interaction.clone();
        other.id = "s2".into();
        let interactions = vec![interaction, other];
        app.interactions.open(interactions.clone(), Vec::new());
        let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
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
                KeyCode::Down
            } else {
                KeyCode::Up
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
    }

    #[test]
    fn only_mapped_keys_dismiss_the_interaction_navigator() {
        for (code, stays_open) in [
            (KeyCode::Char('b'), true),
            (KeyCode::F(12), true),
            (KeyCode::Left, true),
            (KeyCode::Down, true),
            (KeyCode::Char('j'), true),
            (KeyCode::Enter, false),
            (KeyCode::Esc, false),
            (KeyCode::Char('a'), false),
        ] {
            let mut app = App::new(
                styra_protocol::agent::Selection::parse("codex").unwrap(),
                "s1",
            );
            app.enter_list();
            app.interactions.show(Vec::new());
            let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
            terminal.script([Event::Key(crossterm::event::KeyEvent::from(code))]);
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
        let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
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

    /// Run the loop over a fullscreen diff of two files, pressing `keys`.
    /// The loop ends with what the keys asked for or, once the script runs
    /// out, with an error.
    fn run_over_diff(keys: &str, server: std::sync::Arc<Unreachable>) -> Result<RunOutcome> {
        let mut app = App::new(
            styra_protocol::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.enter_list();
        app.view = View::CheckoutDiff;
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
        let mut terminal = styra_ui::TestUi::new(100, 30).unwrap();
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
}

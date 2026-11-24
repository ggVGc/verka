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

#[cfg(feature = "renderer")]
pub use navigation::open_interaction_navigator;
#[cfg(feature = "renderer")]
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
mod tests;

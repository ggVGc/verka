//! Catching up with the server once a round: the attached session's updates,
//! the fleet snapshot, the timers, and the work the loop does on the
//! interaction's behalf when it goes idle.

use anyhow::Result;
use std::time::{Duration, Instant};

use super::navigation::cursored_interaction_is_loadable;
use super::screen::blocked_on;
use super::Loop;
use crate::activity::Status;
use crate::app::App;
use crate::session::{self, Attachment};
use styra_protocol::LogEntry;
use styra_server::Client;

pub(super) const INTERACTIONS_REFRESH: Duration = Duration::from_millis(250);

/// Quota readings change on the scale of whole interactions, not keystrokes,
/// so this is slow enough to cost the server nothing and quick enough that the
/// footer's warning is never meaningfully behind the account.
const QUOTA_REFRESH: Duration = Duration::from_secs(5);
/// How often the open processes view lists the sandbox again.
const PROCESSES_REFRESH: Duration = Duration::from_secs(1);

/// How often the in-window diff is read again while the agent is working on
/// the checkout it shows.
const DIFF_REFRESH: Duration = Duration::from_secs(5);

impl Loop<'_> {
    pub(super) fn sync(&mut self, workspace_id: &str) -> Result<()> {
        let app = &mut *self.app;
        self.dirty |= app.notices.expire();
        self.dirty |= app.interactions.expire_flashes();
        self.dirty |= self.launch_effects.apply_ready(app, workspace_id);
        self.dirty |= self.edit_reports.apply_ready(app);
        self.dirty |= self.audio.apply_ready(app, self.client);
        // Workspace launch policy is a server-owned read model. Refresh it
        // independently of input so edits from another Styra client flow into
        // this Driva view and invalidate its planned options.
        if let Ok(policy) = self.client.workspace_launch(workspace_id) {
            if policy != app.launch.workspace {
                app.launch.sync_workspace(policy);
                self.dirty = true;
            }
        }
        self.dirty |= session::ensure_driva_plan(app, self.client, workspace_id);
        self.poll_updates();
        self.poll_fleet();
        self.poll_timers();

        // A cursor that has come to rest loads the interaction under it.
        // Until then, the screen below still shows the previous interaction.
        // A live tag-filter edit can move the navigator cursor when the row
        // under it stops matching. Do not load that row while the picker is
        // still open: loading replaces the whole App and would make Space
        // appear to close the modal. The settled cursor is loaded on the
        // first round after the picker closes instead.
        if cursored_interaction_is_loadable(self.app) {
            self.load_cursored_interaction();
            self.dirty = true;
        }

        self.restart_for_mounts()?;
        self.send_queued();

        let app = &mut *self.app;
        self.dirty |= crate::branch_log::sync(app, self.client);
        self.dirty |= crate::turn_snapshot::sync(app, self.client);
        self.dirty |= app.sync_turn_diff();
        self.dirty |= app.activity.note_progress();
        Ok(())
    }

    /// Apply what the attached session has done since the last round. A
    /// server that cannot say detaches the screen from it.
    fn poll_updates(&mut self) {
        let Attachment::Attached { cursor } = self.live else {
            return;
        };
        match self.client.updates(&self.app.session_id, *cursor) {
            Ok(batch) => {
                *cursor = batch.next;
                self.dirty |= !batch.updates.is_empty();
                for sequenced in batch.updates {
                    session::apply_update(self.app, sequenced.update);
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "styra_tui::rpc",
                    pid = std::process::id(),
                    session_id = %self.app.session_id,
                    error = %error,
                    "update poll failed"
                );
                self.app
                    .push_log(LogEntry::error(format!("update poll failed: {error:#}")));
                self.app.on_ended(styra_protocol::InteractionEnd {
                    exit_code: None,
                    error: Some(error.to_string()),
                });
                *self.live = Attachment::Detached;
                self.dirty = true;
            }
        }
    }

    /// Keep the fleet snapshot fresh even with the navigator closed: it is
    /// what lets the footer report a different interaction becoming idle while
    /// the operator is reading or working in this one. A rebase or integration
    /// in progress is followed on the same period.
    fn poll_fleet(&mut self) {
        if self.interactions_refreshed.elapsed() >= INTERACTIONS_REFRESH {
            self.interactions_refreshed = Instant::now();
            self.dirty |= poll_rebase(self.app, self.client);
            self.dirty |= poll_integration(self.app, self.client);
            self.interactions_poll
                .request(self.app.interactions_query());
        }
        if let Some(interactions) = self.interactions_poll.ready() {
            self.dirty |= self.app.interactions.refresh(interactions);
            self.dirty |= self.app.adopt_integration();
        }
        // An editor started work in the Workspace on screen and asked for it
        // to be watched. A blank screen is left alone: it is the operator
        // composing work of their own, not watching any.
        if let Some(claimed) = self.app.interactions.take_focus_claim() {
            if !self.app.session_id.is_empty()
                && self.app.workspace.id.as_deref() == Some(claimed.workspace_id.as_str())
            {
                self.make_interaction_current(claimed);
                self.dirty = true;
            }
        }
    }

    /// The readings that are only worth taking every so often.
    fn poll_timers(&mut self) {
        if self.quota_refreshed.elapsed() >= QUOTA_REFRESH {
            self.quota_refreshed = Instant::now();
            self.dirty |= poll_quota(self.app, self.client);
        }

        if self.app.processes_visible() && self.processes_refreshed.elapsed() >= PROCESSES_REFRESH {
            self.processes_refreshed = Instant::now();
            self.dirty |= refresh_processes(self.app, self.client);
        }

        // The diff on screen is a reading of a checkout the agent may still be
        // writing to, so it is read again while the agent works. An idle or
        // stopped agent leaves the checkout alone: reopening refreshes it then.
        if self.app.diff_visible()
            && matches!(
                self.app.activity.status,
                Status::Running | Status::Background
            )
            && self.diff_refreshed.elapsed() >= DIFF_REFRESH
        {
            self.diff_refreshed = Instant::now();
            if let Some(diff) = self.app.checkout_diff.as_mut() {
                self.dirty |= diff.refresh();
            }
        }
    }

    /// A mount changed while the agent sat idle. Restarted here rather than
    /// where the key was pressed, so a Workspace mount is in the server's
    /// policy before the resume merges it, and ahead of the queued send, so a
    /// waiting message is answered under the new mounts.
    fn restart_for_mounts(&mut self) -> Result<()> {
        if !self.app.restart_for_mounts || self.app.workspace_launch_pending != 0 {
            return Ok(());
        }
        self.app.restart_for_mounts = false;
        self.dirty = true;
        let Attachment::Attached { .. } = self.live else {
            return Ok(());
        };
        if !self.app.activity.status.is_idle() {
            // A turn started before the restart could: leave it be.
            self.app.show_action_message(
                "the agent is working — the new mounts apply when this Session next launches",
            );
            return Ok(());
        }
        let client = self.client;
        let live = &mut *self.live;
        let restarted = blocked_on(
            self.terminal,
            self.app,
            "restarting the interaction with the new mounts…",
            |app| session::restart_for_mounts(app, client, live),
        )?;
        let app = &mut *self.app;
        match restarted {
            Ok(()) => app.show_action_message("restarted with the new mounts"),
            // Logged as well as shown: the notice is one line, cut to the
            // terminal's width, and the cause is the part at the end of it.
            Err(session::MountRestartError::Rejected(error)) => {
                app.push_log(LogEntry::error(format!(
                    "the new mounts cannot be launched, so the interaction was not restarted: {error:#}"
                )));
                app.show_error(format!(
                    "mounts not applied, still running on the old ones: {error:#}"
                ));
            }
            Err(session::MountRestartError::Failed(error)) => {
                app.push_log(LogEntry::error(format!(
                    "could not restart with the new mounts: {error:#}"
                )));
                app.show_error(format!(
                    "could not restart with the new mounts ({error:#}); they apply when this Session next launches"
                ));
            }
        }
        Ok(())
    }

    /// Send the next queued message once the agent has gone idle.
    fn send_queued(&mut self) {
        let Attachment::Attached { .. } = self.live else {
            return;
        };
        let app = &mut *self.app;
        if !app.activity.status.is_idle() || app.outbox.queued_count() == 0 {
            return;
        }
        self.dirty = true;
        match self.client.send_queued_message(&app.session_id) {
            Ok((Some(_), queued)) => {
                app.outbox.replace_queued(queued);
                app.activity.status = Status::Running;
                let waiting = app.outbox.queued_count();
                app.show_action_message(if waiting == 0 {
                    "sent queued message automatically".into()
                } else {
                    format!("sent queued message automatically ({waiting} still waiting)")
                });
            }
            Ok((None, queued)) => app.outbox.replace_queued(queued),
            Err(error) => {
                app.push_log(LogEntry::error(format!("queued send failed: {error:#}")));
            }
        }
    }
}

/// Follow a rebase this client started. Reports whether there was one.
fn poll_rebase(app: &mut App, client: &Client) -> bool {
    let Some(id) = app.pending_rebase.clone() else {
        return false;
    };
    match client.session_rebase_status(&id) {
        Ok(status) => {
            if status.finished {
                app.pending_rebase = None;
            }
            app.rebase_status = Some(status);
        }
        Err(error) => {
            app.pending_rebase = None;
            if let Some(status) = app.rebase_status.as_mut() {
                status.finished = true;
                status.succeeded = false;
                status.message = format!("Could not read rebase progress: {error:#}");
            }
        }
    }
    true
}

/// Report an integration this client started, once it has a result.
/// Reports whether anything was shown.
fn poll_integration(app: &mut App, client: &Client) -> bool {
    let Some(id) = app.pending_integration.clone() else {
        return false;
    };
    match client.session_integration_result(&id) {
        Ok(Some(result)) => {
            app.pending_integration = None;
            if result.completed {
                app.show_action_message(result.message);
            } else {
                app.show_error(result.message);
            }
            true
        }
        Err(error) => {
            app.pending_integration = None;
            app.show_error(format!("could not read integration result: {error:#}"));
            true
        }
        Ok(None) => false,
    }
}

/// Fill the quota view from the server's log, which is where the readings live:
/// the server reads the figures off every interaction's wire, so one client
/// asking gets every session's readings rather than only this one's.
///
/// Called both when `Q` asks for the log and once at startup, so the footer's
/// warning is right from the first frame. A provider that is already nearly out
/// says so hours before it next volunteers a reading, and an operator who never
/// presses `Q` would otherwise not be told at all.
///
/// Best-effort: a server that cannot answer leaves the readings as they were
/// and logs why, rather than failing the startup that asked.
pub fn refresh_quota(app: &mut App, client: &Client) {
    match client.quota_log() {
        Ok(readings) => app.quota.replace(readings),
        Err(error) => app.push_log(LogEntry::error(format!(
            "could not read the quota log: {error:#}"
        ))),
    }
}

/// List the processes in the current interaction's sandbox, for the
/// processes view. Reports whether the listing on screen changed.
///
/// A blank screen has no interaction to ask about; a failure is shown in the
/// view itself rather than logged, since the view asks every second.
pub(super) fn refresh_processes(app: &mut App, client: &Client) -> bool {
    let answer = if app.session_id.is_empty() {
        Ok(None)
    } else {
        client
            .interaction_processes(&app.session_id)
            .map_err(|error| format!("{error:#}"))
    };
    let session = app.session_id.clone();
    app.processes.replace(&session, answer)
}

/// Take a fresh reading of the quota log on the loop's timer.
///
/// The attached session's own readings arrive on its update stream, but
/// readings belong to the account: another session burning the plan down shows
/// up here or not at all. Doing it on the timer rather than only on `Q` is what
/// keeps the footer's warning true for an operator who never opens the view —
/// and keeps the view itself moving while it is open.
///
/// Silent on failure, unlike [`refresh_quota`]: a server that cannot answer
/// would otherwise log the same line every few seconds.
///
/// Reports whether the readings actually moved. They change on the scale of
/// whole interactions, so most of these polls bring back exactly what the
/// footer is already showing.
fn poll_quota(app: &mut App, client: &Client) -> bool {
    match client.quota_log() {
        Ok(readings) => app.quota.restock(readings),
        Err(_) => false,
    }
}

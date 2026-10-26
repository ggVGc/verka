//! Server calls the loop makes off the terminal thread, so a slow daemon or
//! filesystem never stops it reading keys.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::app::{App, LaunchPolicy};
use styra_protocol::{
    InteractionQuery, InteractionSummary, LogEntry, TemplateSummary, WorkspaceLaunchChange,
};
use styra_server::Client;

/// Launch-policy work is serialized off the terminal thread. Serialization
/// preserves the operator's edit order; the channel back to the root loop lets
/// it keep rendering and consuming input while the daemon or filesystem is
/// slow.
enum LaunchEffect {
    ListTemplates {
        request_id: u64,
        workspace_id: String,
    },
    ChangeWorkspace {
        workspace_id: String,
        change: WorkspaceLaunchChange,
        clear_interaction: bool,
    },
}

pub(super) enum LaunchEffectResult {
    Templates {
        request_id: u64,
        workspace_id: String,
        result: std::result::Result<Vec<TemplateSummary>, String>,
    },
    WorkspaceChanged {
        workspace_id: String,
        clear_interaction: bool,
        result: std::result::Result<LaunchPolicy, String>,
    },
}

pub(super) struct LaunchEffects {
    send: Option<Sender<LaunchEffect>>,
    receive: Receiver<LaunchEffectResult>,
    worker: Option<thread::JoinHandle<()>>,
    next_template_request: AtomicU64,
}

impl LaunchEffects {
    pub(super) fn new(client: Client) -> Self {
        let (send, jobs) = mpsc::channel();
        let (results, receive) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("styra-launch-effects".into())
            .spawn(move || {
                while let Ok(effect) = jobs.recv() {
                    let response = match effect {
                        LaunchEffect::ListTemplates {
                            request_id,
                            workspace_id,
                        } => {
                            let result = client
                                .list_templates(&workspace_id)
                                .map_err(|error| format!("{error:#}"));
                            LaunchEffectResult::Templates {
                                request_id,
                                workspace_id,
                                result,
                            }
                        }
                        LaunchEffect::ChangeWorkspace {
                            workspace_id,
                            change,
                            clear_interaction,
                        } => {
                            let result = client
                                .change_workspace_launch(&workspace_id, change)
                                .map_err(|error| format!("{error:#}"));
                            LaunchEffectResult::WorkspaceChanged {
                                workspace_id,
                                clear_interaction,
                                result,
                            }
                        }
                    };
                    if results.send(response).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning launch-effect worker");
        Self {
            send: Some(send),
            receive,
            worker: Some(worker),
            next_template_request: AtomicU64::new(1),
        }
    }

    fn submit(&self, effect: LaunchEffect) {
        // The receiver lives for the duration of the event loop. A panic here
        // means the worker itself died, which is not a recoverable UI error.
        self.send
            .as_ref()
            .expect("launch-effect sender missing")
            .send(effect)
            .expect("launch-effect worker stopped");
    }

    pub(super) fn submit_templates(&self, workspace_id: String) -> u64 {
        let request_id = self.next_template_request.fetch_add(1, Ordering::Relaxed);
        self.submit(LaunchEffect::ListTemplates {
            request_id,
            workspace_id,
        });
        request_id
    }

    pub(super) fn submit_workspace_launch(
        &self,
        app: &mut App,
        workspace_id: String,
        change: WorkspaceLaunchChange,
        clear_interaction: bool,
    ) {
        app.workspace_launch_pending += 1;
        self.submit(LaunchEffect::ChangeWorkspace {
            workspace_id,
            change,
            clear_interaction,
        });
    }

    /// Reports whether it applied anything, so a round in which the worker
    /// answered nothing this client is still waiting for does not count as a
    /// change to the screen.
    pub(super) fn apply_ready(&self, app: &mut App, workspace_id: &str) -> bool {
        let mut applied = false;
        while let Ok(response) = self.receive.try_recv() {
            match response {
                LaunchEffectResult::Templates {
                    request_id,
                    workspace_id: answered_for,
                    result,
                } => {
                    let current_picker = app.template_picker.as_ref().is_some_and(|picker| {
                        picker.request_id == request_id
                            && picker.workspace_id == answered_for
                            && workspace_id == answered_for
                    });
                    if !current_picker {
                        continue;
                    }
                    applied = true;
                    match result {
                        Ok(templates) if templates.is_empty() => {
                            app.template_picker = None;
                            app.push_log(LogEntry::warn("no Driva templates are available"));
                        }
                        Ok(templates) => {
                            if let Some(picker) = app.template_picker.as_mut() {
                                picker.loaded(templates);
                            }
                        }
                        Err(error) => {
                            app.template_picker = None;
                            app.push_log(LogEntry::error(format!(
                                "could not list Driva templates: {error}"
                            )));
                        }
                    }
                }
                LaunchEffectResult::WorkspaceChanged {
                    workspace_id: answered_for,
                    clear_interaction,
                    result,
                } => {
                    if workspace_id != answered_for {
                        continue;
                    }
                    applied = true;
                    app.workspace_launch_pending = app.workspace_launch_pending.saturating_sub(1);
                    match result {
                        Ok(policy) if clear_interaction => {
                            app.launch.adopt_workspace(policy);
                            app.show_action_message(
                                "moved into this Workspace's policy — every launch here starts from it",
                            );
                        }
                        Ok(policy) => {
                            app.launch.sync_workspace(policy);
                            app.show_action_message("Workspace launch policy saved");
                        }
                        Err(error) => {
                            let message =
                                format!("could not change the Workspace launch policy: {error}");
                            app.show_error(message.clone());
                            app.push_log(LogEntry::error(message));
                        }
                    }
                }
            }
        }
        applied
    }
}

impl Drop for LaunchEffects {
    fn drop(&mut self) {
        // Finish already-accepted edits before this run is allowed to leave.
        // Otherwise quitting immediately after Enter could terminate the
        // process between enqueueing a durable Workspace edit and writing it.
        self.send.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The interactions snapshot is polled off the terminal thread.
///
/// The server answers `list_interactions` by reading every live Session's
/// record and the Workspace directory, which takes on the order of 100ms. Asked
/// from the loop on every [`INTERACTIONS_REFRESH`](super::INTERACTIONS_REFRESH),
/// that was a tenth of a second in every four during which no key was read, so
/// typing landed in stutters. Here the loop only asks; the answer is applied on
/// whichever round it arrives in.
pub(super) struct InteractionsPoll {
    /// Each request carries its own query, since what the navigator shows —
    /// the Interaction on screen, whether finished rows are listed — changes
    /// between polls.
    send: Sender<InteractionQuery>,
    /// `None` for a poll the server could not answer: the snapshot on screen
    /// stays as it was, as it did when the loop polled for itself.
    receive: Receiver<Option<Vec<InteractionSummary>>>,
    /// A request has been made and not yet answered. One at a time: a server
    /// slower than the refresh period would otherwise have a queue of them.
    in_flight: bool,
}

impl InteractionsPoll {
    pub(super) fn new(client: Client) -> Self {
        let (send, requests) = mpsc::channel::<InteractionQuery>();
        let (results, receive) = mpsc::channel();
        // Not joined: the loop leaving drops `send`, and the worker finishes
        // the request it is in, if any, and stops.
        thread::Builder::new()
            .name("styra-interactions-poll".into())
            .spawn(move || {
                while let Ok(query) = requests.recv() {
                    if results.send(client.list_interactions(&query).ok()).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning interactions poller");
        Self {
            send,
            receive,
            in_flight: false,
        }
    }

    /// Ask for a fresh snapshot, unless one is already on its way.
    pub(super) fn request(&mut self, query: InteractionQuery) {
        if !self.in_flight && self.send.send(query).is_ok() {
            self.in_flight = true;
        }
    }

    /// The answer to the outstanding request, if it has arrived and the server
    /// could give one.
    pub(super) fn ready(&mut self) -> Option<Vec<InteractionSummary>> {
        let answer = self.receive.try_recv().ok()?;
        self.in_flight = false;
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects_for_test() -> (LaunchEffects, Sender<LaunchEffectResult>) {
        let (send, _jobs) = mpsc::channel();
        let (results, receive) = mpsc::channel();
        (
            LaunchEffects {
                send: Some(send),
                receive,
                worker: None,
                next_template_request: AtomicU64::new(1),
            },
            results,
        )
    }

    #[test]
    fn workspace_acknowledgement_updates_the_snapshot_and_clears_pending() {
        let (effects, results) = effects_for_test();
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.workspace_launch_pending = 1;
        let policy = LaunchPolicy {
            templates: vec!["rust".into()],
            ..LaunchPolicy::default()
        };
        results
            .send(LaunchEffectResult::WorkspaceChanged {
                workspace_id: "workspace".into(),
                clear_interaction: false,
                result: Ok(policy.clone()),
            })
            .unwrap();

        effects.apply_ready(&mut app, "workspace");

        assert_eq!(app.workspace_launch_pending, 0);
        assert_eq!(app.launch.workspace, policy);
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text == "Workspace launch policy saved"));
    }

    #[test]
    fn workspace_edit_failure_is_visible_without_opening_the_log() {
        let (effects, results) = effects_for_test();
        let mut app = App::pending(
            styra_protocol::agent::Selection::parse("codex:gpt-5.6-sol/high").unwrap(),
        );
        app.workspace_launch_pending = 1;
        results
            .send(LaunchEffectResult::WorkspaceChanged {
                workspace_id: "workspace".into(),
                clear_interaction: false,
                result: Err("disk full".into()),
            })
            .unwrap();

        effects.apply_ready(&mut app, "workspace");

        assert_eq!(app.workspace_launch_pending, 0);
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text.contains("disk full")));
    }
}

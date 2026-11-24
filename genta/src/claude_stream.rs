//! Stateful control for Claude Code's bidirectional `stream-json` protocol.

use crate::agent::claude_submission;
use crate::appserver::Action;
use crate::event::{AgentEvent, TurnOutcome, TurnUsage};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

struct PendingTurn {
    /// The turn held back until Claude acknowledges the model change, if the
    /// change was requested alongside one. A model change made on its own —
    /// the operator switching models between turns — has nothing to release.
    text: Option<String>,
    model: String,
}

struct State {
    model: String,
    next_request_id: u64,
    pending: HashMap<String, PendingTurn>,
    pending_interrupts: HashSet<String>,
}

/// A live Claude Code streaming client. It serializes model changes ahead of
/// the user turn that requested them and releases that turn only after Claude
/// acknowledges `set_model`.
pub struct ClaudeStream {
    state: Mutex<State>,
}

impl ClaudeStream {
    pub fn new(model: String) -> Self {
        Self {
            state: Mutex::new(State {
                model,
                next_request_id: 1,
                pending: HashMap::new(),
                pending_interrupts: HashSet::new(),
            }),
        }
    }

    pub fn send(&self, text: &str, model: Option<&str>) -> Vec<Action> {
        let mut state = self.state.lock().expect("Claude stream state poisoned");
        let Some(model) = model.filter(|model| *model != state.model) else {
            return vec![Action::Send(claude_submission(text))];
        };
        // A bare model change may already be in flight for exactly this model
        // (the operator switched, then typed). Ride along with it rather than
        // asking Claude to change to the same model twice.
        if let Some(pending) = state
            .pending
            .values_mut()
            .find(|pending| pending.model == model && pending.text.is_none())
        {
            pending.text = Some(text.to_owned());
            return Vec::new();
        }
        Self::request_model(&mut state, model, Some(text.to_owned()))
    }

    /// Switch models now, without a turn waiting on it, so the operator's
    /// choice takes effect when they make it rather than whenever they next
    /// happen to send something. No actions when Claude is already on it.
    pub fn set_model(&self, model: &str) -> Vec<Action> {
        let mut state = self.state.lock().expect("Claude stream state poisoned");
        if model == state.model {
            return Vec::new();
        }
        Self::request_model(&mut state, model, None)
    }

    fn request_model(state: &mut State, model: &str, text: Option<String>) -> Vec<Action> {
        let request_id = format!("styra_model_{}", state.next_request_id);
        state.next_request_id += 1;
        state.pending.insert(
            request_id.clone(),
            PendingTurn {
                text,
                model: model.to_owned(),
            },
        );
        vec![Action::Send(
            json!({
                "type": "control_request",
                "request_id": request_id,
                "request": { "subtype": "set_model", "model": model }
            })
            .to_string(),
        )]
    }

    /// Ask Claude to abandon the turn it is running. The conversation stays
    /// alive, so the next message continues where the interrupt left off.
    pub fn interrupt(&self) -> Vec<Action> {
        let mut state = self.state.lock().expect("Claude stream state poisoned");
        let request_id = format!("styra_interrupt_{}", state.next_request_id);
        state.next_request_id += 1;
        state.pending_interrupts.insert(request_id.clone());
        vec![Action::Send(
            json!({
                "type": "control_request",
                "request_id": request_id,
                "request": { "subtype": "interrupt" }
            })
            .to_string(),
        )]
    }

    /// Consume control responses. Non-control lines remain owned by the normal
    /// Claude event decoder.
    pub fn handle_line(&self, line: &str) -> Option<Vec<Action>> {
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("control_response") {
            return None;
        }
        let response = value.get("response")?;
        let request_id = response.get("request_id").and_then(Value::as_str)?;
        let mut state = self.state.lock().expect("Claude stream state poisoned");
        let errored = response.get("subtype").and_then(Value::as_str) == Some("error");
        let error = || {
            response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned()
        };
        if state.pending_interrupts.remove(request_id) {
            return Some(if errored {
                vec![Action::Warn(format!(
                    "Claude interrupt failed: {}",
                    error()
                ))]
            } else {
                vec![
                    Action::Info("Claude interrupted the active turn".to_owned()),
                    Action::Event(AgentEvent::TurnCompleted {
                        outcome: TurnOutcome::Completed,
                        usage: TurnUsage::default(),
                    }),
                ]
            });
        }
        let pending = state.pending.remove(request_id)?;
        if errored {
            // Only a held-back turn leaves the UI waiting on a turn that will
            // now never run; a bare model change has nothing to release.
            let mut actions = vec![Action::Warn(match &pending.text {
                Some(_) => format!(
                    "Claude rejected model change to {}: {}; message was not sent",
                    pending.model,
                    error()
                ),
                None => format!(
                    "Claude rejected model change to {}: {}",
                    pending.model,
                    error()
                ),
            })];
            if pending.text.is_some() {
                actions.push(Action::Event(AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                    usage: TurnUsage::default(),
                }));
            }
            return Some(actions);
        }
        state.model = pending.model.clone();
        let mut actions = vec![Action::Info(format!(
            "Claude model changed to {}",
            pending.model
        ))];
        if let Some(text) = &pending.text {
            actions.push(Action::Send(claude_submission(text)));
        }
        Some(actions)
    }
}

#[cfg(test)]
mod tests;

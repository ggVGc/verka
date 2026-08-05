//! Stateful control for Claude Code's bidirectional `stream-json` protocol.

use crate::agent::claude_submission;
use crate::appserver::Action;
use crate::event::{AgentEvent, TokenUsage};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

struct PendingTurn {
    text: String,
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
        let request_id = format!("styra_model_{}", state.next_request_id);
        state.next_request_id += 1;
        state.pending.insert(
            request_id.clone(),
            PendingTurn {
                text: text.to_owned(),
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
                vec![Action::Warn(format!("Claude interrupt failed: {}", error()))]
            } else {
                vec![
                    Action::Info("Claude interrupted the active turn".to_owned()),
                    Action::Event(AgentEvent::TurnCompleted {
                        usage: TokenUsage::default(),
                    }),
                ]
            });
        }
        let pending = state.pending.remove(request_id)?;
        if errored {
            return Some(vec![
                Action::Warn(format!(
                    "Claude rejected model change to {}: {}; message was not sent",
                    pending.model,
                    error()
                )),
                Action::Event(AgentEvent::TurnCompleted {
                    usage: TokenUsage::default(),
                }),
            ]);
        }
        state.model = pending.model.clone();
        Some(vec![
            Action::Info(format!("Claude model changed to {}", pending.model)),
            Action::Send(claude_submission(&pending.text)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(actions: &[Action]) -> Vec<Value> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Send(line) => serde_json::from_str(line).ok(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn changing_model_waits_for_ack_before_sending_the_turn() {
        let client = ClaudeStream::new("claude-sonnet-5".into());
        let control = sent(&client.send("use the stronger model", Some("claude-opus-5")));
        assert_eq!(control[0]["request"]["subtype"], "set_model");
        assert_eq!(control[0]["request"]["model"], "claude-opus-5");

        let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_model_1","response":{}}}"#,
            )
            .unwrap();
        let turn = sent(&actions);
        assert_eq!(turn[0]["type"], "user");
        assert_eq!(turn[0]["message"]["content"], "use the stronger model");
    }

    #[test]
    fn interrupting_asks_claude_to_abandon_the_turn_and_completes_it() {
        let client = ClaudeStream::new("claude-sonnet-5".into());
        let request = sent(&client.interrupt());
        assert_eq!(request[0]["type"], "control_request");
        assert_eq!(request[0]["request"]["subtype"], "interrupt");

        let actions = client
            .handle_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"styra_interrupt_1","response":{}}}"#,
            )
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [Action::Info(_), Action::Event(AgentEvent::TurnCompleted { .. })]
        ));
    }

    #[test]
    fn the_current_model_sends_without_control_traffic() {
        let client = ClaudeStream::new("claude-sonnet-5".into());
        let turn = sent(&client.send("continue", Some("claude-sonnet-5")));
        assert_eq!(turn[0]["type"], "user");
    }
}

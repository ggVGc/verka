//! Driver for the bidirectional `codex app-server` JSON-RPC protocol.
//!
//! Unlike the one-shot `exec` profile, this protocol is stateful: a session
//! must `initialize`, announce `initialized`, `thread/start` to obtain a thread
//! id, and then issue a `turn/start` per operator message, consuming streamed
//! notifications in between. This module owns that handshake and turn dispatch;
//! the notification-to-event decoding lives in [`crate::event`].
//!
//! The client is pure: it owns no pipes or channels. Each method returns the
//! [`Action`]s the host must carry out — lines to write to the agent's stdin,
//! decoded events to surface, diagnostics to log — so any transport (Styra's
//! pipe threads, a test harness) can drive it.
//!
//! The flow was verified against a live codex-cli 0.145 `app-server` session.

use crate::event::{decode_line, AgentEvent, Protocol};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Request id for `initialize`.
const INIT_ID: i64 = 1;
/// Request id for `thread/start`.
const THREAD_START_ID: i64 = 2;

/// One thing the host must do in response to protocol progress.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Write this line, newline-terminated, to the agent's stdin.
    Send(String),
    /// Surface a decoded agent event.
    Event(AgentEvent),
    /// Log an informational diagnostic.
    Info(String),
    /// Log a warning diagnostic.
    Warn(String),
}

struct State {
    thread_id: Option<String>,
    ready: bool,
    next_turn_id: i64,
    /// Messages sent before the thread was ready, replayed once it is.
    pending: Vec<String>,
}

/// A live app-server protocol client. One per session.
pub struct AppServer {
    cwd: String,
    sandbox: String,
    state: Mutex<State>,
}

impl AppServer {
    /// Create a client that will start its thread in `cwd` (the workspace path
    /// inside the sandbox).
    pub fn new(cwd: String) -> Self {
        Self {
            cwd,
            sandbox: "danger-full-access".into(),
            state: Mutex::new(State {
                thread_id: None,
                ready: false,
                next_turn_id: THREAD_START_ID + 1,
                pending: Vec::new(),
            }),
        }
    }

    /// Begin the handshake by sending `initialize`.
    pub fn start(&self) -> Vec<Action> {
        vec![send(&json!({
            "id": INIT_ID,
            "method": "initialize",
            "params": { "clientInfo": { "name": "genta", "version": env!("CARGO_PKG_VERSION") } }
        }))]
    }

    /// Handle one line received from the agent: drive the handshake on control
    /// messages, and forward decoded events from notifications.
    pub fn handle_line(&self, line: &str) -> Vec<Action> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                // Surface undecodable input the same way the decoder would.
                return vec![Action::Event(decode_line(Protocol::CodexAppServer, line))];
            }
        };
        let method = value.get("method").and_then(Value::as_str);
        let id = value.get("id").and_then(Value::as_i64);

        match (method, id) {
            // A response to one of our requests: advance the handshake.
            (None, Some(INIT_ID)) => vec![
                send(&json!({ "method": "initialized" })),
                send(&json!({
                    "id": THREAD_START_ID,
                    "method": "thread/start",
                    "params": { "cwd": self.cwd, "approvalPolicy": "never", "sandbox": self.sandbox }
                })),
            ],
            (None, Some(THREAD_START_ID)) => {
                match value
                    .get("result")
                    .and_then(|result| result.get("thread"))
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                {
                    Some(thread_id) => self.become_ready(thread_id.to_owned()),
                    None => Vec::new(),
                }
            }
            (None, _) => Vec::new(), // response to a turn/start or later request
            // A server-to-client request needs a reply. With approvalPolicy
            // "never" and danger-full-access we expect none; log any that
            // appear so they are visible rather than a silent stall.
            (Some(request), Some(_)) => {
                vec![Action::Warn(format!("unhandled server request: {request}"))]
            }
            // A notification: capture the thread id as a backup, then decode.
            (Some(notification), None) => {
                let mut actions = Vec::new();
                if notification == "thread/started" {
                    if let Some(thread_id) = value
                        .get("params")
                        .and_then(|params| params.get("thread"))
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                    {
                        actions.extend(self.become_ready(thread_id.to_owned()));
                    }
                }
                let event = decode_line(Protocol::CodexAppServer, line);
                if !matches!(event, AgentEvent::Unknown { .. }) {
                    actions.push(Action::Event(event));
                }
                actions
            }
        }
    }

    /// Send an operator message as a new turn, or queue it until the thread is
    /// ready.
    pub fn send(&self, text: &str) -> Vec<Action> {
        let (thread_id, id) = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            let Some(thread_id) = state.thread_id.clone() else {
                state.pending.push(text.to_owned());
                return vec![Action::Info(
                    "queued message until the app-server session is ready".into(),
                )];
            };
            let id = state.next_turn_id;
            state.next_turn_id += 1;
            (thread_id, id)
        };
        vec![send(&json!({
            "id": id,
            "method": "turn/start",
            "params": { "threadId": thread_id, "input": [{ "type": "text", "text": text }] }
        }))]
    }

    fn become_ready(&self, thread_id: String) -> Vec<Action> {
        let pending = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            if state.ready {
                return Vec::new();
            }
            state.thread_id = Some(thread_id.clone());
            state.ready = true;
            std::mem::take(&mut state.pending)
        };
        let mut actions = vec![Action::Info(format!("app-server ready; thread {thread_id}"))];
        for text in pending {
            actions.extend(self.send(&text));
        }
        actions
    }
}

fn send(message: &Value) -> Action {
    Action::Send(message.to_string())
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
    fn start_sends_initialize() {
        let client = AppServer::new("/tmp/styra/workspace".into());
        let sent = sent(&client.start());
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["id"], INIT_ID);
    }

    #[test]
    fn handshake_progresses_and_a_ready_thread_starts_a_turn() {
        let client = AppServer::new("/tmp/styra/workspace".into());

        // initialize response -> client sends initialized + thread/start
        let sent_lines = sent(&client.handle_line(r#"{"id":1,"result":{}}"#));
        assert_eq!(sent_lines[0]["method"], "initialized");
        assert_eq!(sent_lines[1]["method"], "thread/start");
        assert_eq!(sent_lines[1]["params"]["approvalPolicy"], "never");
        assert_eq!(sent_lines[1]["params"]["sandbox"], "danger-full-access");

        // thread/start response -> ready
        client.handle_line(r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#);

        // A sent message becomes turn/start referencing the thread.
        let turn = sent(&client.send("do the thing"));
        assert_eq!(turn[0]["method"], "turn/start");
        assert_eq!(turn[0]["params"]["threadId"], "thread-xyz");
        assert_eq!(turn[0]["params"]["input"][0]["text"], "do the thing");
    }

    #[test]
    fn a_message_sent_before_ready_is_queued_then_flushed() {
        let client = AppServer::new("/tmp/styra/workspace".into());

        // Nothing on the wire yet; it was queued.
        assert!(sent(&client.send("early")).is_empty());

        // Becoming ready flushes the queued message as a turn.
        let actions =
            client.handle_line(r#"{"method":"thread/started","params":{"thread":{"id":"t1"}}}"#);
        let turn = sent(&actions);
        assert_eq!(turn[0]["method"], "turn/start");
        assert_eq!(turn[0]["params"]["input"][0]["text"], "early");
    }

    #[test]
    fn a_notification_is_forwarded_as_an_event() {
        let client = AppServer::new("/tmp/styra/workspace".into());
        let actions = client.handle_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"hi"}}}"#,
        );
        let events: Vec<&AgentEvent> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Event(event) => Some(event),
                _ => None,
            })
            .collect();
        assert_eq!(events, vec![&AgentEvent::AgentMessage { text: "hi".into() }]);
    }
}

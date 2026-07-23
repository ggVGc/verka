//! Driver for the bidirectional `codex app-server` JSON-RPC protocol.
//!
//! Unlike the one-shot `exec` profile, this protocol is stateful: a session
//! must `initialize`, announce `initialized`, `thread/start` to obtain a thread
//! id, and then issue a `turn/start` per operator message, consuming streamed
//! notifications in between. This module owns that handshake and turn dispatch;
//! the notification-to-event decoding lives in [`crate::event`] and is shared
//! with journal replay.
//!
//! The flow was verified against a live codex-cli 0.145 `app-server` session.

use crate::event::{decode_line, Protocol, StyraEvent};
use crate::session::{Direction, LogEntry, RawLine, SessionUpdate};
use serde_json::{json, Value};
use std::io::{PipeWriter, Write};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

/// Request id for `initialize`.
const INIT_ID: i64 = 1;
/// Request id for `thread/start`.
const THREAD_START_ID: i64 = 2;

/// The shared agent stdin handle: a pipe writer that is taken (dropped) when the
/// session stops.
type Stdin = Mutex<Option<PipeWriter>>;

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
    pub fn start(&self, stdin: &Stdin, updates: &Sender<SessionUpdate>) {
        self.write(
            stdin,
            updates,
            &json!({
                "id": INIT_ID,
                "method": "initialize",
                "params": { "clientInfo": { "name": "styra", "version": env!("CARGO_PKG_VERSION") } }
            }),
        );
    }

    /// Handle one line received from the agent: drive the handshake on control
    /// messages, and forward decoded events from notifications.
    pub fn handle_line(&self, line: &str, stdin: &Stdin, updates: &Sender<SessionUpdate>) {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                // Surface undecodable input the same way the decoder would.
                let _ = updates.send(SessionUpdate::Event(decode_line(Protocol::CodexAppServer, line)));
                return;
            }
        };
        let method = value.get("method").and_then(Value::as_str);
        let id = value.get("id").and_then(Value::as_i64);

        match (method, id) {
            // A response to one of our requests: advance the handshake.
            (None, Some(INIT_ID)) => {
                self.write(stdin, updates, &json!({ "method": "initialized" }));
                self.write(
                    stdin,
                    updates,
                    &json!({
                        "id": THREAD_START_ID,
                        "method": "thread/start",
                        "params": { "cwd": self.cwd, "approvalPolicy": "never", "sandbox": self.sandbox }
                    }),
                );
            }
            (None, Some(THREAD_START_ID)) => {
                if let Some(thread_id) = value
                    .get("result")
                    .and_then(|result| result.get("thread"))
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                {
                    self.become_ready(thread_id.to_owned(), stdin, updates);
                }
            }
            (None, _) => {} // response to a turn/start or later request; nothing to do
            // A server-to-client request needs a reply. With approvalPolicy
            // "never" and danger-full-access we expect none; log any that
            // appear so they are visible rather than a silent stall.
            (Some(request), Some(_)) => {
                let _ = updates.send(SessionUpdate::Log(LogEntry::warn(format!(
                    "unhandled server request: {request}"
                ))));
            }
            // A notification: capture the thread id as a backup, then decode.
            (Some(notification), None) => {
                if notification == "thread/started" {
                    if let Some(thread_id) = value
                        .get("params")
                        .and_then(|params| params.get("thread"))
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                    {
                        self.become_ready(thread_id.to_owned(), stdin, updates);
                    }
                }
                let event = decode_line(Protocol::CodexAppServer, line);
                if !matches!(event, StyraEvent::Unknown { .. }) {
                    let _ = updates.send(SessionUpdate::Event(event));
                }
            }
        }
    }

    /// Send an operator message as a new turn, or queue it until the thread is
    /// ready.
    pub fn send(&self, text: &str, stdin: &Stdin, updates: &Sender<SessionUpdate>) {
        let (thread_id, id) = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            let Some(thread_id) = state.thread_id.clone() else {
                state.pending.push(text.to_owned());
                let _ = updates.send(SessionUpdate::Log(LogEntry::info(
                    "queued message until the app-server session is ready",
                )));
                return;
            };
            let id = state.next_turn_id;
            state.next_turn_id += 1;
            (thread_id, id)
        };
        self.write(
            stdin,
            updates,
            &json!({
                "id": id,
                "method": "turn/start",
                "params": { "threadId": thread_id, "input": [{ "type": "text", "text": text }] }
            }),
        );
    }

    fn become_ready(&self, thread_id: String, stdin: &Stdin, updates: &Sender<SessionUpdate>) {
        let pending = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            if state.ready {
                return;
            }
            state.thread_id = Some(thread_id.clone());
            state.ready = true;
            std::mem::take(&mut state.pending)
        };
        let _ = updates.send(SessionUpdate::Log(LogEntry::info(format!(
            "app-server ready; thread {thread_id}"
        ))));
        for text in pending {
            self.send(&text, stdin, updates);
        }
    }

    fn write(&self, stdin: &Stdin, updates: &Sender<SessionUpdate>, message: &Value) {
        let line = message.to_string();
        let _ = updates.send(SessionUpdate::Raw(RawLine {
            direction: Direction::ToAgent,
            text: line.clone(),
        }));
        if let Ok(mut guard) = stdin.lock() {
            if let Some(writer) = guard.as_mut() {
                let _ = writer.write_all(line.as_bytes());
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    // The client writes to a real OS pipe so the exact bytes can be read back.
    fn stdin_pipe() -> (Stdin, std::io::PipeReader) {
        let (reader, writer) = std::io::pipe().unwrap();
        (Mutex::new(Some(writer)), reader)
    }

    fn drain(updates: &std::sync::mpsc::Receiver<SessionUpdate>) -> Vec<SessionUpdate> {
        let mut out = Vec::new();
        while let Ok(update) = updates.try_recv() {
            out.push(update);
        }
        out
    }

    #[test]
    fn start_sends_initialize() {
        let (stdin, _reader) = stdin_pipe();
        let (tx, rx) = channel();
        let client = AppServer::new("/tmp/styra/workspace".into());
        client.start(&stdin, &tx);
        let raw: Vec<String> = drain(&rx)
            .into_iter()
            .filter_map(|u| match u {
                SessionUpdate::Raw(r) => Some(r.text),
                _ => None,
            })
            .collect();
        assert_eq!(raw.len(), 1);
        let value: Value = serde_json::from_str(&raw[0]).unwrap();
        assert_eq!(value["method"], "initialize");
        assert_eq!(value["id"], INIT_ID);
    }

    #[test]
    fn handshake_progresses_and_a_ready_thread_starts_a_turn() {
        let (stdin, _reader) = stdin_pipe();
        let (tx, rx) = channel();
        let client = AppServer::new("/tmp/styra/workspace".into());

        // initialize response -> client sends initialized + thread/start
        client.handle_line(r#"{"id":1,"result":{}}"#, &stdin, &tx);
        let sent: Vec<Value> = drain(&rx)
            .into_iter()
            .filter_map(|u| match u {
                SessionUpdate::Raw(r) => serde_json::from_str(&r.text).ok(),
                _ => None,
            })
            .collect();
        assert_eq!(sent[0]["method"], "initialized");
        assert_eq!(sent[1]["method"], "thread/start");
        assert_eq!(sent[1]["params"]["approvalPolicy"], "never");
        assert_eq!(sent[1]["params"]["sandbox"], "danger-full-access");

        // thread/start response -> ready
        client.handle_line(
            r#"{"id":2,"result":{"thread":{"id":"thread-xyz"}}}"#,
            &stdin,
            &tx,
        );
        let _ = drain(&rx);

        // A sent message becomes turn/start referencing the thread.
        client.send("do the thing", &stdin, &tx);
        let turn: Value = drain(&rx)
            .into_iter()
            .find_map(|u| match u {
                SessionUpdate::Raw(r) => serde_json::from_str::<Value>(&r.text)
                    .ok()
                    .filter(|v| v["method"] == "turn/start"),
                _ => None,
            })
            .expect("a turn/start was written");
        assert_eq!(turn["params"]["threadId"], "thread-xyz");
        assert_eq!(turn["params"]["input"][0]["text"], "do the thing");
    }

    #[test]
    fn a_message_sent_before_ready_is_queued_then_flushed() {
        let (stdin, _reader) = stdin_pipe();
        let (tx, rx) = channel();
        let client = AppServer::new("/tmp/styra/workspace".into());

        client.send("early", &stdin, &tx);
        // Nothing on the wire yet; it was queued.
        assert!(!drain(&rx).into_iter().any(|u| matches!(u, SessionUpdate::Raw(_))));

        // Becoming ready flushes the queued message as a turn.
        client.handle_line(
            r#"{"method":"thread/started","params":{"thread":{"id":"t1"}}}"#,
            &stdin,
            &tx,
        );
        let turn = drain(&rx).into_iter().find_map(|u| match u {
            SessionUpdate::Raw(r) => serde_json::from_str::<Value>(&r.text)
                .ok()
                .filter(|v| v["method"] == "turn/start"),
            _ => None,
        });
        assert_eq!(turn.unwrap()["params"]["input"][0]["text"], "early");
    }

    #[test]
    fn a_notification_is_forwarded_as_an_event() {
        let (stdin, _reader) = stdin_pipe();
        let (tx, rx) = channel();
        let client = AppServer::new("/tmp/styra/workspace".into());
        client.handle_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"hi"}}}"#,
            &stdin,
            &tx,
        );
        let events: Vec<StyraEvent> = drain(&rx)
            .into_iter()
            .filter_map(|u| match u {
                SessionUpdate::Event(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(events, vec![StyraEvent::AgentMessage { text: "hi".into() }]);
    }
}

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
use std::sync::Arc;
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
    /// The requested provider conversation could not be opened.
    Error(String),
}

/// A function the app-server advertises to Codex and asks its host to execute.
///
/// The callback stays in the embedding application. Genta only translates the
/// app-server's `dynamicTools` and `item/tool/call` protocol, so a host can add
/// capabilities without putting application-specific behavior in this crate.
type DynamicToolHandler = dyn Fn(&Value) -> Result<String, String> + Send + Sync;

#[derive(Clone)]
pub struct DynamicTool {
    name: String,
    description: String,
    input_schema: Value,
    handler: Arc<DynamicToolHandler>,
}

impl DynamicTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: impl Fn(&Value) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler: Arc::new(handler),
        }
    }

    fn specification(&self) -> Value {
        json!({
            "type": "function",
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
        })
    }
}

struct State {
    thread_id: Option<String>,
    active_turn_id: Option<String>,
    ready: bool,
    next_request_id: i64,
    /// Messages sent before the thread was ready, replayed once it is.
    pending: Vec<String>,
}

/// A live app-server protocol client. One per session.
pub struct AppServer {
    cwd: Mutex<String>,
    sandbox: String,
    resume_thread_id: Option<String>,
    dynamic_tools: Vec<DynamicTool>,
    state: Mutex<State>,
}

impl AppServer {
    /// Create a client that will start its thread in `cwd` (the workspace path
    /// inside the sandbox).
    pub fn new(cwd: String) -> Self {
        Self::with_resume(cwd, None)
    }

    /// Create a client which resumes `thread_id` after initialization instead
    /// of creating a new thread.
    pub fn resume(cwd: String, thread_id: String) -> Self {
        Self::with_resume(cwd, Some(thread_id))
    }

    fn with_resume(cwd: String, resume_thread_id: Option<String>) -> Self {
        Self {
            cwd: Mutex::new(cwd),
            sandbox: "danger-full-access".into(),
            resume_thread_id,
            dynamic_tools: Vec::new(),
            state: Mutex::new(State {
                thread_id: None,
                active_turn_id: None,
                ready: false,
                next_request_id: THREAD_START_ID + 1,
                pending: Vec::new(),
            }),
        }
    }

    /// Add host-executed functions to the thread opened by this client.
    pub fn with_dynamic_tools(mut self, tools: Vec<DynamicTool>) -> Self {
        self.dynamic_tools = tools;
        self
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

        if method == Some("item/tool/call") && value.get("id").is_some() {
            return vec![self.handle_dynamic_tool_call(&value)];
        }

        match (method, id) {
            // A response to one of our requests: advance the handshake.
            (None, Some(INIT_ID)) => {
                let mut params = json!({
                    "cwd": self.current_cwd(),
                    "approvalPolicy": "never",
                    "sandbox": self.sandbox
                });
                if !self.dynamic_tools.is_empty() {
                    params["dynamicTools"] = Value::Array(
                        self.dynamic_tools
                            .iter()
                            .map(DynamicTool::specification)
                            .collect(),
                    );
                }
                let (method, params) = match &self.resume_thread_id {
                    Some(thread_id) => {
                        params["threadId"] = json!(thread_id);
                        ("thread/resume", params)
                    }
                    None => ("thread/start", params),
                };
                let open_thread = send(&json!({
                    "id": THREAD_START_ID,
                    "method": method,
                    "params": params,
                }));
                vec![send(&json!({ "method": "initialized" })), open_thread]
            }
            (None, Some(THREAD_START_ID)) => {
                if let Some(error) = value.get("error") {
                    return vec![Action::Error(format!(
                        "could not {} Codex thread: {error}",
                        if self.resume_thread_id.is_some() {
                            "resume"
                        } else {
                            "start"
                        }
                    ))];
                }
                match value
                    .get("result")
                    .and_then(|result| result.get("thread"))
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
                {
                    Some(thread_id) => {
                        let mut actions = self.become_ready(thread_id.to_owned());
                        // This response is the only place the app-server states
                        // the model and reasoning effort the thread runs on, so
                        // it is surfaced as an event rather than left as control
                        // traffic. The decoder reads it from the same line a
                        // replayed journal would.
                        actions.push(Action::Event(decode_line(Protocol::CodexAppServer, line)));
                        actions
                    }
                    None => Vec::new(),
                }
            }
            (None, _) => {
                if let Some(turn_id) = turn_id(&value) {
                    self.state
                        .lock()
                        .expect("app-server state poisoned")
                        .active_turn_id = Some(turn_id.to_owned());
                }
                Vec::new()
            }
            // A server-to-client request needs a reply. With approvalPolicy
            // "never" and danger-full-access we expect none; log any that
            // appear so they are visible rather than a silent stall.
            (Some(request), Some(_)) => {
                vec![Action::Warn(format!("unhandled server request: {request}"))]
            }
            // A notification: capture the thread id as a backup, then decode.
            (Some(notification), None) => {
                let mut actions = Vec::new();
                let mut already_started = false;
                if notification == "turn/started" {
                    if let Some(turn_id) = turn_id(&value) {
                        self.state
                            .lock()
                            .expect("app-server state poisoned")
                            .active_turn_id = Some(turn_id.to_owned());
                    }
                } else if notification == "turn/completed" {
                    self.state
                        .lock()
                        .expect("app-server state poisoned")
                        .active_turn_id = None;
                }
                if notification == "thread/started" {
                    if let Some(thread_id) = value
                        .get("params")
                        .and_then(|params| params.get("thread"))
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                    {
                        let ready = self.become_ready(thread_id.to_owned());
                        already_started = ready.is_empty();
                        actions.extend(ready);
                    }
                }
                // This notification duplicates the thread the `thread/start`
                // response already reported, and names neither model nor
                // effort — so once the thread is known it adds nothing but a
                // second, less informative session entry.
                if already_started {
                    return actions;
                }
                let event = decode_line(Protocol::CodexAppServer, line);
                if !matches!(event, AgentEvent::Unknown { .. }) {
                    actions.push(Action::Event(event));
                }
                actions
            }
        }
    }

    fn handle_dynamic_tool_call(&self, request: &Value) -> Action {
        let request_id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").unwrap_or(&Value::Null);
        let name = params.get("tool").and_then(Value::as_str);
        let arguments = params.get("arguments").unwrap_or(&Value::Null);
        let result = name
            .and_then(|name| self.dynamic_tools.iter().find(|tool| tool.name == name))
            .ok_or_else(|| match name {
                Some(name) => format!("unknown dynamic tool {name:?}"),
                None => "dynamic tool call did not name a tool".into(),
            })
            .and_then(|tool| (tool.handler)(arguments));
        let (success, text) = match result {
            Ok(output) => (true, output),
            Err(error) => (false, error),
        };
        send(&json!({
            "id": request_id,
            "result": {
                "contentItems": [{ "type": "inputText", "text": text }],
                "success": success,
            }
        }))
    }

    /// Send an operator message as a new turn, or queue it until the thread is
    /// ready.
    pub fn send(&self, text: &str) -> Vec<Action> {
        self.send_with_options(text, None, None)
    }

    /// Send an operator message with per-turn model settings. App-server makes
    /// these the defaults for later turns on the same thread.
    pub fn send_with_options(
        &self,
        text: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Vec<Action> {
        let (thread_id, id, cwd) = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            let Some(thread_id) = state.thread_id.clone() else {
                state.pending.push(text.to_owned());
                return vec![Action::Info(
                    "queued message until the app-server session is ready".into(),
                )];
            };
            let id = state.next_request_id;
            state.next_request_id += 1;
            (
                thread_id,
                id,
                self.cwd.lock().expect("app-server cwd poisoned").clone(),
            )
        };
        let mut params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": text }],
            "cwd": cwd,
        });
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        vec![send(&json!({
            "id": id,
            "method": "turn/start",
            "params": params
        }))]
    }

    /// Change the directory used for the next and later turns. Codex applies a
    /// `cwd` supplied to `turn/start` as the thread's new default.
    pub fn set_cwd(&self, cwd: String) {
        *self.cwd.lock().expect("app-server cwd poisoned") = cwd;
    }

    fn current_cwd(&self) -> String {
        self.cwd.lock().expect("app-server cwd poisoned").clone()
    }

    /// Interrupt the in-flight turn without closing the app-server process or
    /// its thread. A later operator message can start another turn normally.
    pub fn interrupt(&self) -> Result<Vec<Action>, &'static str> {
        let (thread_id, turn_id, id) = {
            let mut state = self.state.lock().expect("app-server state poisoned");
            let thread_id = state
                .thread_id
                .clone()
                .ok_or("the app-server session is not ready")?;
            let turn_id = state
                .active_turn_id
                .clone()
                .ok_or("there is no active turn to interrupt")?;
            let id = state.next_request_id;
            state.next_request_id += 1;
            (thread_id, turn_id, id)
        };
        Ok(vec![send(&json!({
            "id": id,
            "method": "turn/interrupt",
            "params": { "threadId": thread_id, "turnId": turn_id }
        }))])
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
        let mut actions = vec![Action::Info(format!(
            "app-server ready; thread {thread_id}"
        ))];
        for text in pending {
            actions.extend(self.send(&text));
        }
        actions
    }
}

fn turn_id(value: &Value) -> Option<&str> {
    value
        .get("params")
        .and_then(|params| params.get("turn"))
        .or_else(|| value.get("result").and_then(|result| result.get("turn")))
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
}

fn send(message: &Value) -> Action {
    Action::Send(message.to_string())
}

#[cfg(test)]
mod tests;

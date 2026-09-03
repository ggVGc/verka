//! Agent event vocabulary, wire decoding, and presentation.
//!
//! The provider wire format stops here. Hosts consume only [`AgentEvent`] and
//! its rendered [`summary`](AgentEvent::summary) and
//! [`detail`](AgentEvent::detail); their process transport stays uninterpreted.
//!
//! Decoding is versioned by [`Protocol`]: a new wire format is a new
//! `Protocol` variant plus a decode arm, and the match is exhaustive, so a
//! missing decoder is a compile error rather than a silent mis-decode.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The wire protocol an agent speaks, and thus the decoder that reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// The one-shot `codex exec --json` item/thread/turn event schema.
    #[default]
    CodexJsonl,
    /// The bidirectional `codex app-server` JSON-RPC protocol (v2). Notification
    /// lines carry the events; requests and responses are control traffic.
    CodexAppServer,
    /// The Claude Code `stream-json` schema: a `system`/`assistant`/`user`/
    /// `result` newline-delimited JSON stream, as emitted by
    /// `claude --output-format stream-json`.
    ClaudeJsonl,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

/// A provider-reported or locally reconstructed file change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// The stable, provider-independent event vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A message the operator sent to the agent, recorded so their own turns
    /// appear inline in the same list. Host-originated, never decoded.
    UserMessage {
        text: String,
    },
    /// A session began. Both agents report what they are actually running as
    /// they start one, so this is also where the effective model and reasoning
    /// effort come from — not from what the operator asked for, which may have
    /// named neither. `model` and `effort` are `None` when the agent's start
    /// line does not name them (Claude Code reports a model but no effort; a
    /// codex `thread/started` notification names neither, and its
    /// `thread/start` response names both).
    ThreadStarted {
        thread_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    TurnStarted,
    TurnCompleted {
        usage: TokenUsage,
    },
    /// A token-usage snapshot that arrives independently of a turn's end (the
    /// app-server protocol reports it after every step within a turn, not just
    /// the last). Updates the usage display without signalling that the agent
    /// has gone idle — see `TurnCompleted` for the actual end-of-turn signal.
    UsageUpdated {
        usage: TokenUsage,
    },
    CommandStarted {
        command: String,
    },
    CommandCompleted {
        command: String,
        status: String,
        exit_code: Option<i64>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output: String,
    },
    FileChanged {
        /// The provider's item id, used by hosts that correlate file changes
        /// with their own journals (e.g. Orka's checkpoint commits).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        paths: Vec<String>,
        /// A best-effort provider diff for this file-change item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        /// A host-attached checkpoint commit for this change, never decoded
        /// from the wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_error: Option<String>,
    },
    /// An aggregated diff snapshot that is not itself a file-change item.
    /// Codex app-server emits this after each file-change item.
    DiffUpdated {
        diff: String,
    },
    ToolStarted {
        /// Correlates with the matching `ToolCompleted`, so the host can
        /// replace the running row in place rather than appending a second
        /// line for the same call.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        name: String,
        detail: String,
    },
    ToolCompleted {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        id: String,
        name: String,
        /// The same call detail `ToolStarted` carried (e.g. a command's
        /// arguments), so the completed row still shows what actually ran
        /// rather than just the bare tool name.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
        status: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        output: String,
    },
    PlanUpdated {
        text: String,
    },
    AgentMessage {
        text: String,
    },
    /// Claude's extended-thinking prose, surfaced only when a message carries
    /// no visible text alongside it — see [`AgentEvent::is_minor`]. Claude also
    /// reports its thinking-token spend on its own lines, with no prose; such
    /// an update carries only `tokens`, and clients fold consecutive thinking
    /// events into one line rather than showing every tick (see
    /// [`AgentEvent::updates_thinking`]).
    Thinking {
        text: String,
        /// The thinking tokens *this* update reports, not a running total:
        /// Claude's count restarts with each block of reasoning, so a client
        /// folding a run of these into one line adds them up to show what the
        /// whole run of thinking cost.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
    },
    Error {
        message: String,
    },
    /// The operator moved the session onto a different model or reasoning
    /// effort mid-conversation. No provider reports this on the wire — the
    /// host synthesizes it when it applies the change — but it belongs in the
    /// log beside the messages, because which model answered is part of
    /// reading the conversation back.
    ///
    /// Each field is `Some` only if *that* setting changed, so the line states
    /// the change rather than restating the whole selection: an operator who
    /// switched model alone should not have to remember what the effort was to
    /// see that it stayed put. At least one is always `Some` — a selection
    /// equal to the current one is not a change and produces no event.
    ModelChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// A task the agent started alongside its own work: a backgrounded shell
    /// command or a subagent. Claude keys these by a task id that its later
    /// progress and completion lines repeat, so a client shows one row per
    /// task rather than one per report — see [`AgentEvent::task_id`].
    TaskStarted {
        id: String,
        description: String,
        /// The provider's own name for the kind of task (Claude: `local_bash`
        /// for a backgrounded command, `local_agent` for a subagent).
        kind: String,
        /// The subagent type, when the task runs an agent rather than a command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    /// What a running task is doing now, reported repeatedly while it runs.
    /// `description` is the task's current activity, not the description it
    /// started with.
    TaskProgress {
        id: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        /// The tool the task most recently used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
        /// Tokens the task has spent so far.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
    },
    /// A task reached a terminal state. Claude reports this twice for the same
    /// task — once as a notification carrying a human summary, once as a status
    /// patch that may carry the error — so clients merge both into one row.
    TaskCompleted {
        id: String,
        /// `completed`, `failed`, `stopped`, or `killed`.
        status: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// The authoritative count of background tasks the agent currently has
    /// running, as reported by the provider whenever that set changes.
    BackgroundTasks {
        running: usize,
    },
    /// A recognised envelope with no rendered view; carried, not shown as prose.
    Unknown {
        wire_type: String,
    },
    /// An undecodable line, kept visible as an error rather than dropped.
    Malformed {
        error: String,
    },
}

/// A structured, escape-free piece of a detail body. The renderer adds styling;
/// this never carries terminal control sequences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailBlock {
    Text(String),
    Code {
        language: Option<String>,
        text: String,
    },
}

/// Whether an event is shown in its concise, semantic form or in the complete
/// provider-decoded form. Pretty presentation may be generic (minimal diffs)
/// or protocol-specific (Claude's JSON-encoded Bash input).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationMode {
    #[default]
    Pretty,
    Raw,
}

impl AgentEvent {
    /// Whether this event belongs in the human-readable conversation. Errors
    /// complete the exchange being read, and model changes explain which
    /// model produced the replies around them.
    pub fn is_conversation(&self) -> bool {
        matches!(
            self,
            AgentEvent::UserMessage { .. }
                | AgentEvent::AgentMessage { .. }
                | AgentEvent::Error { .. }
                | AgentEvent::ModelChanged { .. }
        )
    }

    /// Whether this event starts or finishes a Claude Code background shell
    /// task. Claude reports `run_in_background` in the Bash tool input, while
    /// the later TaskOutput/TaskGet result reports whether that task is done.
    /// Keeping this interpretation here lets all Styra clients use the same
    /// lifecycle without exposing provider wire details at the UI boundary.
    pub fn starts_background_task(&self) -> bool {
        let AgentEvent::ToolStarted { name, detail, .. } = self else {
            return false;
        };
        name == "Bash"
            && serde_json::from_str::<serde_json::Value>(detail)
                .ok()
                .and_then(|input| input.get("run_in_background").and_then(|v| v.as_bool()))
                == Some(true)
    }

    /// The authoritative background-task count this event carries, if any.
    /// Claude reports the whole set whenever it changes, which is a far better
    /// signal than inferring the lifecycle from tool calls: the agent need not
    /// poll a task at all to learn it finished, so a client that waits for a
    /// poll can hold a "background work running" state forever.
    pub fn background_tasks_running(&self) -> Option<usize> {
        match self {
            AgentEvent::BackgroundTasks { running } => Some(*running),
            _ => None,
        }
    }

    /// Whether this event is a completed poll of a background task. Claude's
    /// task tools use slightly different names across releases, so accept both
    /// spellings and inspect the result text rather than relying on one exact
    /// response format.
    pub fn finishes_background_task(&self) -> bool {
        let AgentEvent::ToolCompleted { name, output, .. } = self else {
            return false;
        };
        let _ = name; // Claude's tool_result replaces the name with its id.
        let output = output.to_ascii_lowercase();
        !output.contains("still running")
            && !output.contains("running")
            && (output.contains("completed")
                || output.contains("finished")
                || output.contains("exit code")
                || output.contains("succeeded")
                || output.contains("success"))
    }

    /// The task this event reports on, if it reports on one. Every task line
    /// repeats the id, which is what lets a client fold a task's whole life —
    /// start, progress, end — into the single row it is about.
    pub fn task_id(&self) -> Option<&str> {
        match self {
            AgentEvent::TaskStarted { id, .. }
            | AgentEvent::TaskProgress { id, .. }
            | AgentEvent::TaskCompleted { id, .. } => Some(id),
            _ => None,
        }
    }

    /// The short tag shown at the head of the collapsed list line.
    pub fn tag(&self) -> &'static str {
        match self {
            AgentEvent::UserMessage { .. } => "user",
            AgentEvent::ThreadStarted { .. } => "thread",
            AgentEvent::TurnStarted => "turn",
            AgentEvent::TurnCompleted { .. } | AgentEvent::UsageUpdated { .. } => "usage",
            AgentEvent::CommandStarted { .. } | AgentEvent::CommandCompleted { .. } => "shell",
            AgentEvent::FileChanged { .. } | AgentEvent::DiffUpdated { .. } => "files",
            AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. }
                if name == "Bash" =>
            {
                "shell"
            }
            AgentEvent::ToolStarted { .. } | AgentEvent::ToolCompleted { .. } => "tool",
            AgentEvent::PlanUpdated { .. } => "plan",
            AgentEvent::AgentMessage { .. } => "agent",
            AgentEvent::Thinking { .. } => "thinking",
            AgentEvent::Error { .. } => "error",
            AgentEvent::ModelChanged { .. } => "model",
            AgentEvent::TaskStarted { .. }
            | AgentEvent::TaskProgress { .. }
            | AgentEvent::TaskCompleted { .. } => "task",
            AgentEvent::BackgroundTasks { .. } => "tasks",
            AgentEvent::Unknown { .. } => "unknown",
            AgentEvent::Malformed { .. } => "malformed",
        }
    }

    /// True for high-frequency lifecycle/bookkeeping events — thread and turn
    /// markers, token usage, Claude's rate-limit snapshots, Claude's other
    /// `system:*` bookkeeping lines (e.g. `system:thinking_tokens`,
    /// `system:compact_boundary`), and thinking-only prose — that carry little
    /// signal turn over turn. The UI hides these by default so the list reads
    /// as the agent's actual work.
    pub fn is_minor(&self) -> bool {
        matches!(
            self,
            AgentEvent::ThreadStarted { .. }
                | AgentEvent::TurnStarted
                | AgentEvent::TurnCompleted { .. }
                | AgentEvent::UsageUpdated { .. }
                | AgentEvent::Thinking { .. }
                | AgentEvent::BackgroundTasks { .. }
        ) || matches!(self, AgentEvent::Unknown { wire_type }
            if wire_type == "rate_limit_event" || wire_type.starts_with("system:"))
    }

    /// Whether this event refreshes the current thinking line rather than
    /// adding one of its own. Claude emits extended-thinking prose and a
    /// running token count as separate lines, many per turn; clients fold a
    /// run of them into a single line that changes in place.
    pub fn updates_thinking(&self) -> bool {
        matches!(self, AgentEvent::Thinking { .. })
    }

    /// A single collapsed-line summary. Never contains newlines.
    pub fn summary(&self) -> String {
        let line = match self {
            AgentEvent::UserMessage { text } => first_line(text),
            AgentEvent::ThreadStarted {
                thread_id,
                model,
                effort,
            } => match (model, effort) {
                (Some(model), Some(effort)) => format!("session {thread_id} · {model} · {effort}"),
                (Some(model), None) => format!("session {thread_id} · {model}"),
                _ => format!("session {thread_id}"),
            },
            AgentEvent::TurnStarted => "turn started".into(),
            AgentEvent::TurnCompleted { usage } | AgentEvent::UsageUpdated { usage } => format!(
                "in {} · out {} · cached {}",
                usage.input_tokens, usage.output_tokens, usage.cached_input_tokens
            ),
            AgentEvent::CommandStarted { command } => {
                format!("{} (running)", first_line(command))
            }
            AgentEvent::CommandCompleted {
                command,
                status,
                exit_code,
                ..
            } => match exit_code {
                Some(code) => format!("{} ({status}, exit {code})", first_line(command)),
                None => format!("{} ({status})", first_line(command)),
            },
            AgentEvent::FileChanged { paths, .. } => paths.join(", "),
            AgentEvent::DiffUpdated { diff } => diff_paths(diff).join(", "),
            AgentEvent::ToolStarted { name, detail, .. } if !detail.is_empty() => {
                format!("{name}: {}", first_line(detail))
            }
            AgentEvent::ToolStarted { name, .. } => name.clone(),
            AgentEvent::ToolCompleted {
                name,
                detail,
                status,
                ..
            } if !detail.is_empty() => {
                format!("{name}: {} ({status})", first_line(detail))
            }
            AgentEvent::ToolCompleted { name, status, .. } => format!("{name} ({status})"),
            AgentEvent::PlanUpdated { text } | AgentEvent::AgentMessage { text } => {
                first_line(text)
            }
            AgentEvent::Thinking { text, tokens } => match (first_line(text), tokens) {
                (line, Some(tokens)) if line.is_empty() => format!("thinking · {tokens} tokens"),
                (line, Some(tokens)) => format!("{line} · {tokens} tokens"),
                (line, None) => line,
            },
            AgentEvent::Error { message } => first_line(message),
            // An arrow marks what moved; a bare parenthetical says what did
            // not, so the line is read at a glance rather than parsed.
            AgentEvent::ModelChanged { model, effort } => match (model, effort) {
                (Some(model), Some(effort)) => format!("model → {model} · effort → {effort}"),
                (Some(model), None) => format!("model → {model} (same effort)"),
                (None, Some(effort)) => format!("effort → {effort} (same model)"),
                // Not emitted; a selection that changed nothing is not an event.
                (None, None) => "selection unchanged".into(),
            },
            AgentEvent::TaskStarted {
                description, agent, ..
            } => match agent {
                Some(agent) => format!("{agent}: {} (running)", first_line(description)),
                None => format!("{} (running)", first_line(description)),
            },
            AgentEvent::TaskProgress {
                description,
                agent,
                tokens,
                ..
            } => {
                let line = match agent {
                    Some(agent) => format!("{agent}: {}", first_line(description)),
                    None => first_line(description),
                };
                match tokens {
                    Some(tokens) => format!("{line} · {tokens} tokens"),
                    None => line,
                }
            }
            AgentEvent::TaskCompleted {
                id,
                status,
                summary,
                ..
            } if summary.is_empty() => format!("task {id} ({status})"),
            AgentEvent::TaskCompleted {
                status, summary, ..
            } => format!("{} ({status})", first_line(summary)),
            AgentEvent::BackgroundTasks { running } => match running {
                0 => "no background tasks running".into(),
                1 => "1 background task running".into(),
                many => format!("{many} background tasks running"),
            },
            AgentEvent::Unknown { wire_type } => wire_type.clone(),
            AgentEvent::Malformed { error } => first_line(error),
        };
        truncate_line(&line, 200)
    }

    /// The expandable detail body as escape-free structured blocks.
    pub fn detail(&self) -> Vec<DetailBlock> {
        match self {
            AgentEvent::UserMessage { text } => markdown_blocks(text),
            AgentEvent::ThreadStarted {
                thread_id,
                model,
                effort,
            } => {
                let mut lines = vec![format!("thread id: {thread_id}")];
                if let Some(model) = model {
                    lines.push(format!("model: {model}"));
                }
                if let Some(effort) = effort {
                    lines.push(format!("reasoning effort: {effort}"));
                }
                vec![DetailBlock::Text(lines.join("\n"))]
            }
            AgentEvent::TurnStarted => Vec::new(),
            AgentEvent::TurnCompleted { usage } | AgentEvent::UsageUpdated { usage } => {
                vec![DetailBlock::Text(format!(
                    "input {} · cached input {} · output {} · reasoning {}",
                    usage.input_tokens,
                    usage.cached_input_tokens,
                    usage.output_tokens,
                    usage.reasoning_output_tokens
                ))]
            }
            AgentEvent::CommandStarted { command } => {
                vec![DetailBlock::Code {
                    language: None,
                    text: command.clone(),
                }]
            }
            AgentEvent::CommandCompleted {
                command,
                status,
                exit_code,
                output,
            } => {
                let mut blocks = vec![DetailBlock::Text(match exit_code {
                    Some(code) => format!("$ {command}\nstatus: {status} (exit {code})"),
                    None => format!("$ {command}\nstatus: {status}"),
                })];
                if !output.is_empty() {
                    blocks.push(DetailBlock::Code {
                        language: None,
                        text: output.clone(),
                    });
                }
                blocks
            }
            AgentEvent::FileChanged { paths, diff, .. } => {
                let mut blocks = vec![DetailBlock::Text(paths.join("\n"))];
                if let Some(diff) = diff {
                    if !diff.is_empty() {
                        blocks.push(DetailBlock::Code {
                            language: None,
                            text: diff.clone(),
                        });
                    }
                }
                blocks
            }
            AgentEvent::DiffUpdated { diff } => vec![DetailBlock::Code {
                language: None,
                text: diff.clone(),
            }],
            AgentEvent::ToolStarted { name, detail, .. } => {
                let mut text = name.clone();
                if !detail.is_empty() {
                    text.push('\n');
                    text.push_str(detail);
                }
                vec![DetailBlock::Text(text)]
            }
            AgentEvent::ToolCompleted {
                name,
                detail,
                status,
                output,
                ..
            } => {
                let mut text = format!("{name}: {status}");
                if !detail.is_empty() {
                    text.push('\n');
                    text.push_str(detail);
                }
                let mut blocks = vec![DetailBlock::Text(text)];
                if !output.is_empty() {
                    blocks.push(DetailBlock::Code {
                        language: None,
                        text: output.clone(),
                    });
                }
                blocks
            }
            AgentEvent::PlanUpdated { text } | AgentEvent::AgentMessage { text } => {
                markdown_blocks(text)
            }
            AgentEvent::Thinking { text, tokens } => match (text.is_empty(), tokens) {
                (true, Some(tokens)) => {
                    vec![DetailBlock::Text(format!("{tokens} thinking tokens"))]
                }
                _ => markdown_blocks(text),
            },
            AgentEvent::Error { message } => vec![DetailBlock::Text(message.clone())],
            AgentEvent::ModelChanged { model, effort } => {
                // Name what did *not* change too: the detail is where an
                // operator checks what the session is now running under, and
                // a missing line reads as an omission rather than as "same as
                // before".
                let mut lines = Vec::new();
                match model {
                    Some(model) => lines.push(format!("model: {model}")),
                    None => lines.push("model: unchanged".into()),
                }
                match effort {
                    Some(effort) => lines.push(format!("reasoning effort: {effort}")),
                    None => lines.push("reasoning effort: unchanged".into()),
                }
                vec![DetailBlock::Text(lines.join("\n"))]
            }
            AgentEvent::TaskStarted {
                id,
                description,
                kind,
                agent,
            } => {
                let mut lines = vec![description.clone(), format!("task id: {id}")];
                lines.push(format!("kind: {kind}"));
                if let Some(agent) = agent {
                    lines.push(format!("agent: {agent}"));
                }
                vec![DetailBlock::Text(lines.join("\n"))]
            }
            AgentEvent::TaskProgress {
                id,
                description,
                agent,
                tool,
                tokens,
            } => {
                let mut lines = vec![description.clone(), format!("task id: {id}")];
                if let Some(agent) = agent {
                    lines.push(format!("agent: {agent}"));
                }
                if let Some(tool) = tool {
                    lines.push(format!("last tool: {tool}"));
                }
                if let Some(tokens) = tokens {
                    lines.push(format!("tokens: {tokens}"));
                }
                vec![DetailBlock::Text(lines.join("\n"))]
            }
            AgentEvent::TaskCompleted {
                id,
                status,
                summary,
                error,
            } => {
                let mut lines = Vec::new();
                if !summary.is_empty() {
                    lines.push(summary.clone());
                }
                lines.push(format!("task id: {id}"));
                lines.push(format!("status: {status}"));
                if let Some(error) = error {
                    lines.push(error.clone());
                }
                vec![DetailBlock::Text(lines.join("\n"))]
            }
            AgentEvent::BackgroundTasks { .. } => vec![DetailBlock::Text(self.summary())],
            AgentEvent::Unknown { wire_type } => {
                vec![DetailBlock::Text(format!(
                    "unrecognised event: {wire_type}"
                ))]
            }
            AgentEvent::Malformed { error } => vec![DetailBlock::Text(error.clone())],
        }
    }

    /// Provider-independent presentation shared by all provider presenters.
    pub(crate) fn presented_detail_default(&self, mode: PresentationMode) -> Vec<DetailBlock> {
        if mode == PresentationMode::Raw {
            return self.detail();
        }
        match self {
            AgentEvent::FileChanged { paths, diff, .. } => {
                let mut blocks = vec![DetailBlock::Text(paths.join("\n"))];
                if let Some(diff) = diff {
                    let minimal = minimal_diff(diff);
                    if !minimal.is_empty() {
                        blocks.push(DetailBlock::Code {
                            language: Some("diff".into()),
                            text: minimal,
                        });
                    }
                }
                blocks
            }
            AgentEvent::DiffUpdated { diff } => vec![DetailBlock::Code {
                language: Some("diff".into()),
                text: minimal_diff(diff),
            }],
            _ => self.detail(),
        }
    }
}

pub(crate) fn truncate_summary(line: &str) -> String {
    truncate_line(line, 200)
}

fn minimal_diff(diff: &str) -> String {
    diff.lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decode one wire line under the given protocol. Never fails: undecodable
/// input becomes [`AgentEvent::Malformed`] so nothing is silently lost.
pub fn decode_line(protocol: Protocol, line: &str) -> AgentEvent {
    match protocol {
        Protocol::CodexJsonl => decode_codex_line(line),
        Protocol::CodexAppServer => decode_appserver_line(line),
        Protocol::ClaudeJsonl => decode_claude_line(line),
    }
}

/// Decode one `codex app-server` line. Notifications (which carry a `method`)
/// map to events; requests and responses are control traffic and decode to
/// [`AgentEvent::Unknown`] so they are carried without cluttering the list.
///
/// The one response that is not merely control traffic is the reply to
/// `thread/start`: it is where the app-server states the model and reasoning
/// effort the thread actually runs on, which no notification repeats. Decoding
/// it here rather than only in [`crate::appserver`] keeps a replayed journal
/// showing the same thing a live session does.
fn decode_appserver_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    match string(&value, "method") {
        Some(method) => {
            decode_appserver_notification(method, value.get("params").unwrap_or(&Value::Null))
        }
        None => match value.get("result").and_then(decode_thread_start_result) {
            Some(event) => event,
            None => AgentEvent::Unknown {
                wire_type: "response".into(),
            },
        },
    }
}

/// A `thread/start` result as a [`AgentEvent::ThreadStarted`], recognised by the
/// thread it reports rather than by the request id it answers — the id belongs
/// to the client that asked, not to the wire format, so a journal read back
/// later does not depend on it.
fn decode_thread_start_result(result: &Value) -> Option<AgentEvent> {
    let thread_id = result
        .get("thread")
        .and_then(|thread| string(thread, "id"))?;
    Some(AgentEvent::ThreadStarted {
        thread_id: clean_terminal_text(thread_id),
        model: string(result, "model").map(clean_terminal_text),
        effort: string(result, "reasoningEffort").map(clean_terminal_text),
    })
}

fn decode_appserver_notification(method: &str, params: &Value) -> AgentEvent {
    match method {
        "thread/started" => AgentEvent::ThreadStarted {
            thread_id: clean_terminal_text(
                params
                    .get("thread")
                    .and_then(|thread| string(thread, "id"))
                    .unwrap_or_default(),
            ),
            // The notification announces only the thread; the model and effort
            // arrive in the `thread/start` response (see
            // `decode_thread_start_result`).
            model: None,
            effort: None,
        },
        "turn/started" => AgentEvent::TurnStarted,
        // `turn/completed` is the actual end-of-turn signal; it carries no
        // usage figures of its own.
        "turn/completed" => AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        },
        // Fires after every step within a turn (each tool call, each model
        // round), not just the last one, so it must not be treated as
        // end-of-turn — that previously made the UI's running/waiting
        // indicator flip idle mid-turn. It only refreshes the usage display.
        "thread/tokenUsage/updated" => AgentEvent::UsageUpdated {
            usage: appserver_usage(params),
        },
        "turn/diff/updated" => AgentEvent::DiffUpdated {
            diff: clean_terminal_text(string(params, "diff").unwrap_or_default()),
        },
        "turn/plan/updated" => AgentEvent::PlanUpdated {
            text: appserver_plan_text(params),
        },
        "item/started" => decode_appserver_item(params.get("item").unwrap_or(&Value::Null), false),
        "item/completed" => decode_appserver_item(params.get("item").unwrap_or(&Value::Null), true),
        "error" | "warning" | "guardianWarning" | "configWarning" => AgentEvent::Error {
            message: clean_terminal_text(&error_message(params)),
        },
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_appserver_item(item: &Value, completed: bool) -> AgentEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("agentMessage", true) => AgentEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("commandExecution", false) => AgentEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("commandExecution", true) => AgentEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exitCode").and_then(Value::as_i64),
            output: clean(string(item, "aggregatedOutput").unwrap_or_default()),
        },
        ("fileChange", true) => AgentEvent::FileChanged {
            id: clean(string(item, "id").unwrap_or_default()),
            paths: changed_paths(item),
            diff: changes_diff(item),
            checkpoint: None,
            checkpoint_error: None,
        },
        ("plan", true) => AgentEvent::PlanUpdated {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("mcpToolCall", false) | ("webSearch", false) => AgentEvent::ToolStarted {
            id: clean(string(item, "id").unwrap_or_default()),
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcpToolCall", true) | ("webSearch", true) => AgentEvent::ToolCompleted {
            id: clean(string(item, "id").unwrap_or_default()),
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
            status: clean(string(item, "status").unwrap_or("completed")),
            output: clean(tool_output(item)),
        },
        // userMessage (echoed back — the host shows its own), reasoning, deltas,
        // and item lifecycles with no view carry without rendering.
        _ => AgentEvent::Unknown {
            wire_type: format!("item:{kind}"),
        },
    }
}

fn appserver_usage(params: &Value) -> TokenUsage {
    let total = params
        .get("tokenUsage")
        .and_then(|usage| usage.get("total"))
        .unwrap_or(&Value::Null);
    let field = |key: &str| total.get(key).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: field("inputTokens"),
        cached_input_tokens: field("cachedInputTokens"),
        output_tokens: field("outputTokens"),
        reasoning_output_tokens: field("reasoningOutputTokens"),
    }
}

/// Render the app-server's structured turn plan into the provider-independent
/// text representation used by [`AgentEvent::PlanUpdated`]. Keep the status
/// names explicit: unlike a checkbox, that preserves the distinction between
/// a pending step and the one currently in progress.
fn appserver_plan_text(params: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(explanation) = string(params, "explanation") {
        let explanation = clean_terminal_text(explanation);
        if !explanation.is_empty() {
            parts.push(explanation);
        }
    }

    let steps = params
        .get("plan")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let step = clean_terminal_text(string(entry, "step")?);
            if step.is_empty() {
                return None;
            }
            let status = clean_terminal_text(string(entry, "status").unwrap_or("pending"));
            Some(format!("- [{status}] {step}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !steps.is_empty() {
        parts.push(steps);
    }

    parts.join("\n\n")
}

fn decode_codex_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    decode_codex_value(&value)
}

fn decode_codex_value(value: &Value) -> AgentEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        // A one-shot `codex exec` run states neither model nor effort on the
        // wire; both stay as the launch asked for them.
        "thread.started" => AgentEvent::ThreadStarted {
            thread_id: clean_terminal_text(string(value, "thread_id").unwrap_or_default()),
            model: None,
            effort: None,
        },
        "turn.started" => AgentEvent::TurnStarted,
        "turn.completed" => AgentEvent::TurnCompleted {
            usage: value
                .get("usage")
                .and_then(|usage| serde_json::from_value(usage.clone()).ok())
                .unwrap_or_default(),
        },
        "turn.failed" | "error" => AgentEvent::Error {
            message: clean_terminal_text(&error_message(value)),
        },
        "item.started" | "item.updated" | "item.completed" => {
            decode_codex_item(wire_type, value.get("item").unwrap_or(&Value::Null))
        }
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

fn decode_codex_item(event_type: &str, item: &Value) -> AgentEvent {
    let kind = string(item, "type").unwrap_or("unknown");
    let completed = event_type == "item.completed";
    let clean = |value: &str| clean_terminal_text(value);
    match (kind, completed) {
        ("command_execution", false) => AgentEvent::CommandStarted {
            command: clean(string(item, "command").unwrap_or_default()),
        },
        ("command_execution", true) => AgentEvent::CommandCompleted {
            command: clean(string(item, "command").unwrap_or_default()),
            status: clean(string(item, "status").unwrap_or("completed")),
            exit_code: item.get("exit_code").and_then(Value::as_i64),
            output: clean(
                string(item, "aggregated_output")
                    .or_else(|| string(item, "output"))
                    .unwrap_or_default(),
            ),
        },
        ("file_change", true) => AgentEvent::FileChanged {
            id: clean(string(item, "id").unwrap_or_default()),
            paths: changed_paths(item),
            diff: changes_diff(item),
            checkpoint: None,
            checkpoint_error: None,
        },
        ("agent_message", true) => AgentEvent::AgentMessage {
            text: clean(string(item, "text").unwrap_or_default()),
        },
        ("plan", true) | ("plan_update", true) => AgentEvent::PlanUpdated {
            text: clean(
                string(item, "text")
                    .or_else(|| string(item, "plan"))
                    .unwrap_or_default(),
            ),
        },
        ("mcp_tool_call", false) | ("web_search", false) => AgentEvent::ToolStarted {
            id: clean(string(item, "id").unwrap_or_default()),
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
        },
        ("mcp_tool_call", true) | ("web_search", true) => AgentEvent::ToolCompleted {
            id: clean(string(item, "id").unwrap_or_default()),
            name: clean(tool_name(item, kind)),
            detail: clean(tool_detail(item)),
            status: clean(string(item, "status").unwrap_or("completed")),
            output: clean(tool_output(item)),
        },
        (_, _) => AgentEvent::Unknown {
            wire_type: format!("{event_type}:{kind}"),
        },
    }
}

/// Decode one Claude Code `stream-json` line. Its schema differs from codex's:
/// a top-level `system` (session/init metadata), `assistant` and `user`
/// messages carrying Anthropic content blocks, and a `result` turn summary.
///
/// One wire line maps to one [`AgentEvent`], as in the codex decoder. An
/// `assistant` message may carry several content blocks; the salient one is
/// chosen (a tool call over prose, prose over reasoning) and the rest remain in
/// the verbatim raw view. NOTE: the exact `stream-json` shape must be confirmed
/// against the installed `claude` version; it is isolated here so adapting to a
/// revised contract is a localized change.
fn decode_claude_line(line: &str) -> AgentEvent {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return AgentEvent::Malformed {
                error: clean_terminal_text(&format!("{error}")),
            }
        }
    };
    decode_claude_value(&value)
}

fn decode_claude_value(value: &Value) -> AgentEvent {
    let wire_type = string(value, "type").unwrap_or("unknown");
    match wire_type {
        "system" => {
            let subtype = string(value, "subtype").unwrap_or_default();
            if subtype == "init" {
                AgentEvent::ThreadStarted {
                    thread_id: clean_terminal_text(string(value, "session_id").unwrap_or_default()),
                    // Claude Code's init line names the model it resolved, but
                    // no effort level, so the launch's own remains all Styra
                    // knows of that.
                    model: string(value, "model").map(clean_terminal_text),
                    effort: None,
                }
            } else if subtype == "thinking_tokens" {
                // Claude reports its extended-thinking token spend on its own
                // line, repeatedly, with no prose. Field naming has varied
                // across releases, so take whichever count is present; a line
                // with none stays an unrecognised system event.
                match claude_thinking_tokens(value) {
                    Some(tokens) => AgentEvent::Thinking {
                        text: String::new(),
                        tokens: Some(tokens),
                    },
                    None => AgentEvent::Unknown {
                        wire_type: "system:thinking_tokens".into(),
                    },
                }
            } else if let Some(event) = decode_claude_task(value, subtype) {
                event
            } else if subtype == "background_tasks_changed" {
                // The whole set of live background tasks, resent on every
                // change. Its length is the authoritative running count.
                AgentEvent::BackgroundTasks {
                    running: value
                        .get("tasks")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len),
                }
            } else {
                AgentEvent::Unknown {
                    wire_type: clean_terminal_text(&format!("system:{subtype}")),
                }
            }
        }
        "assistant" => decode_claude_assistant(value.get("message").unwrap_or(&Value::Null)),
        "user" => decode_claude_user(value.get("message").unwrap_or(&Value::Null)),
        "result" => decode_claude_result(value),
        other => AgentEvent::Unknown {
            wire_type: clean_terminal_text(other),
        },
    }
}

/// Choose the salient block of an assistant message: a tool call is the action
/// worth surfacing, then visible prose, then reasoning. The full message stays
/// available verbatim in the raw view.
/// Decode Claude's task lifecycle lines, or `None` when the subtype is not one
/// of them. Claude runs backgrounded commands and subagents as tasks and
/// reports each one's start, repeated progress, and end on its own `system`
/// lines, all keyed by `task_id`; `task_notification` and the `task_updated`
/// status patch both report the same ending, from different angles, and both
/// decode to `TaskCompleted` for the host to merge.
fn decode_claude_task(value: &Value, subtype: &str) -> Option<AgentEvent> {
    let id = || clean_terminal_text(string(value, "task_id").unwrap_or_default());
    let agent = || string(value, "subagent_type").map(clean_terminal_text);
    match subtype {
        "task_started" => Some(AgentEvent::TaskStarted {
            id: id(),
            description: clean_terminal_text(string(value, "description").unwrap_or_default()),
            kind: clean_terminal_text(string(value, "task_type").unwrap_or_default()),
            agent: agent(),
        }),
        "task_progress" => Some(AgentEvent::TaskProgress {
            id: id(),
            description: clean_terminal_text(string(value, "description").unwrap_or_default()),
            agent: agent(),
            tool: string(value, "last_tool_name").map(clean_terminal_text),
            tokens: value
                .get("usage")
                .and_then(|usage| usage.get("total_tokens"))
                .and_then(Value::as_u64),
        }),
        "task_notification" => Some(AgentEvent::TaskCompleted {
            id: id(),
            status: clean_terminal_text(string(value, "status").unwrap_or_default()),
            summary: clean_terminal_text(string(value, "summary").unwrap_or_default()),
            error: None,
        }),
        // A patch is whatever changed about the task. Only a status change is
        // an event in its own right; the rest (e.g. a command being moved to
        // the background) says nothing the task's own row does not show.
        "task_updated" => {
            let patch = value.get("patch")?;
            Some(AgentEvent::TaskCompleted {
                id: id(),
                status: clean_terminal_text(string(patch, "status")?),
                summary: String::new(),
                error: string(patch, "error").map(clean_terminal_text),
            })
        }
        _ => None,
    }
}

/// The thinking tokens a `system:thinking_tokens` line reports, under any of
/// the keys Claude has used for it.
///
/// The delta comes first because the count beside it is not a running total:
/// it restarts at each block of reasoning, so summing the whole turn's deltas
/// is the only way to a figure that only goes up. A release that reports a
/// count alone is read as that block's spend.
fn claude_thinking_tokens(value: &Value) -> Option<u64> {
    [
        "estimated_tokens_delta",
        "thinking_tokens_delta",
        "estimated_tokens",
        "thinking_tokens",
        "tokens",
        "count",
        "total_thinking_tokens",
    ]
    .iter()
    .find_map(|key| value.get(key).and_then(Value::as_u64))
}

fn decode_claude_assistant(message: &Value) -> AgentEvent {
    match message.get("content") {
        Some(Value::String(text)) => {
            return AgentEvent::AgentMessage {
                text: clean_terminal_text(text),
            }
        }
        Some(Value::Array(blocks)) => {
            let mut text = None;
            let mut thinking = None;
            for block in blocks {
                match string(block, "type") {
                    Some("tool_use") => return claude_tool_started(block),
                    Some("text") if text.is_none() => text = string(block, "text"),
                    Some("thinking") if thinking.is_none() => thinking = string(block, "thinking"),
                    _ => {}
                }
            }
            if let Some(text) = text {
                return AgentEvent::AgentMessage {
                    text: clean_terminal_text(text),
                };
            }
            if let Some(thinking) = thinking {
                return AgentEvent::Thinking {
                    text: clean_terminal_text(thinking),
                    tokens: None,
                };
            }
        }
        _ => {}
    }
    AgentEvent::Unknown {
        wire_type: "assistant".into(),
    }
}

fn claude_tool_started(block: &Value) -> AgentEvent {
    let name = string(block, "name").unwrap_or("tool");
    if matches!(name, "Edit" | "Write" | "MultiEdit") {
        if let Some(change) = claude_file_change(block, name) {
            return AgentEvent::FileChanged {
                id: clean_terminal_text(string(block, "id").unwrap_or_default()),
                paths: vec![change.path],
                diff: change.diff,
                checkpoint: None,
                checkpoint_error: None,
            };
        }
    }
    let detail = block
        .get("input")
        .filter(|input| !input.is_null())
        .map(|input| input.to_string())
        .unwrap_or_default();
    AgentEvent::ToolStarted {
        id: clean_terminal_text(string(block, "id").unwrap_or_default()),
        name: clean_terminal_text(string(block, "name").unwrap_or("tool")),
        detail: clean_terminal_text(&detail),
    }
}

/// Reconstruct the useful part of Claude's Edit/Write tool request. This is
/// intentionally best-effort: Edit has an exact old/new snippet, while Write
/// only gives us the replacement content and no original file contents.
fn claude_file_change(block: &Value, name: &str) -> Option<FileChange> {
    let input = block.get("input")?;
    let path = string(input, "file_path")
        .or_else(|| string(input, "path"))?
        .to_owned();
    let diff = match name {
        "Edit" => {
            let old = string(input, "old_string")?;
            let new = string(input, "new_string")?;
            Some(unified_snippet(old, new))
        }
        "MultiEdit" => input.get("edits").and_then(Value::as_array).map(|edits| {
            edits
                .iter()
                .filter_map(|edit| {
                    Some(unified_snippet(
                        string(edit, "old_string")?,
                        string(edit, "new_string")?,
                    ))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        "Write" => string(input, "content").map(|content| {
            let added = content
                .lines()
                .map(|line| format!("+{line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("@@ write {path} @@\n{added}")
        }),
        _ => None,
    }?;
    Some(FileChange {
        path,
        kind: name.to_lowercase(),
        diff: Some(diff),
    })
}

fn unified_snippet(old: &str, new: &str) -> String {
    let old = old
        .lines()
        .map(|line| format!("-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = new
        .lines()
        .map(|line| format!("+{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("@@ edit @@\n{old}\n{new}")
}

/// A Claude `user` message is a synthetic turn carrying tool results back to the
/// model; it is not an echo of the operator's input (the host records that itself).
/// Its `tool_result` block never repeats the tool's name — only the
/// `tool_use_id` from the matching `ToolStarted` — so `name` here is a
/// placeholder; the host resolves the real name by correlating on `id`.
fn decode_claude_user(message: &Value) -> AgentEvent {
    if let Some(Value::Array(blocks)) = message.get("content") {
        for block in blocks {
            if string(block, "type") == Some("tool_result") {
                let is_error = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let id = clean_terminal_text(string(block, "tool_use_id").unwrap_or("tool"));
                return AgentEvent::ToolCompleted {
                    id: id.clone(),
                    name: id,
                    detail: String::new(),
                    status: if is_error {
                        "error".into()
                    } else {
                        "completed".into()
                    },
                    output: clean_terminal_text(&claude_tool_result_text(block)),
                };
            }
        }
    }
    AgentEvent::Unknown {
        wire_type: "user".into(),
    }
}

/// A `tool_result` block's `content` is either a plain string or a list of
/// content blocks (mirroring assistant messages); either way, the stdout the
/// tool reported for its preview is the text within.
fn claude_tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| string(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn decode_claude_result(value: &Value) -> AgentEvent {
    let subtype = string(value, "subtype").unwrap_or_default();
    let is_error = value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_error || subtype.starts_with("error") {
        return AgentEvent::Error {
            message: clean_terminal_text(&error_message(value)),
        };
    }
    AgentEvent::TurnCompleted {
        usage: claude_usage(value.get("usage").unwrap_or(&Value::Null)),
    }
}

/// Map Claude's usage object onto [`TokenUsage`]. Claude reports cached
/// input as `cache_read_input_tokens`; the rest align by name.
fn claude_usage(usage: &Value) -> TokenUsage {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: count("input_tokens"),
        cached_input_tokens: count("cache_read_input_tokens"),
        output_tokens: count("output_tokens"),
        reasoning_output_tokens: 0,
    }
}

/// Collect the changed paths of a file-change item, tolerating the schema
/// variants seen across codex versions (`path` or `file_path` per change, or a
/// single path on the item itself).
fn changed_paths(item: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(changes) = item.get("changes").and_then(Value::as_array) {
        for change in changes {
            if let Some(path) = string(change, "path").or_else(|| string(change, "file_path")) {
                paths.push(clean_terminal_text(path));
            }
        }
    }
    if paths.is_empty() {
        if let Some(path) = string(item, "path").or_else(|| string(item, "file_path")) {
            paths.push(clean_terminal_text(path));
        }
    }
    paths
}

fn changes_diff(item: &Value) -> Option<String> {
    let changes = item.get("changes").and_then(Value::as_array)?;
    let diffs = changes
        .iter()
        .filter_map(|change| string(change, "diff"))
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>();
    (!diffs.is_empty()).then(|| clean_terminal_text(&diffs.join("\n")))
}

fn diff_paths(diff: &str) -> Vec<String> {
    diff.lines()
        .filter_map(|line| {
            line.strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("--- a/"))
        })
        .filter(|path| *path != "/dev/null")
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn tool_name<'a>(item: &'a Value, kind: &'a str) -> &'a str {
    string(item, "tool")
        .or_else(|| string(item, "name"))
        .or_else(|| string(item, "server"))
        .unwrap_or(kind)
}

fn tool_detail(item: &Value) -> &str {
    string(item, "query")
        .or_else(|| string(item, "arguments"))
        .or_else(|| string(item, "detail"))
        .unwrap_or_default()
}

fn tool_output(item: &Value) -> &str {
    string(item, "result")
        .or_else(|| string(item, "output"))
        .unwrap_or_default()
}

/// Keys that, across the three protocols, have been seen to carry the prose of
/// a failure. Ordered most specific first so `message` wins over a bare
/// `reason` when a payload carries both.
const ERROR_TEXT_KEYS: [&str; 6] = [
    "message",
    "error",
    "result",
    "detail",
    "reason",
    "description",
];

/// Keys whose value labels a failure without describing it (`subtype`,
/// `code`, …). Used only when no prose was found, so an operator still sees
/// *which* error instead of a generic sentence.
const ERROR_LABEL_KEYS: [&str; 4] = ["subtype", "code", "type", "status"];

/// Pull the human-readable text out of a failure payload.
///
/// The shape varies widely — codex nests it under `error`, the app-server puts
/// it on `params.message`, Claude on `result`, and some transports wrap it once
/// more (`error.error.message`, `data.message`). A payload that stated its
/// problem only in a nested object used to render as the generic "agent
/// reported an error", which hid real and actionable failures such as a
/// workspace running out of credits. So search a bounded depth for the first
/// key that carries prose, and fall back to a label rather than to nothing.
fn error_message(value: &Value) -> String {
    if let Some(text) = error_text(value, 4) {
        return text;
    }
    if let Some(label) = error_label(value) {
        return format!("agent reported an error ({label})");
    }
    "agent reported an error".to_owned()
}

/// First non-empty prose string reachable from `value` via the known error
/// keys, descending into nested objects up to `depth`.
fn error_text(value: &Value, depth: usize) -> Option<String> {
    let nonempty = |text: &str| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    if let Value::String(text) = value {
        return nonempty(text);
    }
    let object = value.as_object()?;
    if depth == 0 {
        return None;
    }
    for key in ERROR_TEXT_KEYS {
        if let Some(text) = object
            .get(key)
            .and_then(|child| error_text(child, depth - 1))
        {
            return Some(text);
        }
    }
    // A wrapper layer (`data`, `params`, `body`, …) that itself says nothing:
    // look through it for the keys above rather than giving up at its edge.
    for key in ["data", "params", "payload", "body", "response"] {
        if let Some(text) = object
            .get(key)
            .and_then(|child| error_text(child, depth - 1))
        {
            return Some(text);
        }
    }
    None
}

fn error_label(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    ERROR_LABEL_KEYS
        .iter()
        .find_map(|key| string(value, key).map(str::to_owned))
        .or_else(|| {
            object
                .get("error")
                .and_then(|error| ERROR_LABEL_KEYS.iter().find_map(|key| string(error, key)))
                .map(str::to_owned)
        })
        .map(|label| clean_terminal_text(&label))
        .filter(|label| !label.is_empty())
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn truncate_line(line: &str, max: usize) -> String {
    let flat: String = line
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let kept: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Strip ANSI escape sequences and stray control characters, keeping newlines
/// and expanding tabs. Provider text is presentation data, not a terminal to
/// replay, and a literal tab would move the physical terminal cursor without
/// Ratatui's virtual buffer accounting for it.
pub fn clean_terminal_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ ... final byte in 0x40..=0x7e
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ST (ESC \)
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{07}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other escape: drop ESC and the single following byte.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\n' => out.push(ch),
            '\t' => out.push_str("    "),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Split fenced Markdown into prose and code blocks, keeping the fence's
/// language for the renderer. Ported in spirit from Orka's work-log renderer.
pub fn markdown_blocks(markdown: &str) -> Vec<DetailBlock> {
    let markdown = clean_terminal_text(markdown);
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut code = String::new();
    let mut fence: Option<(char, usize, Option<String>)> = None;

    for line in markdown.split_inclusive('\n') {
        let candidate = line.trim_end_matches(['\r', '\n']);
        if let Some((marker, width, language)) = &fence {
            if closing_fence(candidate, *marker, *width) {
                blocks.push(DetailBlock::Code {
                    language: language.clone(),
                    text: std::mem::take(&mut code),
                });
                fence = None;
            } else {
                code.push_str(line);
            }
            continue;
        }
        if let Some(opening) = opening_fence(candidate) {
            if !prose.is_empty() {
                blocks.push(DetailBlock::Text(
                    std::mem::take(&mut prose).trim_end().to_owned(),
                ));
            }
            fence = Some(opening);
        } else {
            prose.push_str(line);
        }
    }
    if let Some((_, _, language)) = fence {
        blocks.push(DetailBlock::Code {
            language,
            text: code,
        });
    }
    if !prose.is_empty() {
        blocks.push(DetailBlock::Text(prose.trim_end().to_owned()));
    }
    if blocks.is_empty() && !markdown.is_empty() {
        blocks.push(DetailBlock::Text(markdown));
    }
    blocks
}

fn opening_fence(line: &str) -> Option<(char, usize, Option<String>)> {
    let line = line.trim_start_matches(' ');
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let width = line.chars().take_while(|ch| *ch == marker).count();
    if width < 3 {
        return None;
    }
    let info = line[width..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    let language = info
        .split_whitespace()
        .next()
        .filter(|language| !language.is_empty())
        .map(str::to_owned);
    Some((marker, width, language))
}

fn closing_fence(line: &str, marker: char, width: usize) -> bool {
    let line = line.trim_start_matches(' ');
    if line.chars().count() < width || !line.chars().take(width).all(|ch| ch == marker) {
        return false;
    }
    line.chars().skip(width).all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_message_decodes_and_summarises_to_one_line() {
        let event = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Added backoff.\nTests pass."}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::AgentMessage {
                text: "Added backoff.\nTests pass.".into()
            }
        );
        assert_eq!(event.tag(), "agent");
        assert_eq!(event.summary(), "Added backoff.");
    }

    #[test]
    fn command_lifecycle_decodes_with_status_and_output() {
        let started = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"cargo test"}}"#,
        );
        assert_eq!(
            started,
            AgentEvent::CommandStarted {
                command: "cargo test".into()
            }
        );

        let completed = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"cargo test","status":"completed","exit_code":0,"aggregated_output":"ok"}}"#,
        );
        assert_eq!(
            completed,
            AgentEvent::CommandCompleted {
                command: "cargo test".into(),
                status: "completed".into(),
                exit_code: Some(0),
                output: "ok".into(),
            }
        );
        assert_eq!(completed.summary(), "cargo test (completed, exit 0)");
    }

    #[test]
    fn thread_and_turn_events_decode() {
        assert_eq!(
            decode_line(
                Protocol::CodexJsonl,
                r#"{"type":"thread.started","thread_id":"t-7"}"#
            ),
            AgentEvent::ThreadStarted {
                thread_id: "t-7".into(),
                model: None,
                effort: None
            }
        );
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"turn.started"}"#),
            AgentEvent::TurnStarted
        );
        let usage = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":3,"cached_input_tokens":2}}"#,
        );
        assert_eq!(
            usage,
            AgentEvent::TurnCompleted {
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 3,
                    cached_input_tokens: 2,
                    reasoning_output_tokens: 0,
                }
            }
        );
        assert_eq!(usage.summary(), "in 10 · out 3 · cached 2");
    }

    #[test]
    fn file_change_collects_paths() {
        let event = decode_line(
            Protocol::CodexJsonl,
            r#"{"type":"item.completed","item":{"id":"f1","type":"file_change","changes":[{"path":"src/a.rs"},{"path":"src/b.rs"}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::FileChanged {
                id: "f1".into(),
                paths: vec!["src/a.rs".into(), "src/b.rs".into()],
                diff: None,
                checkpoint: None,
                checkpoint_error: None,
            }
        );
        assert_eq!(event.summary(), "src/a.rs, src/b.rs");
    }

    #[test]
    fn appserver_aggregated_diff_is_renderable_without_git() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"turn/diff/updated","params":{"threadId":"t","turnId":"u","diff":"diff --git a/src/a.rs b/src/a.rs\n+++ b/src/a.rs\n@@\n-old\n+new\n"}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::DiffUpdated {
                diff: "diff --git a/src/a.rs b/src/a.rs\n+++ b/src/a.rs\n@@\n-old\n+new\n".into()
            }
        );
        assert_eq!(event.summary(), "src/a.rs");
        assert!(matches!(event.detail()[0], DetailBlock::Code { .. }));
    }

    #[test]
    fn appserver_turn_plan_update_preserves_explanation_steps_and_statuses() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"turn/plan/updated","params":{"turnId":"u","explanation":"Adjusted after inspection.","plan":[{"step":"Read the code","status":"completed"},{"step":"Implement the fix","status":"inProgress"},{"step":"Run tests","status":"pending"}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::PlanUpdated {
                text: "Adjusted after inspection.\n\n- [completed] Read the code\n- [inProgress] Implement the fix\n- [pending] Run tests".into()
            }
        );
        assert_eq!(event.summary(), "Adjusted after inspection.");
    }

    #[test]
    fn appserver_turn_plan_update_without_explanation_uses_first_step_as_summary() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"turn/plan/updated","params":{"turnId":"u","plan":[{"step":"Inspect\u001b[31m code","status":"inProgress"}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::PlanUpdated {
                text: "- [inProgress] Inspect code".into()
            }
        );
        assert_eq!(event.summary(), "- [inProgress] Inspect code");
    }

    #[test]
    fn unknown_and_malformed_are_preserved_not_dropped() {
        assert_eq!(
            decode_line(Protocol::CodexJsonl, r#"{"type":"future.event"}"#),
            AgentEvent::Unknown {
                wire_type: "future.event".into()
            }
        );
        assert!(matches!(
            decode_line(Protocol::CodexJsonl, "not json"),
            AgentEvent::Malformed { .. }
        ));
    }

    #[test]
    fn terminal_escapes_are_stripped_from_decoded_text() {
        // Valid JSON escapes the ESC byte as \u001b, as real agent output does.
        let event = decode_line(
            Protocol::CodexJsonl,
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"\\u001b[31mred\\u001b[0m done\"}}",
        );
        assert_eq!(
            event,
            AgentEvent::AgentMessage {
                text: "red done".into()
            }
        );
    }

    #[test]
    fn terminal_tabs_are_expanded_before_rendering() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"turn/diff/updated","params":{"diff":"@@\n-\told\n+\tnew"}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::DiffUpdated {
                diff: "@@\n-    old\n+    new".into()
            }
        );
        assert!(!event.detail().iter().any(|block| match block {
            DetailBlock::Text(text) | DetailBlock::Code { text, .. } => text.contains('\t'),
        }));
    }

    #[test]
    fn markdown_detail_separates_prose_and_code() {
        let blocks = markdown_blocks("before\n```rust\nfn main() {}\n```\nafter");
        assert_eq!(
            blocks,
            vec![
                DetailBlock::Text("before".into()),
                DetailBlock::Code {
                    language: Some("rust".into()),
                    text: "fn main() {}\n".into()
                },
                DetailBlock::Text("after".into()),
            ]
        );
    }

    #[test]
    fn agent_message_detail_uses_markdown_blocks() {
        let event = AgentEvent::AgentMessage {
            text: "text\n```\ncode\n```".into(),
        };
        assert_eq!(
            event.detail(),
            vec![
                DetailBlock::Text("text".into()),
                DetailBlock::Code {
                    language: None,
                    text: "code\n".into()
                },
            ]
        );
    }

    #[test]
    fn claude_init_starts_a_thread_and_other_system_lines_are_unknown() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"init","session_id":"s-9","model":"claude-opus-4-8"}"#,
            ),
            AgentEvent::ThreadStarted {
                thread_id: "s-9".into(),
                model: Some("claude-opus-4-8".into()),
                effort: None,
            }
        );
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"compact_boundary"}"#,
        );
        assert_eq!(
            event,
            AgentEvent::Unknown {
                wire_type: "system:compact_boundary".into()
            }
        );
        assert!(event.is_minor());
    }

    #[test]
    fn claude_background_task_set_carries_the_running_count() {
        let started = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bk8","task_type":"local_bash","description":"Compile project with sbt"}]}"#,
        );
        assert_eq!(started, AgentEvent::BackgroundTasks { running: 1 });
        assert_eq!(started.background_tasks_running(), Some(1));
        assert!(started.is_minor());
        assert_eq!(started.summary(), "1 background task running");

        let drained = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#,
        );
        assert_eq!(drained, AgentEvent::BackgroundTasks { running: 0 });
        assert_eq!(drained.background_tasks_running(), Some(0));
        assert_eq!(drained.summary(), "no background tasks running");
    }

    #[test]
    fn claude_bash_result_wording_is_not_trusted_to_end_a_background_task() {
        // Claude's own completion notice reads "[exited with code 0]", which
        // none of the heuristic phrases match. The reported task set is what
        // clients must rely on.
        let completed = AgentEvent::ToolCompleted {
            id: "t-1".into(),
            name: "t-1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "[exited with code 0]".into(),
        };
        assert!(!completed.finishes_background_task());
        assert_eq!(completed.background_tasks_running(), None);
    }

    #[test]
    fn claude_task_lifecycle_decodes_to_one_task_id() {
        let started = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"task_started","task_id":"t-1","tool_use_id":"toolu_1","description":"Rebuild and run tests","task_type":"local_bash"}"#,
        );
        assert_eq!(
            started,
            AgentEvent::TaskStarted {
                id: "t-1".into(),
                description: "Rebuild and run tests".into(),
                kind: "local_bash".into(),
                agent: None,
            }
        );
        let progress = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"task_progress","task_id":"t-1","description":"Running cargo test","subagent_type":"Explore","last_tool_name":"Bash","usage":{"total_tokens":10469}}"#,
        );
        assert_eq!(
            progress,
            AgentEvent::TaskProgress {
                id: "t-1".into(),
                description: "Running cargo test".into(),
                agent: Some("Explore".into()),
                tool: Some("Bash".into()),
                tokens: Some(10469),
            }
        );
        let ended = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"task_notification","task_id":"t-1","status":"completed","summary":"Rebuild and run tests","output_file":""}"#,
        );
        assert_eq!(
            ended,
            AgentEvent::TaskCompleted {
                id: "t-1".into(),
                status: "completed".into(),
                summary: "Rebuild and run tests".into(),
                error: None,
            }
        );
        assert!([&started, &progress, &ended]
            .iter()
            .all(|event| event.task_id() == Some("t-1")));
    }

    #[test]
    fn claude_task_update_ends_a_task_only_when_it_patches_the_status() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"task_updated","task_id":"t-2","patch":{"status":"failed","end_time":1,"error":"terminated early"}}"#,
            ),
            AgentEvent::TaskCompleted {
                id: "t-2".into(),
                status: "failed".into(),
                summary: String::new(),
                error: Some("terminated early".into()),
            }
        );
        // Backgrounding a running command changes nothing the task's own row
        // does not already show.
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"task_updated","task_id":"t-2","patch":{"is_backgrounded":true}}"#,
            ),
            AgentEvent::Unknown {
                wire_type: "system:task_updated".into()
            }
        );
    }

    #[test]
    fn claude_system_thinking_tokens_carries_what_this_update_spent() {
        // The delta, not the count beside it: that count restarts at each
        // block of reasoning, so only the deltas add up across a turn.
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":150,"estimated_tokens_delta":100}"#,
        );
        assert_eq!(
            event,
            AgentEvent::Thinking {
                text: String::new(),
                tokens: Some(100),
            }
        );
        assert!(event.is_minor());
        assert!(event.updates_thinking());
        assert_eq!(event.summary(), "thinking · 100 tokens");

        // A release that reports only a count is read as that block's spend.
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"system","subtype":"thinking_tokens","thinking_tokens":1280}"#,
            ),
            AgentEvent::Thinking {
                text: String::new(),
                tokens: Some(1280),
            }
        );
    }

    #[test]
    fn claude_system_thinking_tokens_is_minor() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"system","subtype":"thinking_tokens"}"#,
        );
        assert_eq!(
            event,
            AgentEvent::Unknown {
                wire_type: "system:thinking_tokens".into()
            }
        );
        assert!(event.is_minor());
    }

    #[test]
    fn claude_assistant_text_decodes_to_an_agent_message() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Added backoff.\nTests pass."}]},"session_id":"s"}"#,
        );
        assert_eq!(
            event,
            AgentEvent::AgentMessage {
                text: "Added backoff.\nTests pass.".into()
            }
        );
        assert_eq!(event.summary(), "Added backoff.");
    }

    #[test]
    fn claude_tool_use_is_preferred_over_prose_in_the_same_message() {
        // A tool call is the action worth surfacing; the preamble text stays in
        // the verbatim raw view.
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Running the tests."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::ToolStarted {
                id: "toolu_1".into(),
                name: "Bash".into(),
                detail: "{\"command\":\"cargo test\"}".into()
            }
        );
        assert_eq!(event.summary(), "Bash: {\"command\":\"cargo test\"}");
    }

    #[test]
    fn claude_background_bash_is_detected_until_task_poll_finishes() {
        let started = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"cargo test","run_in_background":true}}]}}"#,
        );
        assert!(started.starts_background_task());
        assert!(!started.finishes_background_task());

        let completed = AgentEvent::ToolCompleted {
            id: "poll-1".into(),
            name: "poll-1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "Task completed successfully".into(),
        };
        assert!(completed.finishes_background_task());
    }

    #[test]
    fn claude_edit_captures_a_best_effort_change_without_git() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"old line","new_string":"new line"}}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::FileChanged {
                id: "tool-1".into(),
                paths: vec!["src/lib.rs".into()],
                diff: Some("@@ edit @@\n-old line\n+new line".into()),
                checkpoint: None,
                checkpoint_error: None,
            }
        );
    }

    #[test]
    fn claude_thinking_only_message_falls_back_to_a_minor_thinking_event() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"weigh the options"}]}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::Thinking {
                text: "weigh the options".into(),
                tokens: None,
            }
        );
        assert!(event.is_minor());
    }

    #[test]
    fn claude_rate_limit_event_is_minor() {
        let event = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"rate_limit_event","rate_limit":{"status":"ok"}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::Unknown {
                wire_type: "rate_limit_event".into()
            }
        );
        assert!(event.is_minor());
    }

    #[test]
    fn claude_tool_result_completes_a_tool_with_its_status() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
            ),
            AgentEvent::ToolCompleted {
                id: "toolu_1".into(),
                name: "toolu_1".into(),
                detail: String::new(),
                status: "completed".into(),
                output: "ok".into()
            }
        );
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","is_error":true,"content":"boom"}]}}"#,
            ),
            AgentEvent::ToolCompleted {
                id: "toolu_2".into(),
                name: "toolu_2".into(),
                detail: String::new(),
                status: "error".into(),
                output: "boom".into()
            }
        );
    }

    #[test]
    fn claude_result_carries_usage_or_surfaces_an_error() {
        let usage = decode_line(
            Protocol::ClaudeJsonl,
            r#"{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":12,"output_tokens":3,"cache_read_input_tokens":8}}"#,
        );
        assert_eq!(
            usage,
            AgentEvent::TurnCompleted {
                usage: TokenUsage {
                    input_tokens: 12,
                    cached_input_tokens: 8,
                    output_tokens: 3,
                    reasoning_output_tokens: 0,
                }
            }
        );
        assert_eq!(usage.summary(), "in 12 · out 3 · cached 8");

        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"hit the turn limit"}"#,
            ),
            AgentEvent::Error {
                message: "hit the turn limit".into()
            }
        );
    }

    #[test]
    fn nested_error_prose_survives_decoding() {
        // The app-server states some failures only inside a nested `error`
        // object; the prose there is what the operator needs to see.
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"method":"error","params":{"error":{"code":"out_of_credits","message":"This workspace is out of credits."}}}"#,
            ),
            AgentEvent::Error {
                message: "This workspace is out of credits.".into()
            }
        );

        assert_eq!(
            decode_line(
                Protocol::CodexJsonl,
                r#"{"type":"turn.failed","error":{"error":{"message":"You have run out of credits."}}}"#,
            ),
            AgentEvent::Error {
                message: "You have run out of credits.".into()
            }
        );
    }

    #[test]
    fn an_error_without_prose_names_itself() {
        assert_eq!(
            decode_line(
                Protocol::ClaudeJsonl,
                r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
            ),
            AgentEvent::Error {
                message: "agent reported an error (error_during_execution)".into()
            }
        );
    }

    #[test]
    fn a_model_change_says_which_settings_moved() {
        let changed = |model: Option<&str>, effort: Option<&str>| AgentEvent::ModelChanged {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
        };

        assert_eq!(
            changed(Some("gpt-5"), Some("high")).summary(),
            "model → gpt-5 · effort → high"
        );
        // The operator switched model only; the line has to say the effort
        // stayed as it was rather than leaving it unmentioned.
        assert_eq!(
            changed(Some("gpt-5"), None).summary(),
            "model → gpt-5 (same effort)"
        );
        assert_eq!(
            changed(None, Some("low")).summary(),
            "effort → low (same model)"
        );
        assert_eq!(
            changed(Some("gpt-5"), None).detail(),
            vec![DetailBlock::Text(
                "model: gpt-5\nreasoning effort: unchanged".into()
            )]
        );
    }

    #[test]
    fn claude_unknown_and_malformed_are_preserved() {
        assert_eq!(
            decode_line(Protocol::ClaudeJsonl, r#"{"type":"stream_event"}"#),
            AgentEvent::Unknown {
                wire_type: "stream_event".into()
            }
        );
        assert!(matches!(
            decode_line(Protocol::ClaudeJsonl, "not json"),
            AgentEvent::Malformed { .. }
        ));
    }

    #[test]
    fn summary_is_flattened_and_truncated() {
        let long = "x".repeat(500);
        let event = AgentEvent::AgentMessage { text: long };
        let summary = event.summary();
        assert!(summary.chars().count() <= 200);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn pretty_claude_bash_presents_only_the_command() {
        let event = AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test --all","description":"run tests"}"#.into(),
        };
        assert_eq!(
            Protocol::ClaudeJsonl.presented_summary(&event, PresentationMode::Pretty),
            "cargo test --all"
        );
        assert_eq!(
            Protocol::ClaudeJsonl.presented_detail(&event, PresentationMode::Pretty),
            vec![DetailBlock::Code {
                language: Some("bash".into()),
                text: "cargo test --all".into(),
            }]
        );
        assert!(Protocol::ClaudeJsonl
            .presented_detail(&event, PresentationMode::Raw)
            .iter()
            .any(|block| matches!(block, DetailBlock::Text(text) if text.contains("description"))));
    }

    #[test]
    fn pretty_diffs_are_provider_independent_and_changed_lines_only() {
        let event = AgentEvent::DiffUpdated {
            diff: "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new".into(),
        };
        assert_eq!(
            Protocol::CodexAppServer.presented_detail(&event, PresentationMode::Pretty),
            vec![DetailBlock::Code {
                language: Some("diff".into()),
                text: "-old\n+new".into(),
            }]
        );
    }

    // The following lines are copied verbatim from a live `codex app-server`
    // session (codex-cli 0.145) captured during development.

    #[test]
    fn appserver_notifications_decode_from_real_output() {
        let d = |line| decode_line(Protocol::CodexAppServer, line);
        assert_eq!(
            d(
                r#"{"method":"thread/started","params":{"thread":{"id":"019f8f61-b7df-7291-81fc-04ff0bfb786f"}}}"#
            ),
            AgentEvent::ThreadStarted {
                thread_id: "019f8f61-b7df-7291-81fc-04ff0bfb786f".into(),
                model: None,
                effort: None,
            }
        );
        assert_eq!(
            d(r#"{"method":"turn/started","params":{"threadId":"t"}}"#),
            AgentEvent::TurnStarted
        );
        assert_eq!(
            d(
                r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"msg_1","text":"hello","phase":"final_answer"}}}"#
            ),
            AgentEvent::AgentMessage {
                text: "hello".into()
            }
        );
    }

    #[test]
    fn appserver_command_execution_decodes_both_ends() {
        let started = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"i0","command":"/usr/bin/bash -lc 'echo hi'","status":"in_progress","exitCode":null}}}"#,
        );
        assert_eq!(
            started,
            AgentEvent::CommandStarted {
                command: "/usr/bin/bash -lc 'echo hi'".into()
            }
        );
        let completed = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"i0","command":"/usr/bin/bash -lc 'echo hi'","aggregatedOutput":"hi\n","exitCode":0,"status":"completed"}}}"#,
        );
        assert_eq!(
            completed,
            AgentEvent::CommandCompleted {
                command: "/usr/bin/bash -lc 'echo hi'".into(),
                status: "completed".into(),
                exit_code: Some(0),
                output: "hi\n".into(),
            }
        );
    }

    #[test]
    fn appserver_token_usage_updates_usage_without_ending_the_turn() {
        // A real session reports this after every step within a turn (each
        // tool call, each model round), not just the last, so it must not be
        // read as the turn ending — only `turn/completed` is that signal.
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"totalTokens":12603,"inputTokens":12598,"cachedInputTokens":9600,"cacheWriteInputTokens":0,"outputTokens":5,"reasoningOutputTokens":0}}}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::UsageUpdated {
                usage: TokenUsage {
                    input_tokens: 12598,
                    cached_input_tokens: 9600,
                    output_tokens: 5,
                    reasoning_output_tokens: 0,
                }
            }
        );
    }

    /// What is actually running is the agent's to state, not the launch's to
    /// assume: the `thread/start` response is where the app-server names the
    /// model and reasoning effort it resolved, including when the operator
    /// pinned neither.
    #[test]
    fn appserver_thread_start_response_reports_the_model_and_effort_in_use() {
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"id":2,"result":{"thread":{"id":"t-9"},"model":"gpt-5.6-sol","modelProvider":"openai","reasoningEffort":"high","cwd":"/tmp/styra/workspace"}}"#,
            ),
            AgentEvent::ThreadStarted {
                thread_id: "t-9".into(),
                model: Some("gpt-5.6-sol".into()),
                effort: Some("high".into()),
            }
        );
        // A thread reported without them (an older app-server, or the
        // `thread/started` notification) still decodes; the fields stay absent
        // rather than being guessed at.
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"id":2,"result":{"thread":{"id":"t"}}}"#
            ),
            AgentEvent::ThreadStarted {
                thread_id: "t".into(),
                model: None,
                effort: None,
            }
        );
    }

    /// The reported model and effort must survive a journal round trip, since a
    /// stored session is read back through the same decoded events.
    #[test]
    fn a_reported_model_and_effort_survive_the_journal_round_trip() {
        let event = AgentEvent::ThreadStarted {
            thread_id: "t-9".into(),
            model: Some("gpt-5.6-sol".into()),
            effort: Some("xhigh".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<AgentEvent>(&json).unwrap(), event);
        assert!(event.summary().contains("gpt-5.6-sol"));
        assert!(event.summary().contains("xhigh"));

        // An older journal entry with neither field still reads back.
        let bare: AgentEvent =
            serde_json::from_str(r#"{"type":"thread_started","thread_id":"t"}"#).unwrap();
        assert_eq!(
            bare,
            AgentEvent::ThreadStarted {
                thread_id: "t".into(),
                model: None,
                effort: None,
            }
        );
    }

    #[test]
    fn appserver_turn_completed_is_the_end_of_turn_signal() {
        let event = decode_line(
            Protocol::CodexAppServer,
            r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"t1","status":"completed"}}}"#,
        );
        assert_eq!(
            event,
            AgentEvent::TurnCompleted {
                usage: TokenUsage::default()
            }
        );
    }

    #[test]
    fn appserver_control_and_echoed_user_message_carry_without_rendering() {
        // A response (no "method") is control traffic, unless it reports a
        // started thread — see the model/effort test below.
        for line in [
            r#"{"id":1,"result":{}}"#,
            r#"{"id":9,"result":{"turn":{"id":"t1"}}}"#,
        ] {
            assert_eq!(
                decode_line(Protocol::CodexAppServer, line),
                AgentEvent::Unknown {
                    wire_type: "response".into()
                }
            );
        }
        // The server echoes the operator's own message; the host shows its own, so
        // this decodes to Unknown rather than duplicating it.
        assert_eq!(
            decode_line(
                Protocol::CodexAppServer,
                r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"u","content":[{"type":"text","text":"hi"}]}}}"#
            ),
            AgentEvent::Unknown {
                wire_type: "item:userMessage".into()
            }
        );
    }
}

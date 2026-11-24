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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Total input, including cache reads and cache creation. Cached input is
    /// a subset of this count, matching Codex's inclusive input counter.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

impl TokenUsage {
    /// What was spent between `earlier` and here. Saturating, because a
    /// provider that resets or re-reports a counter must not produce a
    /// nonsense figure; `None` means nothing was spent before this point.
    pub fn since(&self, earlier: Option<&TokenUsage>) -> TokenUsage {
        let Some(earlier) = earlier else {
            return self.clone();
        };
        TokenUsage {
            input_tokens: self.input_tokens.saturating_sub(earlier.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(earlier.cached_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(earlier.output_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_sub(earlier.reasoning_output_tokens),
        }
    }

    /// The two spends added together.
    pub fn plus(&self, other: &TokenUsage) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens + other.input_tokens,
            cached_input_tokens: self.cached_input_tokens + other.cached_input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens + other.reasoning_output_tokens,
        }
    }
}

/// How a turn ended, as far as the provider stated it.
///
/// A turn that failed is still a turn that ended, and both providers say so on
/// the line that ends it — Claude in the `result`'s `subtype` and `is_error`,
/// the app-server in `turn.status` and `turn.error`. Reading the failure as an
/// error and nothing else, which is what Claude's decoder used to do, left a
/// failed turn with no ending at all: no end-of-turn line in the log, and a
/// client waiting for a `TurnCompleted` that was never coming.
///
/// What it does not cover is why a turn ended *early* by the operator's own
/// doing. An interrupt looks like an ordinary ending on the wire, so only the
/// client that asked for it knows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The turn ran its course.
    #[default]
    Completed,
    /// The turn gave up, with what the provider said about it.
    Failed { message: String },
}

/// What a turn cost, and what the thread has cost through the end of it.
///
/// No provider reports both, and they do not report the same one: the
/// app-server sends a running thread total in its own notification and
/// nothing at all with the turn's end, while Claude and `codex exec` report
/// the turn's own spend and never a total. Each decoder fills only the half
/// its wire line actually states — an absent figure stays `None` rather than
/// becoming a zero that reads like a real measurement — and [`UsageTracker`]
/// derives the other half from the run of events around it.
///
/// Both halves are optional, so this accepts almost anything — including the
/// bare [`TokenUsage`] that used to sit in `usage`'s place, which would come
/// through as neither figure and drop what a peer on an older build actually
/// said. `deny_unknown_fields` is what makes that a decode error instead of a
/// silent loss: a client showing nothing where a number was reported is worse
/// than one saying it could not read the line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnUsage {
    /// What this turn alone spent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TokenUsage>,
    /// What the thread has spent through the end of this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<TokenUsage>,
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

/// Which side of a branch an [`AgentEvent::Branched`] marker sits on. Each
/// marker names the other session, so the direction says which way the link
/// points rather than which session it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchDirection {
    /// This session was branched from the named one.
    From,
    /// The named session was branched from this one.
    To,
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
    /// The turn ended, for better or worse — see [`TurnOutcome`]. `usage`
    /// states what it cost and what the thread has cost so far, as far as
    /// either is known — see [`TurnUsage`], and [`UsageTracker`], which fills
    /// in whichever half the provider left out.
    ///
    /// `outcome` defaults because its absence means something definite — an
    /// ending that says nothing about how it went ran its course. `usage` does
    /// not: an ending always serializes one, empty (`{}`) when neither figure
    /// is known, so a missing `usage` is a line this build cannot read rather
    /// than one with nothing to report.
    TurnCompleted {
        #[serde(default)]
        outcome: TurnOutcome,
        usage: TurnUsage,
    },
    /// A running thread total that arrives independently of a turn's end (the
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
    /// The host restarted this conversation under another provider.
    ProviderChanged {
        from: String,
        to: String,
        model: String,
        effort: String,
    },
    /// A branch boundary between two sessions. No provider reports this — the
    /// host writes one marker into each side when it branches a session: the
    /// source records where its history was continued, the branch records
    /// where its history came from. It names the *other* session, so a client
    /// can follow the marker and open it.
    Branched {
        direction: BranchDirection,
        /// The other session's host id.
        session: String,
        /// The other session's name, when it has one, so the line reads as
        /// something an operator recognizes rather than as a bare id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
                | AgentEvent::ProviderChanged { .. }
                | AgentEvent::Branched { .. }
        )
    }

    /// The session an [`AgentEvent::Branched`] marker links to, so a client
    /// can offer to follow it without matching on the variant itself.
    pub fn branch_target(&self) -> Option<&str> {
        match self {
            AgentEvent::Branched { session, .. } => Some(session),
            _ => None,
        }
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
            // A turn's end is a turn event, tagged to pair with its start; the
            // `usage` tag stays with the running totals alone, so a reader can
            // tell a turn boundary from a mid-turn tick at a glance.
            AgentEvent::TurnStarted | AgentEvent::TurnCompleted { .. } => "turn",
            AgentEvent::UsageUpdated { .. } => "usage",
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
            AgentEvent::ProviderChanged { .. } => "provider",
            AgentEvent::Branched { .. } => "branch",
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
    ///
    /// A turn that *failed* is the exception among the turn markers: it is the
    /// only report of that failure Claude makes, and an operator who has the
    /// bookkeeping hidden still needs to be told the turn gave up.
    pub fn is_minor(&self) -> bool {
        if matches!(
            self,
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
                ..
            }
        ) {
            return false;
        }
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
            AgentEvent::TurnCompleted { outcome, usage } => {
                let spend = |usage: &TokenUsage| {
                    format!(
                        "in {} · out {} · cached {}",
                        usage.input_tokens, usage.output_tokens, usage.cached_input_tokens
                    )
                };
                // A failure leads with why, since that is what the line is
                // there to report; the figures follow, because a turn that
                // gave up still spent what it spent.
                let ending = match outcome {
                    TurnOutcome::Completed => "turn complete".to_owned(),
                    TurnOutcome::Failed { message } => {
                        format!("turn failed: {}", first_line(message))
                    }
                };
                match (&usage.turn, &usage.total) {
                    (Some(turn), Some(total)) => format!(
                        "{ending} · this turn {} (thread total {})",
                        spend(turn),
                        spend(total)
                    ),
                    (Some(turn), None) => format!("{ending} · this turn {}", spend(turn)),
                    (None, Some(total)) => format!("{ending} · thread total {}", spend(total)),
                    // Nothing was reported and nothing could be reconstructed
                    // — say only what is known, rather than three zeros that
                    // read as a turn that cost nothing.
                    (None, None) => ending,
                }
            }
            AgentEvent::UsageUpdated { usage } => format!(
                "thread total · in {} · out {} · cached {}",
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
            AgentEvent::ProviderChanged {
                from, to, model, ..
            } => {
                format!("provider: {from} → {to} · model → {model}")
            }
            AgentEvent::ModelChanged { model, effort } => match (model, effort) {
                (Some(model), Some(effort)) => format!("model → {model} · effort → {effort}"),
                (Some(model), None) => format!("model → {model} (same effort)"),
                (None, Some(effort)) => format!("effort → {effort} (same model)"),
                // Not emitted; a selection that changed nothing is not an event.
                (None, None) => "selection unchanged".into(),
            },
            // The id, not the name: a branch inherits its source's name, so
            // the name alone would not say which of the two the line points
            // at. The name is in the detail, where it is unambiguous.
            AgentEvent::Branched {
                direction, session, ..
            } => match direction {
                BranchDirection::From => format!("branched from {session}"),
                BranchDirection::To => format!("branched to {session}"),
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
            AgentEvent::TurnCompleted { outcome, usage } => {
                let mut lines = Vec::new();
                // In full, not flattened to its first line as the summary has
                // it: a provider's failure often runs to a paragraph, and this
                // is the one place it can be read.
                if let TurnOutcome::Failed { message } = outcome {
                    lines.push(message.clone());
                }
                if let Some(turn) = &usage.turn {
                    lines.push(format!("this turn:    {}", spend_detail(turn)));
                }
                if let Some(total) = &usage.total {
                    lines.push(format!("thread total: {}", spend_detail(total)));
                }
                if lines.is_empty() {
                    Vec::new()
                } else {
                    vec![DetailBlock::Text(lines.join("\n"))]
                }
            }
            AgentEvent::UsageUpdated { usage } => {
                vec![DetailBlock::Text(format!(
                    "thread total: {}",
                    spend_detail(usage)
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
            AgentEvent::ProviderChanged {
                from,
                to,
                model,
                effort,
            } => vec![DetailBlock::Text(format!(
                "provider: {from} → {to}\nmodel: {model}\nreasoning effort: {effort}"
            ))],
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
            AgentEvent::Branched {
                direction, name, ..
            } => {
                // The id is left out: following the marker is how an operator
                // gets to that session, and the id says nothing they can read.
                // A host adds what it knows of the session now — see
                // `styra_ui::event_list::with_live_branch`.
                let mut lines = vec![match direction {
                    BranchDirection::From => "branched from this session:".to_string(),
                    BranchDirection::To => "branched into this session:".to_string(),
                }];
                if let Some(name) = name {
                    lines.push(format!("name: {name}"));
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

/// Return the working directory an agent reports on one inbound wire line.
///
/// This is deliberately separate from [`AgentEvent`]: a directory is live
/// session metadata, not an event for the conversation timeline. Claude Code
/// puts it at the top level of its stream records; Codex's app-server returns
/// it while opening a thread (and has used both result and notification
/// shapes). The one-shot Codex transcript format keeps it in `session_meta`.
///
/// Claude Code states that top-level directory only on the `system`/`init`
/// line it writes once per turn, so an agent that moves mid-turn is read from
/// the ingest context of the tool calls that follow instead — see
/// [`tool_ingest_cwd`].
///
/// The caller must still verify that the path belongs to the workspace before
/// trusting or presenting it.
pub fn reported_cwd(protocol: Protocol, line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let cwd = match protocol {
        Protocol::ClaudeJsonl => string(&value, "cwd").or_else(|| tool_ingest_cwd(&value)),
        Protocol::CodexJsonl => value
            .get("payload")
            .filter(|_| string(&value, "type") == Some("session_meta"))
            .and_then(|payload| string(payload, "cwd")),
        Protocol::CodexAppServer => value
            .pointer("/result/cwd")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/params/cwd").and_then(Value::as_str))
            .or_else(|| value.pointer("/params/thread/cwd").and_then(Value::as_str)),
    }?;
    (!cwd.is_empty()).then(|| cwd.to_owned())
}

/// The directory Claude Code's tool calls ran in, from the `wire_ingest_context`
/// an assistant line carries — one entry per `tool_use` id in that message.
///
/// This is the only place a move made during a turn appears before the turn
/// after it, and so the only thing standing between a Session and a whole turn
/// spent naming a directory the agent has left.
///
/// A tool need not run where its thread stands — a subagent works elsewhere,
/// and a single command may `cd` on its way — so this speaks for the thread
/// only when every entry agrees. Disagreement yields `None` rather than a
/// guess: leaving the Session where it was is wrong for one turn, while
/// following a subagent into its directory is wrong until something else moves
/// it back. The entries cannot be ordered to break the tie either, since they
/// are keyed by tool id and reach us through a sorted map.
fn tool_ingest_cwd(value: &Value) -> Option<&str> {
    let mut agreed: Option<&str> = None;
    for context in value.get("wire_ingest_context")?.as_object()?.values() {
        let cwd = string(context, "cwd")?;
        match agreed {
            Some(previous) if previous != cwd => return None,
            _ => agreed = Some(cwd),
        }
    }
    agreed
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
        // `turn/completed` is the actual end-of-turn signal. It carries no
        // usage figures of its own — both halves are left unknown here and
        // reconstructed from the running totals by `UsageTracker` — but it
        // does say whether the turn got there, and with what error if not.
        "turn/completed" => AgentEvent::TurnCompleted {
            outcome: appserver_turn_outcome(params.get("turn").unwrap_or(&Value::Null)),
            usage: TurnUsage::default(),
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

/// How a `turn/completed` says its turn went. Any status but `failed` is the
/// turn running its course — including `interrupted`, which the client that
/// asked for the interrupt reads as its own doing, not as a failure.
fn appserver_turn_outcome(turn: &Value) -> TurnOutcome {
    if string(turn, "status") != Some("failed") {
        return TurnOutcome::Completed;
    }
    TurnOutcome::Failed {
        message: clean_terminal_text(&error_message(turn)),
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
        // `codex exec` reports what the turn itself spent, never a thread
        // total — for a one-shot run the two coincide, but the distinction is
        // the honest one to record.
        "turn.completed" => AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage: TurnUsage {
                turn: value
                    .get("usage")
                    .and_then(|usage| serde_json::from_value(usage.clone()).ok()),
                total: None,
            },
        },
        // `turn.failed` ends the turn as surely as `turn.completed` does, so
        // it is an ending that states why, not a bare error: read as the
        // latter it left the turn with no end at all. A `turn.failed` reports
        // no usage. A bare `error` line is not an ending — the run may carry
        // on — and stays one.
        "turn.failed" => AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Failed {
                message: clean_terminal_text(&error_message(value)),
            },
            usage: TurnUsage::default(),
        },
        "error" => AgentEvent::Error {
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
    // A `result` is the end of Claude's turn whether the turn went well or
    // not, and it is the only line that says so: reading a failed one as an
    // error alone left the turn with no ending, and a client waiting on one.
    let outcome = if is_error || subtype.starts_with("error") {
        TurnOutcome::Failed {
            message: clean_terminal_text(&error_message(value)),
        }
    } else {
        TurnOutcome::Completed
    };
    // Claude's `result` states what the turn spent and nothing about the
    // thread; the running total is added up by `UsageTracker`.
    AgentEvent::TurnCompleted {
        outcome,
        usage: TurnUsage {
            turn: value.get("usage").map(claude_usage),
            total: None,
        },
    }
}

/// Map Claude's separate input buckets onto our inclusive input count.
/// Cache creation is new input; only cache reads belong in cached input.
fn claude_usage(usage: &Value) -> TokenUsage {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: count("input_tokens")
            + count("cache_creation_input_tokens")
            + count("cache_read_input_tokens"),
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

/// Reconstructs the half of [`TurnUsage`] the provider did not report.
///
/// Decoding is per line and stateless, but neither figure can be had from one
/// line alone: a turn's own spend only exists as the difference between two of
/// the app-server's running totals, and a thread total only exists as the sum
/// of Claude's per-turn ones. So the events run past this on the way to being
/// shown — [`render_events`](crate::render::render_events) for a recorded log,
/// the host's own ingest for a live session — and each `TurnCompleted` leaves
/// it stating both figures.
///
/// A provider that reports neither (an interrupt the host synthesizes an
/// ending for) still leaves both `None`: an unknown figure stays unknown.
#[derive(Clone, Debug, Default)]
pub struct UsageTracker {
    /// The last running total seen, for providers that send them. Only ever
    /// written from `UsageUpdated` — a per-turn figure must not land here, or
    /// the next turn would be measured against the wrong baseline.
    running: Option<TokenUsage>,
    /// The running total as it stood when the current turn began.
    turn_start: Option<TokenUsage>,
    /// Per-turn spends added up, which is the only thread total available for
    /// a provider that never sends one. `None` until a turn has reported one.
    accumulated: Option<TokenUsage>,
}

impl UsageTracker {
    /// Note what this event says about spend, and fill in what a turn's end
    /// leaves out.
    pub fn observe(&mut self, event: &mut AgentEvent) {
        match event {
            AgentEvent::ThreadStarted { .. } => *self = Self::default(),
            AgentEvent::TurnStarted => self.turn_start = self.running.clone(),
            AgentEvent::UsageUpdated { usage } => self.running = Some(usage.clone()),
            AgentEvent::TurnCompleted { usage, .. } => {
                if usage.turn.is_none() {
                    usage.turn = self
                        .running
                        .as_ref()
                        .map(|end| end.since(self.turn_start.as_ref()));
                }
                if let Some(turn) = &usage.turn {
                    self.accumulated = Some(match &self.accumulated {
                        Some(total) => total.plus(turn),
                        None => turn.clone(),
                    });
                }
                if usage.total.is_none() {
                    usage.total = self.running.clone().or_else(|| self.accumulated.clone());
                }
                self.turn_start = self.running.clone();
            }
            _ => {}
        }
    }
}

/// One spend, spelled out in full for a detail pane.
fn spend_detail(usage: &TokenUsage) -> String {
    format!(
        "input {} · cached input {} · output {} · reasoning {}",
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens
    )
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
mod tests;

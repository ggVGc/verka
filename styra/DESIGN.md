# Styra design

## Purpose

Styra manages durable Workspaces containing provider-native agent Sessions. A
Session may have one live Interaction: Styra's isolated process/protocol wrapper
around the provider conversation. The terminal application navigates
Workspaces and Sessions, renders a Session's events, and steers its Interaction
when one is live.

Where Orka runs an agent *non-interactively* against a Linka node — one prompt,
run to completion, transcript captured as durable evidence — Styra runs an agent
*interactively*: a live session the operator steers turn by turn while the same
raw event journal is captured. Styra is the interactive counterpart to an Orka
attempt, not a replacement for it.

Styra is a peer of Orka, not a layer above or below it. It depends on Driva to
obtain isolation and on nothing else in the suite. It does not depend on Orka,
Linka, or Nota, and none of them depend on Styra.

```text
Orka  ----> Driva ----> Bubblewrap
Styra ----> Driva ----> Bubblewrap
```

## Scope and non-goals

Styra owns, in its first form:

- durable Workspaces grouping related Sessions against a host directory;
- launching isolated Interactions through Driva;
- the wire protocol spoken to that agent and its decoding into a small, stable
  event vocabulary;
- capturing the raw event journal verbatim as the session's fundamental record;
- a terminal application that lists events, expands and collapses them, and
  sends operator messages to the agent;
- separate Workspace, Session, and Interaction lifecycles.

Styra does **not**, in its first form:

- discover, freeze, or record work in a Linka store (that is Orka's role);
- perform reviews or produce candidates (Orka and Nota);
- implement isolation (Driva);
- interpret which program produced a stream inside Driva (Driva transports
  bytes; Styra owns interpretation, exactly as Orka does).

`F` starts a new Interaction for a stopped Session using the provider's native
resume mechanism. Styra never reconstructs old context into a new prompt.

## Server-client architecture

Styra is split at a versioned, local JSON boundary:

```text
styra TUI ──Unix socket──> styra-server ──> Genta protocol ──> agent
other tools ─────────────>       │
                                 ├──> Driva isolation
                                 └──> XDG journal store
```

`styra-server` owns Workspace metadata, durable Sessions, and live
Interactions: process launch, agent stdin/stdout, Genta protocol state,
journals, update ordering, and replay. Each socket connection carries one
newline-terminated JSON request and response. Live updates have monotonically
increasing sequence numbers; clients poll with an `after` cursor, which
supports reconnects and multiple independent observers.

The TUI is an ordinary socket client. It owns presentation and input state, but
never constructs an `Interaction`, opens a journal, decodes provider traffic, or
calls Driva. The headless example uses the same client. Public wire types live
in `api.rs`, the reusable Rust client in `client.rs`, and server dispatch in
`server.rs`.

Each live interaction also has a persistent interactive shell. A hidden broker is
the top-level command inside Bubblewrap: it starts a detached tmux server and
`/bin/sh`, then launches the profile's agent with the inherited protocol
streams unchanged. The tmux socket is created through a private per-session
control bind mount, allowing `styra shell` (or `styra shell --session ID`) to
select and attach from the host
without exposing a host shell inside the sandbox or mixing terminal bytes into
the agent journal. The broker kills tmux when the agent interaction ends.

Durable Workspaces and Sessions default to `$XDG_STATE_HOME/styra` (falling back to
`$HOME/.local/state/styra`). The socket is independent, ephemeral runtime state
at `$XDG_RUNTIME_DIR/styra/styra.sock`. Default Styra directories use mode
`0700`, and the socket uses mode `0600`. This is deliberately a local API: it
has no TCP listener or remote-access configuration.

## Ownership and boundaries

- **Driva** owns isolation: mount policy, networking policy, backend selection,
  and connecting the isolated process to the standard streams Styra provides. It
  never interprets the bytes on those streams.
- **Styra server** owns the agent profile (command, wire protocol, how a user message
  is encoded as an input line), the decoding of provider wire events into
  Styra's event vocabulary, the raw journal, and live interaction lifecycle.
- **Styra clients** own presentation and operator interaction. They consume
  only the JSON API and never receive process or journal handles.

The boundary mirrors Orka's: the provider wire format stops inside Styra. The
rest of the application — the list, the renderer, session state — consumes only
Styra's own event vocabulary, and Driva stays an uninterpreted transport.

Styra deliberately re-derives, rather than imports, the agent-event vocabulary
Orka already has. Orka's `events` and `agent` modules are private and shaped
for a one-shot `exec` run; sharing them would couple two peer applications and
drag a one-shot execution model into an interactive one. The two vocabularies
are kept *aligned* (same event names, same versioned-decoder discipline) so that
a future extraction into a shared, dependency-free crate remains open, but that
extraction is not a prerequisite and is not part of this design.

## Running the agent through Driva

Driva's execution interface already fits an interactive session without change.
`driva::execute` takes an `ExecutionRequest` and an `ExecutionIo { stdin,
stdout, stderr }` whose fields are ordinary `File` handles wired directly to the
child's `Stdio`. Orka passes `/dev/null` for stdin and a file for stdout because
its run is one-shot; the server instead passes the ends of OS pipes:

1. The server creates two pipes: one for the child's stdin, one for its stdout.
   A third file receives stderr as diagnostics, as in Orka.
2. The child's stdin-read end and stdout-write end become the `ExecutionIo`
   handed to `driva::execute`. The server keeps the stdin-write end and the
   stdout-read end.
3. `driva::execute` is called on a dedicated worker thread. It blocks for the
   life of the interaction (the agent process runs until it exits or is stopped),
   which is why it runs off the thread accepting socket requests.
4. A reader thread pulls newline-delimited JSON from the stdout-read end,
   decodes each line, and forwards events to the update collector. Operator
   messages, arriving over the socket, are written by the server as protocol
   input lines to the stdin-write end.
5. Closing the stdin-write end signals end-of-input to the agent; dropping the
   child (interaction stop) tears the interaction down. The worker thread's return value
   carries the exit report.

No change to Driva is required, and this is a deliberate check on Driva's
interface: an interactive, bidirectional session composes from the same
validated-request-plus-streams primitive as a batch run.

Isolation policy follows Orka's proven shape and is owned by Styra's agent
profile: a writable workspace mount (the project or a throwaway worktree), a
writable agent-auth mount, networking enabled for the agent, and everything else
denied. Styra does not invent new isolation concepts; it selects Driva policy.

## The agent profile

A profile is the only agent-specific knowledge in Styra. It defines:

- `command` — the argument vector Driva executes;
- `protocol` — a versioned identity for the wire format, exactly like Orka's
  `AgentProtocol`, selecting both the encoder for outgoing messages and the
  decoder for incoming events;
- `mounts`, `environment`, `network` — the Driva policy the agent needs;
- `message_format` — how an operator message becomes one input line.

### A selection resolves to an internal profile

Styra's user-facing launch concept is genta's structured `Selection`: provider,
model, and effort. All three parts are always present. A selection may not leave
the model or effort to whatever the agent happens to be configured for, because
that configuration is invisible both to Styra and to anyone reading the journal
afterwards. Each provider therefore declares defaults
(`Provider::default_model`, `Provider::default_effort`).

The defaults are declared rather than derived from the front of the catalog, so
reordering the catalog cannot silently move every unpinned launch to another
model. Claude Code's differs from its catalog's lead deliberately: the catalog
starts at `claude-fable-5`, priced above the Opus tier, so an operator who named
no model gets `claude-opus-5`. Interactive Codex defaults to `medium`; Claude
Code defaults to `high`. Naming no level therefore applies Styra's declared
provider default explicitly.

Both parts reach each agent its own way — codex as `-c model=…` and
`-c model_reasoning_effort=…` on the process it launches, Claude Code as `--model`
and `--effort` — and the effort ladders differ at the ends (codex has `minimal`,
Claude Code has `max`), so each provider publishes the levels it accepts.

The server resolves the structured selection to an internal Genta profile. The
selection itself is persisted in `SessionMeta`, so the model and effort that ran
are available directly during replay. Model ids are free-form strings, not an enum
— the authoritative catalog belongs to the agent, and an id it does not know
fails there rather than being second-guessed here. Genta only suggests a
per-provider list for a picker to offer.

Nothing below this boundary can be partial either: Genta's profile builders take
a model and an effort outright, so no host can construct a profile that leaves
them to the agent. Orka, which has no operator to ask, pins them from its own
configuration (`agent.model` / `agent.effort`, defaulting to the same declared
values) — an attempt's recorded argv has to say what produced the work.

The protocol is versioned: a new wire format, or a new revision of an existing
one, is a new `protocol` variant plus a decoder arm, and the match is
exhaustive, so a missing decoder is a compile error rather than a silent
mis-decode. This is the same discipline as Orka's decoder registry.

### Interactive agent protocols

The session and pipe machinery is genuinely bidirectional and multi-turn: it
holds the agent's stdin open and writes each operator message as it is sent.

The default `codex` profile is multi-turn over the experimental **`app-server`
JSON-RPC protocol** on stdio. This is a stateful wire contract, not a plain
event stream, so it has two cooperating parts:

- the `codex-app-server` `Protocol` variant decodes *notification* lines
  (`thread/started`, `turn/started`, `item/started`, `item/completed`,
  `thread/tokenUsage/updated`, errors) into the event vocabulary — shared by
  live interactions and journal replay; requests and responses decode as `Unknown`
  control traffic, with one exception: the `thread/start` *response* is the only
  place the app-server states the model and reasoning effort the thread resolved
  to, so it decodes to `ThreadStarted` carrying them (recognised by the thread it
  reports rather than by the request id it answers, which belongs to the client
  and not to the wire format). The `thread/started` notification then repeats a
  thread already known, naming neither, so the client suppresses it rather than
  logging a second, less informative session entry;
- an `AppServer` client owns the session state machine: `initialize` →
  `initialized` → `thread/start` for a new Session or `thread/resume` for a
  stopped one → one `turn/start` per operator message. Messages sent before the
  thread is ready are queued and flushed on readiness. The reader thread routes
  every line through this client; the client forwards decoded events and answers
  control traffic.

The turn's token usage arrives as `thread/tokenUsage/updated` just before
`turn/completed` (which itself carries none), so that notification is what maps
to `TurnCompleted` — flipping the status line to `waiting` between turns. The
server exits on stdin end-of-input, so stopping a session tears it down cleanly.
Threads are started with `approvalPolicy: never` and a `danger-full-access`
inner sandbox: approvals never stall a turn, and real isolation stays Driva's.
Any server-to-client request that does appear is surfaced in the log view
rather than silently dropped.

## Event vocabulary

Styra decodes provider wire events into a small, stable set that the UI
consumes. It is intentionally the same shape as Orka's `AgentEvent`:

- `ThreadStarted { thread_id, model, effort }` — the model and reasoning
  effort are the agent's own report of what the session resolved to, absent
  where it does not name them (Claude Code reports a model but no effort)
- `TurnStarted` / `TurnCompleted { usage }`
- `CommandStarted { command }` / `CommandCompleted { command, status,
  exit_code, output }`
- `FileChanged { paths }`
- `ToolStarted { name, detail }` / `ToolCompleted { name, status }`
- `PlanUpdated { text }`
- `AgentMessage { text }`
- `Error { message }`
- `Unknown { wire_type }` — a recognised envelope Styra has no view for; carried
  but not rendered.
- `Malformed { error }` — an undecodable line; kept visible as an error rather
  than dropped.
- `UserMessage { text }` — a Styra-originated event recording what the operator
  sent, so the operator's own turns appear inline in the same list.

Each event renders to a **one-line summary** (for the collapsed list) and a
**detail body** (for the expanded view). The detail body reuses Orka's
presentation-block idea: prose and fenced code become structured blocks with no
embedded terminal escapes, so the renderer adds styling rather than parsing
provider text. Terminal control sequences in provider text are stripped on
decode, as Orka does.

## The raw journal is the session

The fundamental record of a session is an append-only journal of source-tagged
records, one per line: each agent record carries the verbatim line received on
the agent's stdout, and each operator record carries a message the operator
sent. Append order is receive order, so a single ordered file reconstructs the
whole session — agent turns and operator turns interleaved — without a separate
clock. The agent's line is preserved byte-for-byte inside its record, so the
protocol decoder still reads it as the fundamental fact. Nothing rendered or
normalized is written at rest; the list, the summaries, and the detail bodies
are all interpretations produced on demand from the journal — the same stance
Orka takes toward its raw logs.

This is what makes the wishlist's session properties fall out cheaply:

- **Stop without losing context.** Stopping ends the child process; the journal
  remains. The context *is* the journal.
- **View.** Styra can open a journal and replay it into the same list view
  without a live agent.
- **Resume.** A stopped Session starts a new Interaction using the provider's
  native conversation id, appending new traffic to the same Styra journal.
  The raw journal remains independently viewable even if native state is gone.

### Native resume

Both codex and Claude Code implement their own `resume` by replaying a
persisted transcript to reconstruct context, not by restoring literal model
state — so underneath, they aren't fundamentally different from Styra
rendering its journal into a seed message. But operationally, neither expects
its *client* to invent that seed text: each has its own native resume path,
keyed by the same thread/session id genta already captures via
`ThreadStarted`.

- **Codex app-server** exposes a `thread/resume` JSON-RPC method that
  reconstructs the thread's internal context by replaying **codex's own
  rollout file** (`~/.codex/sessions/...`) — storage separate from Styra's
  journal. Genta sends this RPC when reopening a stopped Session.
- **Claude Code** has no equivalent mid-stream RPC; resume is a
  process-launch-time CLI flag (`--resume <session_id>` / `--continue`) that
  reloads Claude's own locally stored transcript before the new process's
  first turn.

Styra hands the freshly spawned agent process *its own*
thread/session id and lets its native resume machinery reconstruct context
from *its own* storage, rather than Styra reconstructing a prompt from *its*
journal. `session.json` stores that provider identity once `ThreadStarted` is
observed. Provider state directories are mounted writable so their native
records survive the sandbox. Sessions without an id, and sessions whose native
record has disappeared, remain viewable but return an error from resume.

Before launching a resumed Interaction, the server seeds its cursor-based
update history from the stored journal and records the resulting boundary.
The client that requested resume already has that journal on screen and starts
polling after the boundary. A later client attaches from cursor zero and
therefore receives the historical events/raw records and then the new native
Interaction updates without a snapshot/poll race or duplicate history.

A structurally different alternative, seen in the `pi.dev` harness: rather
than replaying raw or rendered history, generate a goal-first **recap**
(why the session exists, current state, decisions, relevant files, likely
next action) and seed with that instead of — or once a session gets long,
alongside — the full transcript. Pi can do this because it talks to model
provider APIs directly, so it owns the conversation's message array end to
end; Styra only ever owns a subprocess's text-shaped input. A recap here
would mean Styra itself calling out to a model to summarize before seeding
the new session — a real new capability (an API client, a key, a cost),
not a reshaping of what already exists. It remains deferred.

The durable layout makes ownership explicit:

```text
workspaces/<workspace-id>/
  workspace.json
  sessions/<session-id>/
    session.json
    journal.jsonl
    diagnostics.log
```

`workspace.json` records the Workspace identity, optional display name, host
directory, creation time, and the standing launch policy every Session started
here begins from (see "The sandbox is chosen where it is shown"). A host
directory does not determine identity: separate Workspaces may intentionally
refer to the same checkout.

Alongside `journal.jsonl`, one `session.json` is written at session creation:
the owning Workspace plus genta's `SessionMeta` (the structured selection and
wire protocol that launched the Session). Once the provider reports its
conversation id, that id is added to the sidecar for native resume. The journal
itself is agent-agnostic—it stores whatever raw line arrived—so without this
sidecar there is no record of which agent a stored Session came from. `--view`
reads `session.json` and decodes with the protocol it names rather than guessing
from current launch defaults.
Workspace ownership and `session.json` are required; incomplete or flat Session
directories are invalid store entries.

## Terminal interface

The application is a single full-screen view with three regions:

```text
┌───────────────────────────────────────── styra · codex · running ─┐
│  ▸ user     implement the retry backoff and add a test            │
│  ▸ plan     3 steps · 1 done                                      │
│  ▾ command  cargo test                                            │
│      status: completed (exit 0)                                   │
│        ┌─ message ──────────────────────────────────────┐          │
│        │ › _                                           │          │
│        └───────────────────────────────────────────────┘          │
│      test result: ok. 24 passed; 0 failed                         │
│  ▸ files    src/retry.rs, tests/retry.rs                          │
│  ▸ agent    Added exponential backoff capped at 30s; tests pass.  │
│  ▸ usage    in 4.1k · out 900 · cached 2.0k                       │
└───────────────────────────────────────────────────────────────────┘
```

- **Event list (top).** One line per event: a type tag and its one-line summary.
  The list scrolls and auto-follows the tail while the newest entry is selected;
  moving the selection upward pins the view so incoming events do not yank it
  away.
- **Message box (center overlay).** A floating single- or multi-line editor.
  Submitting sends the text to the agent (encoded by the profile) and appends a
  `UserMessage` entry to the list.
- **Status line (top border).** Application name, the agent, the model and
  reasoning effort in use, and session state: `not started` (no interaction launched
  yet, awaiting the operator's first message), `running`, `idle` (turn complete,
  agent idle for input), `stopped`, or `ended`/`failed` once the agent process
  exits. Token usage from the latest `TurnCompleted` is shown.

  The model and effort are named in every view, because the agent's name alone
  does not say them, and what runs can differ from what was asked for. Each agent states what it resolved as it starts a session
  (`ThreadStarted`), and that report is what the line shows; until it arrives —
  and for a value the agent never reports, such as Claude Code's effort — the
  line falls back to what the launch asked for and dims it, so what *is* running
  reads differently from what was *requested*. A stored session's selection is
  reconstructed from its metadata without resolving or launching a profile.

### The raw view

The event list is one interpretation of the journal; the journal itself is the
verbatim wire interaction. `r` toggles the top region between the event list and
a **raw view** that shows that interaction undecoded, one wire line per row:
outgoing operator submissions marked `»` and incoming agent lines marked `«`, in
occurrence order. It is the same fact the decoder reads and the journal stores,
shown directly — useful for understanding an `Unknown`/`Malformed` event, or
just watching the protocol.

Each row is truncated to one line rather than wrapped, so the list reads as a
dense timeline instead of a wall of JSON; a side **entry panel** pretty-prints
and syntax-highlights the selected line in full (falling back to the verbatim
text if it does not parse as JSON), so nothing is actually lost to the
truncation. `j`/`k` move the selection one line at a time (`g`/`G` jump to the
first/last line), `PgUp`/`PgDn` scroll the entry panel for a line whose
pretty-printed form overflows it, and the selection tracks the newest line
until the operator moves off the tail.

Entering the raw view focuses the wire line behind whatever entry was
selected in the event list (or the tail, if the list was following it), so
switching views keeps the same point in the session in view rather than
resetting to wherever the raw view was last left. Under `--view` the raw view
is reconstructed from the stored journal, replayed in the same order as the
event list so that correspondence still holds.

### The log view

`l` toggles a **log view** for diagnostics that are neither agent events nor
wire lines: Styra's own notes (launch command, bytes sent, exit code, why a
message was not sent) and the agent's stderr streamed live. Entries are tagged
`info`/`warn`/`error`. The agent's stderr is the usual place a failure explains
itself — a missing credential, a rejected flag, a backend error — so streaming
it here (rather than only persisting it to `diagnostics.log`) is what makes a
session that produces no events diagnosable from inside the interface. The log
view shares the raw view's bottom-anchored scrolling.

### The quota view

`Q` toggles a **quota view**: what the providers have said about how much of
the plan subscription is left. Both interactive agents volunteer these figures
unprompted and in different shapes — Claude sends a `rate_limit_event` naming
one window (`five_hour`) and its reset, adding a `utilization` figure only once
it has something to warn about; Codex reports a percentage for a short and a
long window inside its token-count notification, and says nothing about
severity. Genta's decoder keeps neither, so the server reads them off the
verbatim line before anything discards them (`styra_server::quota`).

The log is **server-wide and in-memory**, which is the whole design in one
sentence: quota belongs to the account rather than to a session, so a reading
taken on one Interaction is what every other Interaction is also spending, and
it is a live reading rather than a record worth keeping, so it dies with the
daemon instead of accumulating in the store beside the journals. `Q` therefore
also refreshes the view — there is nothing local to show without asking.

A reading is *announced* rather than merely recorded when a window is nearly
full (the provider's own warning, or past 90% for Codex, which states no
threshold of its own) or has been refused outright. An announcement crosses the
wire as its own `InteractionUpdate::Quota`, so the client decides how to show
it rather than parsing prose out of a log line: Styra puts it in this view, on
the log at `warn` (or `error`, when the window is full), and in a transient
notice, so an operator reading the event list learns their window is filling
without having gone looking. A given window is announced only when its reading
actually changes — status moving, or usage climbing another ten percent — so a
provider that repeats the same warning every turn costs one message, not one
per turn. Every reading is logged either way.

Note that what the providers volunteer is thinner than a quota display would
want: Claude reports no figure at all below 90%, so a permitted window shows
`?` rather than a misleading `0%`, and neither provider names the plan or its
credit balance on the wire.

### The transcript view

`t` toggles a **transcript view**: the current session's decoded events laid
out as plain text through genta's `render_events`, read from the live entries
in memory each frame rather than a stored journal. It exists for operators who
want to skim or copy the whole
conversation as prose instead of navigating the folded event list one entry
at a time. Unlike the raw and log views, it anchors to the *start*: a
transcript reads as a document front-to-back, not a tail-following stream, so
there is no "stays put while scrolled up" logic to speak of — new content
just extends past whatever is already below the viewport.

Claude streams extended thinking as many lines per turn — reasoning prose plus
a running `thinking_tokens` count. They all describe one ongoing thought, so a
consecutive run of them folds into a single list line whose token figure is
rewritten in place; the line only breaks when other work intervenes.

`c` toggles a conversation-only filter in the main event list, hiding tools,
thinking, plans, and lifecycle events while retaining the operator's messages
and the agent's replies. Like the minor-event filter, it does not switch views.
The filter is on by default.

### Two focuses, like vim modes

The wishlist asks to "go in and out of the main view, like vim insert/normal
mode." Styra has two focuses and one key that toggles between them:

- **List focus (normal).** Keys navigate and fold the list. This is the default.
- **Input focus (insert).** Keys type into the message box.

Toggle: `i` (or `Enter` on an empty selection) enters input focus; `Esc` returns
to list focus. `Tab` also toggles, for operators who prefer a single key. The
current focus is shown in the status line and by which region draws the cursor.

### List-focus keys

| Key             | Action                                                      |
| --------------- | ----------------------------------------------------------- |
| `j` / `↓`       | Select next entry                                           |
| `k` / `↑`       | Select previous entry                                       |
| `Space`/`Enter` | Toggle expand/collapse of the selected entry                |
| `o` / `c`       | Expand / collapse the selected entry explicitly             |
| `C`             | Show only expanded conversation lines                       |
| `zR` / `zM`     | Expand all / collapse all                                   |
| `g` / `G`       | Jump to first / last entry (`G` re-enables tail-follow)     |
| `r`             | Toggle the raw wire view, focused on the selected entry's wire line (in the raw view, `j`/`k`/`g`/`G` select, `PgUp`/`PgDn` scroll the entry panel) |
| `l`             | Toggle the diagnostic log view (same scrolling as the raw view) |
| `t`             | Toggle the rendered transcript view (`j`/`k`/`g`/`G` scroll from the start) |
| `Q`             | Toggle the plan-quota view, refreshing it from the server (`j`/`k` scroll) |
| `i`             | Enter input focus                                           |
| `L`             | Choose launch settings, or the model for the next idle agent turn (and Codex effort) |
| `s`             | Stop the Interaction (keeps the Session and journal)        |
| `F`             | Seed an explicit new Session from this Session's transcript |
| `a`             | Open live Interactions above the event list                  |
| `D`             | In the Interaction navigator, delete the selected stopped Interaction |
| `A`             | Browse Sessions in the current Workspace with a preview     |
| `S`             | Stop the Interaction and return to a blank Session screen   |
| `V`             | Choose a Workspace, then browse its Sessions                |
| `q`             | Quit (prompts if the session is still running)              |

### Input-focus keys

| Key            | Action                                                       |
| -------------- | ------------------------------------------------------------ |
| `Enter`        | Send the message (configurable: `Enter` sends vs. newline)   |
| `Alt+Enter`    | Insert a newline (when `Enter` sends)                        |
| `Ctrl+L`       | Choose launch settings, or the next idle turn's model (and Codex effort) |
| `Ctrl+F`       | Insert a file path, mounting it first if the sandbox lacks it |
| `Esc`          | Return to list focus without sending                         |

#### Naming a file to the agent

`Ctrl+F` opens a path prompt over the message box. `Tab` completes against the
host filesystem, `Enter` accepts, and what is accepted has to exist: a path that
is not there cannot be mounted either, so it is refused here rather than at the
launch that would have carried it.

An accepted path is inserted **in the agent's terms**, not the operator's. The
operator knows host paths; the agent only ever sees the destination its mount
carries, and the two differ whenever a mount renames what it binds — the
workspace itself being the usual case. So the path is looked up in the live (or
planned) Driva policy and rewritten through the innermost mount that carries it,
which is the one whose destination and access actually apply.

A path no mount carries stops for a second question — `r` readable, `w`
writable, `n` insert without mounting — because the alternative is a message
that names something the sandbox has never heard of and a turn spent finding
that out. A grant lands in *this interaction's* layer, never the Workspace's:
the path came up in one message, which is the smallest claim available, and `U`
in the driva view moves it up a layer if it turns out to belong to the work.
Like every mount edit it takes effect at the next launch or resume, which the
confirmation says. While an interaction is running its mounts are fixed, so
there is no question to ask: the path is inserted and the limit is stated.

Expansion is per-entry and inline: an expanded entry grows to show its detail
body and pushes later entries down, rather than opening a separate pane. This
keeps a single scrollable column, matching the wishlist's "history a list of
entries which can be expanded inline."

An entry whose detail is large (long command output, a diff) expands to a
bounded height with its own internal scroll while selected, so one noisy command
cannot bury the rest of the session. Rich external viewing of diffs (the
wishlist's "show the diff in two vim buffers") is a later hook: a `FileChanged`
entry can offer to open the change in a configured external viewer against a
temporary worktree, but the first form only summarizes the paths.

### Workspace and Session navigation

`V` opens the Workspace picker. The list is ordered once on entry — Workspaces
holding an Interaction the server still accepts input for first, then the rest
by recent access — and each such row is marked with a green dot and a count
(`2 live`), so where work is in flight is legible directly rather than only
implied by the ordering. The marker refreshes while the picker sits open; the
ordering does not, so a row does not move under the cursor looking at it.

The right-hand pane previews the screen `Enter` leads to: the selected
Workspace's notes above its Sessions, listed one line each — provider, name,
age — with live ones carrying the same green dot. Loading a Session list is a
round-trip to the server, so it waits for the cursor to settle (as the Session
picker's conversation preview does) and the pane says `loading…` until it
lands: a Workspace with no Sessions and an unread one must not read the same.

Choosing a Workspace then opens its Session picker; choosing a Session
attaches to its Interaction when live, otherwise it replays the stored
journal read-only. Neither step stops the Interaction the
client was previously viewing. An empty Workspace opens a blank pending Session
screen. `Esc`/`q` cancels without changing the current view.

Sending a message to a stopped, ended, or merely-viewed Session resumes it
automatically. It reopens the existing journal for append and starts a new
Interaction using the native provider id captured when the Session began. A
missing id or missing provider record is an error, without affecting
read-only journal replay; the typed message is preserved for a retry.

### Branching a Session

A Session can be branched: forked into a new sibling Session in the same
Workspace, seeded with its history up to some point, optionally under a
different provider. Provider conversion is the special case where the
branch point is the end of the history and the provider changes; a
same-provider branch at an earlier point is a checkpoint or retry point.
Both go through the same server operation, `branch_session` (`x`/`b` below
are its two client-facing shortcuts), built on Genta's session conversion
(see genta/README.md) with one addition: `ConversionOptions::keep_messages`
truncates the parsed history to a prefix before re-serializing it, which is
what "branch at an earlier point" means at the transcript level.

A branch is always a fresh copy, never a live reference: the source Session,
its native transcript, and its Styra journal are left untouched, and nothing
either side does afterwards is visible on the other — the same way a git
branch's fork point does not move when the source gets new commits. The
branch always gets its own fresh native provider session id, even when the
provider does not change, since the provider's own `--resume` lookup searches
its whole session tree by id and could not otherwise tell the two apart. The
result records where it came from (`SessionOrigin`: the source Session id,
its provider, and the cutoff, or no cutoff for a full branch), which is not a
live link — it is a historical fact, fixed at branch time.

The branch's Styra journal is seeded with the same leading history as its
native provider transcript, so opening the new Session shows the conversation
immediately rather than an empty preview. Copied agent records retain the wire
protocol that produced them. This matters for provider conversion: historical
Claude lines still decode as Claude while new Codex app-server lines appended
after resume decode as Codex (and conversely).

Two client-facing shortcuts:

- `x` in the Session picker converts the selected Session's native transcript
  to Styra's other interactive provider, keeping the whole history. Carries
  over the source's name and notes. The picker opens the new Session
  immediately on success, or shows the failure (most commonly a stored
  Session with no provider id, so nothing native exists to convert) without
  leaving the list.
- `b` in the event list branches the current Session under the *same*
  provider, seeded with history up to the selected entry (a checkpoint), or
  the whole history when the list is following the newest entry or the
  selection has no known wire line. Opens the branch immediately, the same
  as `x`. The cutoff is resolved by timestamp — Styra's own journal and a
  provider's native transcript are decoded differently and do not otherwise
  line up — so a `RawLine`'s `at_ms` (via the selected entry's `raw_index`)
  is compared against each native message's own timestamp.

### Current Interactions

`a` opens the server's live Interactions as a navigator above the main event
list. There is no separate picker or conversation preview: moving with `j`/`k`
marks the highlighted Interaction as this client's current target and sends a
preview request without waiting in the key handler. Its eventual payload fills
the ordinary event list below. The Interaction previously shown keeps running
on the server; only this client's current view changes.

Moving initially asks for only the five newest conversation events, and a newly
selected Interaction returns to conversation-only mode focused on its newest
visible entry. The returned cursor is still the true stream tail, so live
polling continues from there rather than filling in the omitted prefix. The
payload also carries the current lifecycle summary and durable input queue, so
populating the view needs no follow-up round trips. `Enter` confirms the
highlighted Interaction, requests its complete history including raw wire data,
and closes the navigator immediately. The same incoming-event handler applies
preview and full payloads whether or not the navigator remains open.

Every payload is tagged with its Interaction id and the local request
generation. Each Styra instance keeps its own active id and applies only a
matching latest payload. A late response for a row that instance has already
moved past—and an old preview arriving after a full request—is ignored. The
instance also sends the server an explicit cancellation for its outstanding
request before fetching the newly selected row; client-generated request ids
keep this cancellation private to that Styra instance. The
first `r` can also hydrate complete history on demand; subsequent raw-view
toggles use the local history.

The navigator refreshes its summaries while open. Pending work is listed
first, running work next, and stopped Interactions last, with server order
retained within each group. Each row shows the latest received agent message
on a subordinate line, updated along with those live summaries. All-Workspaces
mode always groups the rows beneath
Workspace headings; current-Workspace mode omits the one redundant heading.
`w` switches between those scopes. `a` or `Esc` closes the navigator while
leaving its lightweight tail current. `D` removes a highlighted
stopped Interaction from the server. The next available Interaction becomes
current without closing the navigator; deleting the last one closes it and
returns Styra to its blank default state.

### Starting sends nothing on its own

A bare `styra` invocation does not spawn the agent process by itself. It lands
in `Status::Pending` (`App::pending`): an `App`
with no session id yet, opened directly in input focus, and a `Live::Pending`
event-loop state that holds no session id or cursor. The operator's submitted
message is what triggers a `create_session`
request to the server — creating the journal, launching the sandboxed interaction
through Driva, and sending that first message once the agent is ready. A
launch failure (e.g. a missing binary) is reported by the server, logged in the
diagnostic view, and the typed message is restored to the box rather than lost,
so the operator can fix the problem and retry without retyping.

The one exception is a trailing CLI `PROMPT`: since that is already input the
operator gave (as a command-line argument, before the terminal even took
over), it launches immediately, exactly as it always has.

### Choosing what to launch

Because nothing is launched until the first message, the blank start screen is
also where *what to launch* is chosen. `L` (or `Ctrl+L`, since that screen opens
in input focus) opens a modal picker with three columns — agent, model, effort —
and applying it records a `Selection` on the `App`; the status line updates with
it, so the screen always names what an `Enter` would
start. Nothing is launched or sent by picking: the operator's own message still
does that, as everywhere else.

Every row of every column is a concrete choice out of the agent's own catalogs.
There is no row standing for "whatever
the agent is configured for", because no selection can express that: the picker
opens on the model and effort its current selection names, and both are always
named. Changing the agent resets both to that provider's declared defaults,
since neither the catalogs nor the effort ladders correspond across providers.

The model catalog is a fixed list per agent, not a free-text field: for Claude
Code it is every id Anthropic lists as `Active` in its model-status table, as
full ids rather than the moving `opus`/`sonnet` aliases, so a journal records the
exact model a session ran on. A model outside that list can survive in stored
state, and the picker carries it as a final row so confirming cannot silently
swap it for a catalogued model; it can carry such an id, never author one.

The picker is reachable in `Status::Pending`, and also while a Codex app-server
or Claude Code interaction is idle. Codex receives model and effort overrides
on the next `turn/start`. Claude receives an acknowledged `set_model` control
request before Styra releases the next user message; its effort remains fixed
because Claude's streaming control protocol exposes no corresponding setter.
Changing provider still requires a new Session. A running turn cannot be
changed in place.
Opening another Session (`V`) adopts that Session's recorded launch selection.

### The sandbox is shown before it exists

The driva view (`d`) answers "what can this agent touch": isolation backend,
command, working directory, network policy, and every mount. On a live
interaction that is a record, captured at spawn from the same `ExecutionRequest`
Driva executes. On the blank start screen there is nothing running to record,
but the policy is already decided — by the selection, the Workspace's standing
policy, and this launch's own inputs over it — so
the client asks the server for it with `plan_session` and shows that instead,
marked as planned. The server resolves it through the same profile, template and
mount resolution `create_session` uses, creating no session, journal or control
directory: the one thing a plan cannot name is the session id, so the broker
control mount carries a placeholder for the directory the launch will make.
Switching model before launch re-asks, since the profile a selection resolves to
carries its own mounts. The moment something launches, its own policy replaces
the plan.

### The sandbox is chosen where it is shown

While nothing has launched, the driva view is also where the policy is decided.
Networking (`w`), the Driva templates to layer (`T`), and extra host mounts (`m`
to add, `x` to remove) are all editable there.

Those inputs are a [`LaunchPolicy`], and there are two of them — so the view
shows two panes with the same rows in the same order, and every editing key acts
on whichever pane `Tab` has focused. The focused pane is the bright one, carries
the mount cursor, and is named in the key hints under both; the other is muted.
That is the whole of the difference between changing what this conversation
needs and changing what the work always needs, which was previously the
difference between pressing a key and pressing a key followed by `W`.

Most of what a
given body of work needs from its sandbox is a property of the work, not of one
interaction with an agent about it: the corpus that has to be readable, the
toolchain template, whether anything here may reach the network. That belongs to
the Workspace, so a Workspace stores one — in `workspace.json`, beside its notes
— and every launch there starts from it, from any client. What is particular to
one interaction is the other: `App::launch`, seeded from the saved defaults with
this invocation's flags over it.

`LaunchPolicy::merge` is where the two become one, and it lives on the server
because the server is what resolves the result. It is additive by default: the
overlay's templates layer after the Workspace's (so a repeated name moves later,
where it wins), its mounts add to them (except one landing on a destination the
Workspace already binds, which replaces it rather than colliding with it), and a
stated `network` overrides an inherited one. Adding cannot express *dropping*
something the Workspace grants, so `standalone` (`I`) does: that launch ignores
the Workspace's policy entirely and carries its own.

The two layers are kept in two different places, so an edit to each is made
durable differently. This client owns the overlay: it lives in memory until `D`
saves it as the client's standing default (only that half — saving the merge
would carry grants meant for one Workspace into launches everywhere else). The
server owns the Workspace's, and every launch path merges the *stored* one, so an
edit to that pane is sent the moment it is made rather than on request: a change
kept only in the client would be shown as part of the effective policy and then
quietly not applied. While such a send is outstanding or has failed, the pane
says `not stored` and `W` retries it; the planner also holds off, since a plan
made against the policy the server still has would not describe what is on
screen.

`U` is the bridge between the panes: it moves this interaction's settings up into
the Workspace's standing policy, storing the merge and emptying the overlay, so
what the next launch runs under does not change. It is for the setting that turns
out to belong to the work rather than to one conversation about it.

Every launch path — `create_session`, `plan_session`, `resume_session` — sends
the overlay and merges it against the Workspace on the server, which is what
makes the view editable at all: an edit is automatically re-planned (the plan is
keyed on the selection *and* the merged policy, so entering a Workspace with its
own grants re-asks too) and automatically applied when the operator's first
message finally starts the agent.

The client never resolves policy itself. It sends what the operator asked for —
template *names*, mount *requests* — and the server resolves both against the
Workspace's `driva.toml` and the host filesystem, so a bad path or an unknown
template is rejected at plan time, from the same code the launch would use. An
extra mount reaches Driva as an ordinary bind mount alongside the profile's own,
and the captured `DrivaOptions` therefore shows it without any special case. The
view still lists the effective set apart from both panes, because much of it is
in neither: the workspace, the profile's credential mounts and the broker's
control mount are not the operator's to drop from either layer. A grant that *is*
theirs is only ever in one pane, which is what says where to go to change it —
and `I`, which makes this interaction ignore the Workspace's policy entirely,
strikes that pane through rather than only reporting itself in a message.

Editing stops the moment an interaction exists. From then on the view is a
record of the sandbox an agent is confined to, and changing that means a new
session.

## Concurrency model

A live interaction runs entirely inside `styra-server`, on threads the interaction owns, each
forwarding over a channel to a per-interaction update collector:

- **Execution thread** — calls `driva::execute` and blocks for the interaction's
  lifetime; on return it sends the exit report.
- **Reader thread** — reads lines from the interaction's stdout pipe, records each
  verbatim to the journal, decodes it into a Styra event (or routes it through
  the app-server client), and forwards the result.
- **Stderr thread** — reads the agent's stderr, appends it to the diagnostics
  file, and streams each line to the log view as a diagnostic entry. Stderr is
  never interleaved into the event list.

The collector serializes these into a monotonically sequenced update history.
The `styra` TUI is a separate process and never touches the agent's pipes: it
owns terminal state and input, polls the update history over the socket with an
`after` cursor, and sends operator messages back over the socket for the server
to write to the interaction's stdin. Because updates are pulled by cursor rather than
pushed over a channel, a client can reconnect — or observe alongside other
clients — without coupling to the interaction's threads.

## Crate layout

The server-client split is also a crate split: two standalone crates, siblings
to `orka/` and `driva/`, linked by a plain path dependency (no workspace).

Three names describe distinct ownership:

- A **Workspace** is the durable top-level container and host-directory binding.
- A **Session** is one durable provider-native conversation, identity, and
  append-only journal within a Workspace.
- An **Interaction** is the optional live process, sandbox, pipes, protocol
  driver, shell, and update stream serving a Session.

Stopping an Interaction preserves its Session. Navigating to another Workspace
or Session only changes the client view. Native provider resume attaches a new
Interaction to an existing stopped Session.

```text
styra-server/            # the server application + its client interface library
  Cargo.toml             # [lib] styra_server  +  [[bin]] styra-server
  src/
    lib.rs               # curated interface; server modules are pub for the binary
    main.rs              # server binary: a thin CLI shim over daemon::run
    daemon.rs            # server bootstrap: socket bind, store setup, serve loop
    spawn.rs             # connect-or-spawn: re-exec self as a detached daemon
    api.rs               # JSON wire types (interface)
    client.rs            # blocking Rust client over the socket (interface)
    types.rs             # data vocabulary that crosses the wire (interface)
    paths.rs             # default socket/store locations (interface + server)
    server.rs            # socket dispatch and the server-owned interaction manager
    interaction.rs               # one live agent interaction: Driva launch, pipes, threads
    journal.rs           # raw event/input capture and replay
    workspace.rs         # Workspace metadata and hierarchy

styra/                   # the terminal client application
  Cargo.toml             # [[bin]] styra; depends on styra-server (path)
  src/
    main.rs              # CLI entry, terminal setup/teardown, event loop wiring
    app.rs               # the App struct: session status, views, and the state below
    timeline.rs          # the event list: rows, selection, filters, expansion
    ingest.rs            # how one AgentEvent changes that list and the status
    launch.rs            # the sandbox policy's two layers, and the keys that edit them
    mount.rs             # writing, reading and locating host mounts (pure)
    launcher.rs          # the agent/model/effort picker's state
    composer.rs          # the message buffer and prompt history
    notes.rs             # Session and Workspace notes: state, keys, persistence
    ui/                  # widget layout, one module per view
```

Each of those owns one field of [`App`] and everything that is only about it,
so a feature is a module rather than a scattering of fields and methods over
the state struct. Where a decision needs both — a mount added to the Workspace's
layer has to reach the server, a selection move has to reset the preview scroll
— the module makes the decision and hands back what happened, and `App` (or the
event loop) does the part that is not its business to know about.

The agent knowledge (`agent`), event decoding (`event`), app-server handshake
(`appserver`), and rendering (`render`) live in the `genta` library; the server
crate re-exports them, and the client crate reaches the event vocabulary,
`render`, and `agent::SandboxLayout` through `styra-server`'s interface rather
than depending on `genta` or `driva` directly.

The `styra-server` library deliberately exposes only what a client needs to
speak the API — `api`, `Client`, the `types` vocabulary, `paths`, and the
re-exported event/render surface. Its session-runner modules (`server`,
`interaction`, `journal`) are `pub` because the `styra-server` binary drives them,
but they are not part of the interface the client depends on. A headless client
example lives under `styra-server/examples/`.

Dependencies: `styra-server` depends on `driva` and `genta` (path),
`serde` / `serde_json`, `clap`, and `anyhow`; `styra` depends on
`styra-server` (path), `ratatui` with a `crossterm` backend, `clap`, and
`anyhow` — matching the suite's existing choices.

## Command-line surface

```text
styra [OPTIONS] [-- PROMPT]

  --socket <PATH>      styra-server socket (default: $XDG_RUNTIME_DIR/styra/styra.sock)
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (providers may default this on)
  --view <SESSION>     Open a captured journal read-only instead of launching
  -d, --daemon         Start the background daemon and exit (no interface)
  --stop               Stop the daemon on the socket and exit
```

The `styra` TUI is a client of `styra-server`, but it need not be started
separately: `--socket` selects the server, and if none is listening there the
TUI spawns one as a detached daemon by re-exec'ing its own executable with a
serve sentinel in the environment (`styra_server::spawn`). Because `styra`
links the `styra_server` crate, that re-exec'd copy *is* the server — there is
no second binary to locate or install — and it outlives the client so live
interactions survive detach and quit.

The daemon runs until it is stopped or killed; it does not retire itself when
idle. `-d`/`--daemon` brings it up in the background without opening the
interface (idempotent — a no-op if one is already listening), and `--stop`
asks the daemon on the socket to remove the socket and exit, ending any live
interactions it owns with it. Both are pure lifecycle commands: they act and exit
without touching the terminal. An optional trailing `PROMPT` seeds the first turn so a
session can start with one message already sent, launching the interaction immediately;
without it, the application opens in input focus with an empty box and launches
nothing until the operator submits a message (see *Starting sends nothing on
its own*). `--view` opens the view/replay path over a stored
journal; it decodes with the session's own recorded profile and protocol.

## Relationship to Orka and the wishlist

Styra is the "Session runner" from `wishlist.wiki`: an interactive agent session
in JSON, each output a single-line expandable entry, stoppable without losing
the context, with the context being the raw JSON. It is intentionally the
interactive sibling of an Orka attempt — same isolation via Driva, same
raw-log-is-truth stance, same versioned decoder discipline — so that a session
Styra captures can later be promoted into an Orka/Linka node with little
friction. That promotion path is a future integration, owned by Orka, and is not
part of Styra's first form.

## Further reading

- [`../driva/DESIGN.md`](../driva/DESIGN.md) — the isolation interface Styra uses.
- [`../orka/DESIGN.md`](../orka/DESIGN.md) — the non-interactive counterpart and
  the origin of the aligned event vocabulary and decoder discipline.
- [`../wishlist.wiki`](../wishlist.wiki) — the "Session runner" and interactive
  Driva UI entries this design realizes.

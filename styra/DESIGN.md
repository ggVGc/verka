# Styra design

## Purpose

Styra runs one interactive agent session in isolation and presents it as a
navigable terminal application. It uses Driva to execute the agent command with
deny-by-default isolation, speaks the agent's machine-readable protocol over
piped standard streams, and shows every agent output as a selectable one-line
entry that can be expanded in place. A message box lets the operator send input
to the running agent.

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

- launching one isolated agent process through Driva;
- the wire protocol spoken to that agent and its decoding into a small, stable
  event vocabulary;
- capturing the raw event journal verbatim as the session's fundamental record;
- a terminal application that lists events, expands and collapses them, and
  sends operator messages to the agent;
- session lifecycle: start, send, stop, and view the captured journal.

Styra does **not**, in its first form:

- discover, freeze, or record work in a Linka store (that is Orka's role);
- perform reviews or produce candidates (Orka and Nota);
- implement isolation (Driva);
- interpret which program produced a stream inside Driva (Driva transports
  bytes; Styra owns interpretation, exactly as Orka does).

Later phases — session forking, resuming an old context in a new session, and
switching models mid-context — are described under *Sessions and context* but
are explicitly out of the first milestone.

## Ownership and boundaries

- **Driva** owns isolation: mount policy, networking policy, backend selection,
  and connecting the isolated process to the standard streams Styra provides. It
  never interprets the bytes on those streams.
- **Styra** owns the agent profile (command, wire protocol, how a user message
  is encoded as an input line), the decoding of provider wire events into
  Styra's event vocabulary, the raw journal, and the whole terminal interface.

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
its run is one-shot; Styra instead passes the ends of OS pipes:

1. Styra creates two pipes: one for the child's stdin, one for its stdout. A
   third file receives stderr as diagnostics, as in Orka.
2. The child's stdin-read end and stdout-write end become the `ExecutionIo`
   handed to `driva::execute`. Styra keeps the stdin-write end and the
   stdout-read end.
3. `driva::execute` is called on a dedicated worker thread. It blocks for the
   life of the session (the agent process runs until it exits or is stopped),
   which is why it must not run on the UI thread.
4. A reader thread pulls newline-delimited JSON from the stdout-read end,
   decodes each line, and forwards events to the UI. The UI thread writes
   operator messages as protocol input lines to the stdin-write end.
5. Closing the stdin-write end signals end-of-input to the agent; dropping the
   child (session stop) tears the session down. The worker thread's return value
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
- `message_format` — how an operator message becomes one input line;
- `single_turn` — whether the agent reads one prompt to end-of-input and then
  runs to completion, so the session closes stdin after the first message.

The first profile targets the same provider Orka uses. Its wire event schema is
the `thread.started` / `turn.started` / `turn.completed` / `item.{started,
updated,completed}` family Orka already decodes, so the decoders stay aligned.
The protocol is versioned: a new wire format, or a new revision of an existing
one, is a new `protocol` variant plus a decoder arm, and the match is
exhaustive, so a missing decoder is a compile error rather than a silent
mis-decode. This is the same discipline as Orka's decoder registry.

### Interactive vs. single-turn, and the two codex profiles

The session and pipe machinery is genuinely bidirectional and multi-turn: it
holds the agent's stdin open and writes each operator message as it is sent.
Whether a session is *actually* multi-turn is a property of the agent's
protocol, carried by the profile's `single_turn` flag. Both codex profiles were
verified live against codex-cli 0.145.

The default `codex` profile is multi-turn over the experimental **`app-server`
JSON-RPC protocol** on stdio. This is a stateful wire contract, not a plain
event stream, so it has two cooperating parts:

- the `codex-app-server` `Protocol` variant decodes *notification* lines
  (`thread/started`, `turn/started`, `item/started`, `item/completed`,
  `thread/tokenUsage/updated`, errors) into the event vocabulary — shared by
  live sessions and journal replay; requests and responses decode as `Unknown`
  control traffic;
- an `AppServer` client owns the session state machine: `initialize` →
  `initialized` → `thread/start` (capturing the thread id) → one `turn/start`
  per operator message. Messages sent before the thread is ready are queued and
  flushed on readiness. The reader thread routes every line through this client;
  the client forwards decoded events and answers control traffic.

The turn's token usage arrives as `thread/tokenUsage/updated` just before
`turn/completed` (which itself carries none), so that notification is what maps
to `TurnCompleted` — flipping the status line to `waiting` between turns. The
server exits on stdin end-of-input, so stopping a session tears it down cleanly.
Threads are started with `approvalPolicy: never` and a `danger-full-access`
inner sandbox: approvals never stall a turn, and real isolation stays Driva's.
Any server-to-client request that does appear is surfaced in the log view
rather than silently dropped.

The `codex-exec` profile remains the one-shot alternative: `codex exec --json -`
reads the prompt from stdin, streams the `thread.`/`turn.`/`item.` events, and
exits — `single_turn`, so Styra closes stdin after the first message and the
session is one turn. Its simpler stream is also the format Orka's attempts
capture, which keeps the two applications' decoders aligned.

## Event vocabulary

Styra decodes provider wire events into a small, stable set that the UI
consumes. It is intentionally the same shape as Orka's `AgentEvent`:

- `ThreadStarted { thread_id }`
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
- **Resume / fork / switch model.** A new session is seeded by feeding a prior
  journal's context to a freshly launched agent — possibly a different
  profile, hence a different model — while preserving the original journal.
  Fork is resume that keeps both branches. These reuse the launch and decode
  paths and add no new persistence concept, so they stay cheap. The first cut
  (see *Session switching* below) does this the simple way: render the old
  journal to a text transcript and send it as the new session's opening
  message. A native alternative, deferred for now, is described next.

### Future idea: native resume instead of a rendered seed message

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
  journal. genta doesn't implement or send this RPC today (only
  `initialize` → `thread/start` → `turn/start`), so this is new protocol
  surface, not something already wired up. Forking works the same way
  (`forked_from_id`): a new thread inheriting the parent's history as context.
- **Claude Code** has no equivalent mid-stream RPC; resume is a
  process-launch-time CLI flag (`--resume <session_id>` / `--continue`) that
  reloads Claude's own locally stored transcript before the new process's
  first turn.

Using these would mean handing the freshly spawned agent process *its own*
thread/session id and letting its native resume machinery reconstruct context
from *its own* storage, rather than Styra reconstructing a prompt from *its*
journal. That should be more faithful (each agent's own well-tested
reconstruction of its own format) but is real new work — `thread/resume`
isn't in genta yet, and using it means "load a session" needs the *agent's*
thread id, not just a Styra session directory, plus the picker would need to
distinguish sessions it can natively resume from ones it can only seed as
text. Worth revisiting once the simple version is in and its limits (token
cost of the rendered transcript, fidelity of the reconstruction) are felt in
practice.

Journals live under a per-session directory in a Styra store (`.styra/` in the
workbench, separately owned from `.orka/` and `.linka/`), named by a session id.

Alongside `journal.jsonl`, one `session.json` is written once at session
creation: genta's `SessionMeta` (the profile name and wire protocol that
launched the session). The journal itself is agent-agnostic — it stores
whatever raw line arrived — so without this sidecar there is no record of
which agent a stored session came from. `--view` reads `session.json` and
decodes with the protocol it names; there is no `--profile` fallback, since an
operator-supplied guess could silently mis-decode a mismatched session. A
session predating this sidecar has no `session.json` and so cannot be viewed.

## Terminal interface

The application is a single full-screen view with three regions:

```text
┌───────────────────────────────────────── styra · codex · running ─┐
│  ▸ user     implement the retry backoff and add a test            │
│  ▸ plan     3 steps · 1 done                                      │
│  ▾ command  cargo test                                            │
│      status: completed (exit 0)                                   │
│      running 24 tests ...                                         │
│      test result: ok. 24 passed; 0 failed                         │
│  ▸ files    src/retry.rs, tests/retry.rs                          │
│  ▸ agent    Added exponential backoff capped at 30s; tests pass.  │
│  ▸ usage    in 4.1k · out 900 · cached 2.0k                       │
├───────────────────────────────────────────────────────────────────┤
│ › _                                                                │
└───────────────────────────────────────────────────────────────────┘
```

- **Event list (top).** One line per event: a type tag and its one-line summary.
  The list scrolls and auto-follows the tail while the newest entry is selected;
  moving the selection upward pins the view so incoming events do not yank it
  away.
- **Message box (bottom).** A single- or multi-line editor. Submitting sends the
  text to the agent (encoded by the profile) and appends a `UserMessage` entry
  to the list.
- **Status line (top border).** Application name, active profile/model, and
  session state: `running`, `waiting` (turn complete, agent idle for input),
  or `stopped`. Token usage from the latest `TurnCompleted` is shown.

### The raw view

The event list is one interpretation of the journal; the journal itself is the
verbatim wire interaction. `r` toggles the top region between the event list and
a **raw view** that shows that interaction undecoded, one wire line per row:
outgoing operator submissions marked `»` and incoming agent lines marked `«`, in
occurrence order. It is the same fact the decoder reads and the journal stores,
shown directly — useful for understanding an `Unknown`/`Malformed` event, or
just watching the protocol. The raw view anchors to the newest line and scrolls
back with `j`/`k` (`g`/`G` jump to top/bottom); a new line while scrolled up
keeps the current content in place rather than yanking to the tail. Under
`--view` the raw view is reconstructed from the stored journal.

### The log view

`l` toggles a **log view** for diagnostics that are neither agent events nor
wire lines: Styra's own notes (launch command, bytes sent, exit code, why a
message was not sent) and the agent's stderr streamed live. Entries are tagged
`info`/`warn`/`error`. The agent's stderr is the usual place a failure explains
itself — a missing credential, a rejected flag, a backend error — so streaming
it here (rather than only persisting it to `diagnostics.log`) is what makes a
session that produces no events diagnosable from inside the interface. The log
view shares the raw view's bottom-anchored scrolling.

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
| `zR` / `zM`     | Expand all / collapse all                                   |
| `g` / `G`       | Jump to first / last entry (`G` re-enables tail-follow)     |
| `r`             | Toggle the raw wire view (in the raw view, `j`/`k`/`g`/`G` scroll) |
| `l`             | Toggle the diagnostic log view (same scrolling as the raw view) |
| `i`             | Enter input focus                                           |
| `s`             | Stop the session (keeps the journal)                        |
| `q`             | Quit (prompts if the session is still running)              |

### Input-focus keys

| Key            | Action                                                       |
| -------------- | ------------------------------------------------------------ |
| `Enter`        | Send the message (configurable: `Enter` sends vs. newline)   |
| `Alt+Enter`    | Insert a newline (when `Enter` sends)                        |
| `Esc`          | Return to list focus without sending                         |

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

## Concurrency model

Three threads, communicating over channels:

- **UI thread** — owns terminal state and all rendering, reads input events, and
  writes operator messages to the stdin-write pipe. Never blocks on the agent.
- **Execution thread** — calls `driva::execute` and blocks for the session's
  lifetime; on return it sends the exit report to the UI thread.
- **Reader thread** — reads lines from the stdout-read pipe, decodes each into a
  Styra event, appends it to the journal, and forwards it to the UI thread.

Diagnostics (stderr) are captured to a file as Orka does and surfaced on demand;
they are not interleaved into the event list.

## Crate layout

A standalone binary crate, sibling to `orka/` and `driva/`:

```text
styra/
  Cargo.toml
  DESIGN.md
  README.md
  src/
    main.rs      # CLI entry, terminal setup/teardown, event loop wiring
    app.rs       # application state: list, selection, focus, session status
    session.rs   # Driva launch, pipe plumbing, execution + reader threads
    agent.rs     # agent profiles: command, protocol, message encoding, mounts
    event.rs     # wire decode -> Styra events; summary + detail rendering
    journal.rs   # raw event/input capture and replay
    ui.rs        # widget layout: list, message box, status line
```

Dependencies: `driva` (path), a terminal UI library (`ratatui` with a
`crossterm` backend), `serde` / `serde_json`, and `anyhow` — matching the
suite's existing choices.

## Command-line surface

```text
styra [OPTIONS] [-- PROMPT]

  --profile <NAME>     Agent profile to launch a live session with (default: codex)
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (profiles may default this on)
  --view <SESSION>     Open a captured journal read-only instead of launching
```

An optional trailing `PROMPT` seeds the first turn so a session can start with
one message already sent; without it, the application opens in input focus with
an empty box. `--view` opens the view/replay path over a stored journal; it
decodes with the session's own recorded profile and protocol, so `--profile`
is not read in this mode.

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

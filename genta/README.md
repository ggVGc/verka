# Genta

Genta is a standalone library of coding-agent knowledge: it knows how to launch
an agent, what it says, and how to read it back. It owns nothing else — no
graph, no orchestration, no isolation mechanism, and no process.

## What it owns

- **Launch profiles** (`agent`) — the command line, mounts, environment, and
  sandbox layout for an agent, including Codex (`exec`, `app-server`) and
  Claude Code. A profile is a description, not an invocation. A `Selection`
  names one as `provider:model/effort` (`codex:gpt-5.6-sol/high`,
  `claude:claude-opus-5/xhigh`), translating the model and reasoning effort into
  whichever flags or config overrides that agent takes. A selection always pins
  both: the shorter `provider[:model][/effort]` forms parse, but fill in that
  provider's declared defaults rather than standing for "unset", so a name always
  states what it launches.
- **Wire protocols** (`event`) — decoding each agent's output into one stable
  event vocabulary, so hosts do not carry per-agent parsing.
- **The `codex app-server` handshake** (`appserver`) — the stateful exchange
  that protocol requires.
- **Transcript rendering** (`render`) — turning a decoded event stream into
  readable output.
- **Native-session conversion** (`session`) — move the model-visible history
  between Claude Code's resumable JSONL sessions and Codex CLI rollout JSONL
  files.  Tool interactions are retained as readable context notes because the
  providers do not share executable tool-call semantics.

## Boundary

Genta is transport-agnostic: it never spawns processes and never owns pipes.
A host launches the agent through its own executor and feeds the output lines
to Genta's decoders. Genta has no dependency on any other application in this
repository, and knows nothing of work graphs, attempts, candidates, or reviews.

Two hosts use it today. Orka pairs a Genta profile with a Driva execution
request and decodes the resulting stream into attempt evidence. Styra, through
`styra-server`, uses the same profiles and decoders for its own sessions.
Neither dependency is visible to the other, and Genta is aware of neither.

## Convert a native session

`genta convert` operates on persisted sessions, rather than live `stream-json`
or `codex --json` output. It assigns a fresh UUID by default so the converted
file can coexist with its source session.

```sh
genta convert --from claude --to codex ~/.claude/projects/.../SESSION.jsonl converted.jsonl
genta convert --from codex --to claude rollout-...jsonl converted.jsonl
```

Use `--session-id` when a caller needs to choose the target id and `--cwd` when
the target should resume in a different workspace. The generated JSONL is
intended to be placed in the destination CLI's normal session directory.

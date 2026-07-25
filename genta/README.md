# Genta

Genta is a standalone library of coding-agent knowledge: it knows how to launch
an agent, what it says, and how to read it back. It owns nothing else — no
graph, no orchestration, no isolation mechanism, and no process.

## What it owns

- **Launch profiles** (`agent`) — the command line, mounts, environment, and
  sandbox layout for an agent, including Codex (`exec`, `app-server`) and
  Claude Code. A profile is a description, not an invocation.
- **Wire protocols** (`event`) — decoding each agent's output into one stable
  event vocabulary, so hosts do not carry per-agent parsing.
- **The `codex app-server` handshake** (`appserver`) — the stateful exchange
  that protocol requires.
- **Transcript rendering** (`render`) — turning a decoded event stream into
  readable output.

## Boundary

Genta is transport-agnostic: it never spawns processes and never owns pipes.
A host launches the agent through its own executor and feeds the output lines
to Genta's decoders. Genta has no dependency on any other application in this
repository, and knows nothing of work graphs, attempts, candidates, or reviews.

Two hosts use it today. Orka pairs a Genta profile with a Driva execution
request and decodes the resulting stream into attempt evidence. Styra, through
`styra-server`, uses the same profiles and decoders for its own sessions.
Neither dependency is visible to the other, and Genta is aware of neither.

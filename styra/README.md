# Styra

Styra runs one interactive agent session in isolation and presents it as a
navigable terminal application. It uses [Driva](../driva) to execute the agent
with deny-by-default isolation, speaks the agent's machine-readable protocol
over piped standard streams, and shows every agent output as a selectable
one-line entry that can be expanded in place. A message box sends input to the
running agent.

Styra is the interactive counterpart to an [Orka](../orka) attempt: the same
isolation, the same raw-event-journal-as-truth stance, but steered turn by turn
by an operator rather than run to completion against a Linka node. It depends
only on Driva.

See [`DESIGN.md`](DESIGN.md) for the architecture and [`TASKS.md`](TASKS.md) for
the implementation plan.

## Usage

```sh
styra [OPTIONS] [-- PROMPT]

  --profile <NAME>     Agent profile to launch (default: codex)
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (profiles may default this on)
  --attach <SESSION>   Open a captured journal read-only instead of launching
```

Built-in profiles:

- `codex` — multi-turn session over the codex `app-server` JSON-RPC protocol;
  each submitted message starts a new turn in the same thread.
- `codex-exec` — one-shot `codex exec --json`; the first message is the prompt
  and the session ends when the turn completes.
- `claude` — multi-turn session over Claude Code's bidirectional `stream-json`
  mode; each submitted message starts a new turn in the same session.
  `claude:<model>` (e.g. `claude:opus`) pins a model.

Two focuses, like vim modes: list focus navigates and folds the event list,
input focus types into the message box. `i` or `Tab` enters input focus; `Esc`
or `Tab` returns to list focus.

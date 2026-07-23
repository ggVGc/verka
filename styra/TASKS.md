# Styra implementation tasks

Ordered, independently reviewable tasks realizing [`DESIGN.md`](DESIGN.md).
Each is committed on its own. Check off as completed.

- [x] **1. Scaffold.** Crate `Cargo.toml` (driva path dep, ratatui/crossterm,
  serde/serde_json, anyhow, clap), `README.md`, this task list, and a minimal
  `main.rs` that compiles and runs. Wire into `build_all.sh`.
- [x] **2. Event vocabulary and decode (`event.rs`).** The stable `StyraEvent`
  set, a versioned `Protocol`, a decoder for the codex item/thread/turn wire
  schema, terminal-escape cleaning, and summary + detail rendering. Unit tested.
- [x] **3. Journal (`journal.rs`).** Verbatim capture of the agent event stream
  and the operator input log; replay of a stored journal into events. Unit
  tested.
- [ ] **4. Agent profile (`agent.rs`).** The `Profile` (command, protocol,
  mounts, environment, network) and outgoing-message encoding; the built-in
  codex interactive profile and its Driva isolation policy. Unit tested.
- [ ] **5. Session (`session.rs`).** Driva launch with piped stdin/stdout, the
  execution and reader threads, and the channel protocol delivering events and
  lifecycle changes to the UI. Journal writing wired in.
- [ ] **6. Application state (`app.rs`).** The event list, selection, per-entry
  expand/collapse, focus (list vs. input), the message buffer, and session
  status. Pure state transitions, unit tested.
- [ ] **7. Rendering (`ui.rs`).** The ratatui layout: event list with summaries
  and inline expansion, the message box, and the status line.
- [ ] **8. Event loop (`main.rs`).** CLI arguments, terminal setup/teardown,
  input handling per focus, and wiring the session threads to the app and
  renderer. `--attach` replay path.

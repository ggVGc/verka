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
- [x] **4. Agent profile (`agent.rs`).** The `Profile` (command, protocol,
  mounts, environment, network) and outgoing-message encoding; the built-in
  codex interactive profile and its Driva isolation policy. Unit tested.
- [x] **5. Session (`session.rs`).** Driva launch with piped stdin/stdout, the
  execution and reader threads, and the channel protocol delivering events and
  lifecycle changes to the UI. Journal writing wired in.
- [x] **6. Application state (`app.rs`).** The event list, selection, per-entry
  expand/collapse, focus (list vs. input), the message buffer, and session
  status. Pure state transitions, unit tested.
- [x] **7. Rendering (`ui.rs`).** The ratatui layout: event list with summaries
  and inline expansion, the message box, and the status line.
- [x] **8. Event loop (`main.rs`).** CLI arguments, terminal setup/teardown,
  input handling per focus, and wiring the session threads to the app and
  renderer. `--view` replay path.
- [x] **9. Server-client split.** Versioned JSON API over a local Unix socket,
  server-owned live sessions and journal replay, cursor-based updates, reusable
  Rust client, and migration of the TUI and headless example.

## Workspace / Session / Interaction redesign

The vocabulary and ownership model for the next storage/API version is:

```text
Workspace
└── Session
    └── Interaction (zero or one live)
```

- [x] **10. Interaction terminology.** Rename the live `Track` process/protocol
  wrapper and all of its server, API, client, TUI, test, and documentation
  vocabulary to `Interaction`, without changing behaviour.
- [x] **11. Workspace storage.** Add durable Workspace metadata and nest new
  Session directories below their owning Workspace. Keep the Session journal
  as the durable provider-conversation record.
- [x] **12. Workspace API.** Bump the wire API version and add create, list, and
  inspect operations for Workspaces plus Workspace-scoped Session creation and
  listing. Key live Interactions by their Session IDs.
- [x] **13. Workspace-aware TUI.** Make `V` choose a Workspace, browse the
  Workspace's Sessions separately, and attach to a live Interaction when the
  selected Session has one. Switching views must not stop unrelated
  Interactions.
- [x] **14. Session lifecycle.** Make stopping affect only the live Interaction;
  keep its Session and Workspace intact. Support opening stored Sessions without
  an Interaction.
- [x] **17. Native resume.** Store provider conversation ids and resume stopped
  Sessions with Codex `thread/resume` or Claude Code `--resume`; retain
  view-only access when native state is unavailable.
- [x] **15. Strict Workspace store.** Require Workspace ownership and complete
  Session metadata; reject invalid Session entries.
- [x] **16. Documentation and validation.** Rewrite the architecture, CLI/API
  reference, and tests around Workspace → Session → Interaction, then run all
  Styra and repository-level checks.

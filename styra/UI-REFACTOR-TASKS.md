# Styra UI crate refactor tasks

Status: implementation complete. Automated validation is recorded below;
interactive terminal exercises remain an environment limitation.

Completed slices:

- [x] Create the `styra-ui` workspace crate and use it from `tui`.
- [x] Move terminal setup, restoration, drawing ownership, and input polling
  into `RatatuiUi`; add `MockUi` for non-terminal loop tests.
- [x] Route the main loop and standalone picker loops through `&mut dyn Ui`.
- [x] Move the palette, code styling, Markdown renderer, help renderer, modal
  input, insert prompt, branch/tag/reference overlays, status messages, and
  log renderer into `styra-ui`.
- [x] Replace help's rendering-time scroll-limit mutation with an explicit
  returned limit applied by the TUI.

Extract all Ratatui rendering and visual layout from `styra/tui` into
`styra/ui`. The TUI application must use one consistent `Ui` trait so tests can
substitute a mock. Keep application behavior, server communication, and effect
execution in the TUI crate. Preserve current appearance and interaction behavior.

Tasks below are ordered by dependency. Check a task off only when its acceptance
criteria and applicable validation are complete. Keep intermediate changes
buildable; temporary migration adapters must be removed before completion.

## Target architecture

```text
tui ──→ ui ──→ ratatui
 │       │
 └──→ protocol ←──┘
 │
 └──→ server
```

Use package `ui` and library name `styra_ui`, following the existing
`protocol`/`styra_protocol` and `server`/`styra_server` convention. Confirm the
package name is available in the workspace before adding it.

The UI crate defines its trait, presentation models, rendering feedback, and
concrete `RatatuiUi` implementation. It must not depend on `tui` or `server`.
Existing protocol types can cross the boundary as data; application controllers
and server clients cannot. Do not add UI-specific types to the wire protocol.

Proposed interface, to refine during task 2:

```rust
pub trait Ui {
    fn render_application(&mut self, view: &ApplicationView<'_>)
        -> Result<RenderFeedback, UiError>;
    fn render_session_picker(/* presentation data */)
        -> Result<RenderFeedback, UiError>;
    // One method per independently rendered application or picker view.

    fn close(&mut self) -> Result<(), UiError>;
}
```

Each method accepts the presentation model for one application or standalone
picker view, including overlays and loading states where relevant. Models
contain content and semantic presentation state, not widgets, styles,
rectangles, or drawing callbacks. The TUI passes `&mut dyn Ui` through runtime
paths. Only composition at startup constructs the concrete implementation.
Terminal setup belongs to that implementation; cleanup must also work when
errors bypass explicit `close`.

### Ownership rules

| Concern | Owner |
| --- | --- |
| Server calls, polling, updates, attachment and session lifecycle | TUI |
| Input bindings, commands, editing, focus and selection transitions | TUI |
| Launch policy, permissions, queued messages and preferences | TUI |
| Event filtering, ordering and application-derived content | TUI |
| File resolution, content loading and external process execution | TUI |
| Mapping application state into presentation models | TUI |
| Layout, colors, borders, wrapping, Markdown and code styling | UI |
| Terminal backend, drawing, cursor placement and terminal restoration | UI |
| Ratatui widget state and rendering caches | UI |
| Requested scroll position and navigation intent | TUI |
| Layout-derived scroll limits and effective offsets | UI, returned as feedback where needed |

## 1. Inventory the boundary and establish a baseline

- [x] Record existing TUI test results before code changes. Distinguish existing
  failures from failures introduced by the refactor.
- [x] Inventory Ratatui imports and drawing entry points in `tui/src/ui/`,
  `main.rs`, `event_loop.rs`, `picker.rs`, and `terminal.rs`.
- [x] Map each renderer's reads from `App` and other TUI types to the data it
  actually needs. Include helpers currently imported from `session`, `files`,
  `workspace`, `mount`, `keymap`, and `activity`.
- [x] Inventory rendering writes: `Scroll::note_limit`, help scroll limits,
  and `Timeline::list_offset`. Document which navigation code reads them.
- [x] Inventory rendering-time filesystem access, including the files view,
  path resolution, and any content acquisition reached indirectly by helpers.
- [x] Classify current tests as application behavior, presentation mapping,
  renderer output, or combined behavior requiring a split.
- [x] Record representative existing screens and edge cases for comparison:
  narrow and tiny terminals, long histories, Unicode, Markdown, nested modals,
  empty/loading/error states, and scrolling at either end.

Acceptance: every drawing entry point, reverse dependency on application
types, and rendering side effect has an assigned destination.

## 2. Define the crate and public UI contract

Depends on: task 1.

- [x] Add `styra/ui/Cargo.toml`, `src/lib.rs`, and workspace membership.
- [x] Define the object-safe `Ui` trait and an error type with no Ratatui types
  in its public contract. Document draw and cleanup failure behavior.
- [x] Define direct trait methods for the application, session picker, and
  workspace picker. Represent picker message and rename prompts within the draw
  contract, so no caller needs to compose widgets or invoke renderer functions.
- [x] Define focused models for main views, shared chrome, messages, input,
  help, launcher, template selection, and overlays. Model loading, empty,
  ready, and failed content explicitly where the existing behavior needs them.
- [x] Reuse suitable protocol records. Introduce presentation-only records for
  TUI-owned types rather than exposing `App` or moving whole controllers.
- [x] Prefer borrowing large strings and histories. Document any owned data or
  allocation needed for derived content; avoid cloning all history each frame.
- [x] Specify time-dependent content: the TUI supplies elapsed/progress values
  so renderer fixtures do not depend on wall-clock timing.
- [x] Specify semantic identities for sessions, panels, and preview targets so
  renderer state can be reset or retained deliberately.
- [x] Specify overlay precedence and focus/cursor ownership for combinations
  already supported by the application.
- [x] Add a minimal recording mock that demonstrates the trait is implementable
  without terminal initialization or Ratatui access.

Acceptance: the contract can describe every current screen, is usable through
`dyn Ui`, and has no dependency on TUI application types or server clients.

## 3. Add the TUI presentation adapter

Depends on: task 2.

- [x] Introduce a TUI module that maps `App` and picker state into UI models.
  Keep mapping separate from server polling and effect execution.
- [x] Supply visible timeline entries, selection/expansion state, activity,
  launch labels, workspace/session metadata, notices, and message state.
- [x] Supply the data required by raw/provider-raw, log, entry-log, transcript,
  quota, files, answer, preview, and Driva views.
- [x] Supply launcher, template, tag, reference, branch, insertion, help, and
  composer state without exposing their controllers to the UI crate.
- [x] Keep filtering and session ordering authoritative in the TUI. Pass tree
  depth or other derived structure where needed; let the UI choose indentation
  and other visual representation.
- [x] Supply help rows from the same key definitions that drive behavior.
- [x] Separate reusable content derivation from terminal styling. Preserve
  transcript/export and clipboard behavior where content is shared.
- [x] Move file reads and path resolution out of rendering. Prepare content
  and errors in the TUI before drawing, preserving existing refresh behavior
  and avoiding an unrelated asynchronous-loading redesign.
- [x] Audit helpers individually: move visual formatting to UI, retain domain
  decisions in TUI, and split helpers that currently perform both.
- [x] Test meaningful mappings, including filtered selection, reported versus
  requested model labels, loading/error content, and modal precedence.

Acceptance: the UI receives all needed data without accessing `App`, calling
the server, resolving application paths, or reading displayed files itself.

The adapter is named `tui/src/presentation/`, rather than `tui/src/ui/`, to
make this ownership boundary visible in the source tree. It contains model
mapping and TUI-to-UI integration tests, not widgets or rendering code.

## 4. Replace hidden rendering mutations with explicit feedback

Depends on: tasks 2–3.

- [x] Define feedback for layout-dependent scroll limits and effective offsets
  used by application navigation. Distinguish panels using semantic identifiers.
- [x] Replace render-time writes through `Cell` with feedback returned by a
  successful draw and applied by the TUI before subsequent input handling.
- [x] Keep Ratatui `ListState` and renderer-only caches inside the UI. Return
  an offset only where the TUI actually needs it.
- [x] Clamp the displayed position against the current frame's measurements,
  so resizing does not require a second draw to produce a valid viewport.
- [x] Define behavior for panels absent from a frame, zero-sized viewports,
  and sessions or preview targets changing between draws.
- [x] Preserve follow-tail, selection visibility, page movement, end jumps,
  and scroll resets after filtering or changing selection.
- [x] Test long wrapped previews, repeated PageDown at the bottom followed by
  PageUp, resize while scrolled, help scrolling, and session/view switches.
- [x] Remove interior mutability that existed solely for rendering feedback.

Acceptance: rendering cannot mutate application state indirectly, and both a
real UI and a mock can report the measurements required for navigation.

## 5. Migrate rendering modules

Depends on: tasks 3–4. Move in small groups, compiling and running relevant
tests after each group. Add private UI modules as needed; expose the trait and
models rather than a collection of public rendering functions.

- [x] Move shared palette, Markdown, code styling, and modal input. Formatting,
  status indicators, and frame composition helpers remain.
- [x] Move shared chrome: titles, footer, composer/input, and help. Status
  messages and help are complete; the rest remain.
- [x] Move event list, inline details, interaction navigator, and entry log.
- [x] Move preview and fullscreen preview, transcript, raw/provider-raw, and log.
- [x] Move quota, files, typed answer, and Driva views.
- [x] Move launcher and template selection, including loading states.
- [x] Move reference, tag, branch, and insertion/grant overlays.
- [x] Move session/workspace pickers, previews, rename prompts, and messages.
- [x] Preserve every existing `View` variant and the existing overlay order.
- [x] Replace all renderer imports of TUI modules with presentation models or
  private UI helpers. Remove obsolete re-exports and migration wrappers.
- [x] Move rendering dependencies and their required features, including
  Ratatui's `unstable-rendered-line-info`. Retain dependencies in TUI when
  non-rendering code still uses them, such as Markdown parsing for file links.

Acceptance: all layouts and Ratatui widgets live in `ui`; the crate builds and
its renderers work from fixtures without the TUI crate.

## 6. Encapsulate terminal ownership and integrate the trait

Depends on: tasks 2 and 5.

- [x] Implement `RatatuiUi` with an owned terminal/backend and private draw
  dispatch. Keep a test-backend construction path for renderer tests.
- [x] Move raw-mode setup, alternate-screen entry/exit, cursor restoration,
  and terminal flushing from `tui/src/terminal.rs` into the implementation.
- [x] Handle partial initialization failure, explicit close, drop after an
  error, and repeated cleanup. Review panic cleanup without hiding the
  original failure or needlessly changing global panic behavior.
- [x] Keep shell lookup, external terminal/editor commands, and process
  execution in the TUI; rename or split its terminal module if helpful.
- [x] Replace concrete terminal parameters in the main event loop and every
  picker loop with `&mut dyn Ui`.
- [x] Replace draw closures with presentation-adapter calls and direct `Ui`
  rendering methods.
- [x] Apply feedback after successful draws and propagate draw failures through
  the existing application error path.
- [x] Preserve lazy terminal initialization and reuse between startup pickers
  and the main application. Cover `--view` and shell browsing entry paths.
- [x] Keep CLI-only output and commands from initializing the interactive UI.
- [x] Ensure cancellation, normal quit, early return, and application errors
  all restore the terminal.
- [x] Remove direct Ratatui imports and the Ratatui dependency from `tui`.

Acceptance: all interactive drawing goes through `Ui`; runtime code receives
the abstraction, and concrete construction is confined to startup wiring.

## 7. Split tests and make application flows mockable

Depends on: tasks 3–6. Migrate tests alongside the modules they cover rather
than deferring all testing until the end.

- [x] Port `ui/testing.rs` to presentation fixtures and `TestBackend`. Preserve
  explicit fixture defaults and region-based assertions for titles, bodies,
  footers, cursors, and styles.
- [x] Keep state-transition and server-related tests in TUI. Split mixed tests
  into application/mapping assertions and rendering assertions.
- [x] Ensure the UI crate has no test dependency back on TUI or the server.
- [x] Implement a `MockUi` that records relevant presented values, returns
  configurable feedback, and can simulate draw and close failures.
- [x] Add a small injectable input source or equivalent loop-step boundary for
  the main loop and standalone pickers. Production continues using Crossterm;
  tests provide keys, resize events, and idle ticks deterministically.
- [x] Keep input bindings and command interpretation in TUI. Input injection
  is a testing seam, not a move of business behavior into the renderer.
- [x] Reuse available server fixtures/fakes where applicable. Introduce only
  the narrow additional seams required by flow tests; avoid a server redesign.
- [x] Test startup picker cancellation and selection, view/modal changes,
  rendering after updates, scroll-feedback application, and draw-error exits
  without a real terminal.
- [x] Test representative pending operations, such as template loading and
  its cancellation, continue to follow the existing event-loop behavior.
- [x] Confirm retained renderer tests cover Unicode/wrapping, tiny dimensions,
  empty states, focus styling, and cursor placement.

Acceptance: application flow tests can replace the UI without a terminal;
renderer tests exercise visuals without constructing the application.

## 8. Validate, document, and remove migration scaffolding

Depends on: tasks 1–7.

- [x] Run focused checks after each implementation stage. At completion run:

  ```sh
  cargo fmt --all -- --check
  cargo check -p ui -p tui --all-targets
  cargo test -p ui -p tui
  cargo check --workspace --all-targets
  cargo test --workspace
  ```

- [x] Run any additional repository-required checks discovered during the
  baseline inventory. Record environment-dependent or pre-existing failures
  separately; do not mark failed checks as passed.
- [x] Audit dependencies with `cargo tree -p ui` and `cargo tree -p tui`.
  Verify no dependency cycle, no UI dependency on server/TUI, and no direct
  Ratatui dependency in TUI. Distinguish unrelated transitive dependencies.
- [x] Search for remaining Ratatui types, widget construction, layout
  calculations, direct draw calls, and stale `crate::ui` references in TUI.
- [x] Audit UI code for server calls, content-loading filesystem access,
  application effect execution, and hidden state mutation.
- [ ] Manually exercise startup workspace/session selection, rename/message
  prompts, live updates, stored-session viewing, switching, and shell browsing.
- [ ] Manually exercise all main views and overlays, narrow-terminal resizing,
  scrolling/follow-tail, Unicode input, and external shell/editor actions.
- [ ] Verify terminal restoration on quit, cancellation, and representative
  errors. Confirm noninteractive commands remain noninteractive.
- [x] Compare representative screens and behavior with the baseline; investigate
  differences rather than updating assertions merely to accept regressions.
- [x] Review large-history mapping and redraw costs for new full-history clones
  or repeated content loading introduced by the split.
- [x] Update `styra/DESIGN.md`, relevant README documentation, and module docs
  to describe the trait, ownership, models, feedback, and test boundaries.
- [x] Remove the old renderer tree, transitional adapters, unused dependencies,
  and stale documentation links once all consumers have migrated.

Acceptance: the workspace checks are accounted for, visual and behavioral
parity is verified, and the documented dependency boundary matches the code.

### Completion validation

- Baseline before the final scaffolding removal: 58 `styra-ui` tests and 371
  TUI tests passed. Renderer-only tests were replaced by UI fixtures; the final
  focused result is 59 `styra-ui` tests and 286 TUI/application tests passing.
- `cargo check -p styra-ui -p tui --all-targets`: passed.
- `cargo test -p styra-ui -p tui`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo test --workspace`: passed.
- Dependency audit: `styra-ui` has no TUI/server dependency; TUI has no direct
  Ratatui dependency. Searches found no Ratatui types, widgets, layout, or draw
  callbacks in `styra/tui`, and no server calls, filesystem reads, effects, or
  interior mutability in `styra/ui`.
- `cargo fmt --all -- --check` remains blocked by pre-existing formatting in
  `linka/src/model.rs`, `orka-web/src/main.rs`, and five `styra/server` source
  locations. `cargo fmt -p tui -p styra-ui` passes for the refactored packages.
- Interactive terminal validation was not performed in this non-interactive
  environment. Automated picker, modal, view, feedback, cleanup-contract, and
  renderer tests cover those paths without opening a terminal.

## Suggested review sequence

1. Baseline inventory, crate scaffold, trait and presentation contract.
2. Presentation adapter and explicit rendering feedback.
3. Renderer migration in coherent groups, with their fixture tests.
4. Terminal ownership and all runtime/picker paths using the trait.
5. Application-flow mocks, input injection, final cleanup and documentation.

These are review boundaries, not a requirement to leave incomplete migrations
on the main branch. Adjust grouping to keep each landed change buildable and
tested. No stage should require moving business logic into UI to break a cycle.

## Completion checklist

- [x] `styra/ui` owns all Ratatui rendering, visual layout, and terminal drawing.
- [x] TUI application and picker paths consume one `Ui` trait.
- [x] UI models and feedback expose no Ratatui types or application controllers.
- [x] Server interaction and application effects remain in TUI.
- [x] Renderer state changes cannot silently mutate `App`.
- [x] UI mocks support application tests without opening a terminal.
- [x] Rendering tests use independent presentation fixtures.
- [x] Current appearance, navigation, scrolling, and terminal cleanup are preserved.
- [x] Required checks and manual validation are recorded, with limitations stated.

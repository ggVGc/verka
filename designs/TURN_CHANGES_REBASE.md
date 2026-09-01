# Turn-change capture: rebase notes

Status: branch note for `feature/styra-turn-changes`, written when the branch's
single commit was rebased onto `main` (`9c9de00`). It records what the rebase
had to reconstruct, what has not been verified, and what should be settled
before the branch merges. Delete it when the branch lands.

## 1. What the branch does

The server records the net Git-visible workspace delta for each conversation
turn, independently of provider file-change events. A private Git index
snapshots the host worktree immediately before a message is dispatched and
again when the turn completes; the difference is kept with the Session as a
binary-capable patch (`turn-changes.v1.jsonl`). Writes made by shell commands,
formatters, and other subprocesses are therefore observed even where the
provider reports nothing. The client keys those deltas by a host-assigned turn
number and shows the selected response's diff in the Files view.

## 2. What the rebase had to reconstruct

`main`'s `158c00c` ("Refactor app.rs") moved most of the code this commit
touched, so six files conflicted and two of them non-trivially.

* **`Entry` moved from `app.rs` to `timeline.rs`.** The `turn: Option<u64>`
  field was re-added there.
* **`push_event` moved from `app.rs` to `ingest.rs`.** The turn-open/close
  logic was reimplemented as `App::open_turn_if` and `App::close_turn`, called
  from `ingest::push_event`. `App::selected_turn_changes` now reads
  `self.timeline.selected_entry()`.
* **`server.rs`, `Request::SendMessage`.** `main`'s contract framing was kept
  and routed through the branch's `ManagedInteraction::send_with_selection`
  wrapper, so the tracker's `begin()` still fires on a typed turn.
* **Export lists** in `lib.rs` and `protocol/mod.rs` were unioned.
* **`ui/driva.rs`.** The branch's hunk there was unrelated drift against an API
  `main` has since replaced (`add_git_root_mount` became
  `launch::add_git_history`). It was dropped; no part of the feature lived in
  it.
* **`keymap.rs`.** `main` condensed the reference rows the branch edited. The
  wording change ("selected-response/all-session files") was carried into the
  condensed row.

## 3. Not verified

Nothing in this branch has been compiled or tested since the rebase. The
machine the rebase was performed on carried only Rust 1.60, which cannot parse
this workspace's manifest (workspace-inherited `package.version`). Every
resolved file was syntax-checked with `rustfmt --edition 2021` — all parse and
all are formatting-clean — and the type and field usage was traced by hand, but
that is not a substitute. Run `cargo test --workspace` before trusting the
result, particularly around `app.rs`, whose structure was largely re-derived.

## 4. Open defect: turn numbering can drift

The client and the server number turns independently. `App` increments on each
`UserMessage` event; `turn_changes::Tracker` increments on each `begin()`, and
`begin()` unconditionally replaces any in-flight `ActiveTurn`.

A message dispatched while the previous turn is still open therefore does two
things: it discards the earlier turn's before-snapshot, and it moves the two
counters out of step. Once they disagree, the Files view attributes a diff to
the wrong response. `main`'s durable message queue makes this more reachable
than it was when the branch was written, since a queued message is dispatched
by the client as an ordinary `SendMessage` as soon as one is taken.

This is the branch's own defect rather than something the rebase introduced,
but it should be fixed before merge. Either the server should refuse to open a
turn while one is active (folding the message into the open turn), or the turn
number should have a single owner and travel with the message.

## 5. Relevance

The premise is untouched by the eight commits `main` gained in the interim, and
nothing there supersedes it. It composes with the newer work: turn numbering
rides on `UserMessage`, which typed-turn unframing preserves, and
`journal::user_message_count` still matches `Record::User`.

Two things decayed. The `driva.rs` hunk is dead, as above. The keymap and
README copy assume the pre-refactor Files view; the wording was updated during
the rebase, but the rendered Files pane has not been looked at since, and
should be.

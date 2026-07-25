# Nota design

> **Status: aspirational.** Nothing in the "Domain model", "Storage boundary",
> or "Backends" sections below is implemented. The shipped implementation is
> specified by [`PROTOTYPE_V1.md`](PROTOTYPE_V1.md), which intentionally tests
> a smaller Git-native model first. See [Implementation
> status](#implementation-status) for the exact difference before relying on
> anything here.

The first implementation is specified by [`PROTOTYPE_V1.md`](PROTOTYPE_V1.md).
It intentionally tests a smaller Git-native model before the more general
domain and storage design below is implemented.

## Implementation status

What exists today is the prototype: a review is a Git branch whose marker
commit records the review id and subject in trailers, and whose later
first-parent commits are the entries. Git supplies entry identity, ordering,
concurrency detection, history, and distribution, so there is no store layer.

Implemented:

- `ReviewProvider` / `ReviewSubject`, with a Git-revision provider.
- `start_review`, `add_note`, `load_review`, `load_review_ref`.
- Exactly two entry kinds, `Note` and `Suggestion`, discriminated by whether
  every changed path lies under `.nota/notes/`.
- Validation on load: a suggestion must have a non-empty message, must change
  at least one project file, and must not touch `.nota/`.

Not implemented — the sections below describe these, and no code provides them:

- The `ReviewStore` trait and any storage abstraction or backend selection.
- Optimistic concurrency, expected versions, and `update_entry`; entries are
  append-only Git commits and are never revised in place.
- Suggested edits as structured proposals carrying an expected original range.
  A suggestion is an ordinary Git commit; it is applied by cherry-pick, and a
  Git conflict is the only staleness signal.
- Reply, resolution-transition, and follow-up-request entry kinds, and any
  resolved/actionable state.
- The cross-backend contract test suite. The prototype has one test file
  covering the Git behaviour it actually has.

## Purpose

Nota is a standalone review application. It lets a user inspect a review
subject, leave comments, and propose edits. A review item can be resolved or
left actionable for a later worker. Nota does not run that worker or require a
particular work graph.

## Domain model

A review pins its subject and subject version. Entries are append-oriented and
have stable identities. The initial entry kinds are:

- a general or location-specific comment;
- a suggested edit containing an expected original range and replacement;
- a reply or clarification;
- a resolution transition;
- a follow-up request with enough context for another worker.

Suggested edits are proposals, not silent file mutations. Applying one checks
that the pinned content still matches. Concurrent or stale suggestions remain
visible and require reconciliation.

## Storage boundary

Nota's domain and UI depend on a storage trait, conceptually:

```rust
trait ReviewStore {
    fn create_review(&self, review: NewReview) -> Result<Review>;
    fn load_review(&self, id: &ReviewId) -> Result<Review>;
    fn append_entry(&self, id: &ReviewId, entry: NewEntry) -> Result<Entry>;
    fn update_entry(&self, change: VersionedEntryChange) -> Result<Entry>;
    fn list_reviews(&self, query: ReviewQuery) -> Result<Vec<ReviewSummary>>;
}
```

Writes use expected versions so two reviewers cannot silently overwrite each
other. Backend-specific paths, commits, and node types do not leak into Nota's
domain objects.

## Backends

The repository-file backend stores inspectable, versionable review records in
ordinary files within a configurable directory. It requires no Linka service
or library and is the baseline standalone mode.

Nota has no Linka adapter. Cross-application coordination belongs to an
orchestrator: Orka may resolve a Linka candidate to an exact Git commit, ask
Nota to review that commit, and later submit the review as a Linka verification
result. Nota sees only Git repositories, revisions, branches, and commits.

All backends run the same contract test suite, including optimistic
concurrency, stable ordering, stale suggested edits, and idempotent retries.

## Follow-up work

Nota records what needs attention; it does not dispatch an agent. A storage
adapter or external integration can expose an actionable follow-up to Orka.
The handoff pins the review, entry, subject version, and relevant suggestion so
the worker cannot accidentally address different content.

## Non-goals

- Agent/container execution (Driva).
- Scheduling follow-up workers (Orka).
- Depending on Linka or interpreting graph and candidate identities.
- Owning graph readiness or result semantics (Linka).

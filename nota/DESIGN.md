# Nota design

The first implementation is specified by [`PROTOTYPE_V1.md`](PROTOTYPE_V1.md).
It intentionally tests a smaller Git-native model before the more general
domain and storage design below is implemented.

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

The Linka adapter maps Nota review identities and follow-up requests to Linka
records or nodes while preserving Nota's versioning contract. The adapter owns
that mapping. Linka itself does not gain review-specific semantics.

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
- Requiring Linka for file-backed review.
- Owning graph readiness or result semantics (Linka).

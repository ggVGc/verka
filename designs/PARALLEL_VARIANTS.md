# Parallel variants, selection, and switching

Status: proposed feature design.

## 1. Purpose

A unit of work may have several valid implementations. llaundry should allow
multiple workers to start from the same definition and project state, preserve
all of their results, continue developing any result after another has been
chosen, and change the chosen result later without rewriting history.

The feature separates four concepts:

* an **attempt** is one execution of one node and produces at most one result;
* a **variant** is a continuing lineage of implementation results;
* a **selection** is an immutable decision preferring one candidate at a
  particular point in history;
* a **projection** is the project state currently presented as the integrated
  or main state.

"Selected" therefore does not mean that alternatives are deleted, abandoned,
or made invalid. It only records which alternative was used to construct a
particular projection.

## 2. Required properties

The implementation must provide the following guarantees:

1. Every parallel worker sees the same declared node definition, dependencies,
   context, and input project commit.
2. Workers cannot overwrite each other's result or working tree.
3. Every successful attempt has an independently addressable output commit.
4. Selecting a result does not mutate the result, its alternatives, or an
   earlier selection.
5. Work can continue from any variant, selected or not.
6. A later selection can supersede an earlier selection without erasing why
   the earlier choice was made.
7. Switching cannot silently discard downstream work or claim that work built
   against another variant is still valid.
8. The current projection and all stale or conflicting downstream work can be
   derived from recorded facts.

## 3. Graph model

The core continues to have one generic node kind. Attempt, continuation,
selection, and reconciliation are semantic roles expressed by descriptions and
ordinary graph relationships; status and execution do not branch on a stored
node type.

### 3.1 Fan-out

Suppose node `T` defines the work to perform. Parallel execution does not run
`T` several times. Instead, a fan-out operation creates child nodes `A1`, `B1`,
and so on, each derived from `T`:

```text
T: implement cache invalidation
  +-- A1: implement T, variant A
  +-- B1: implement T, variant B
```

Each child copies no prose from `T`; its description identifies it as an
attempt and its `derived_from` edge supplies the actual definition. At dispatch
time both children pin:

* the same definition and result versions of all consumed nodes;
* the same context blobs;
* the same input project commit and tree;
* the resolved backend and execution configuration.

The dispatcher creates one worktree per child, based on that exact input
commit. Completion creates a normal output commit and result for the child.
The existing one-node/one-result invariant remains intact.

Fan-out should be an explicit operation rather than a race between repeated
`work T` calls. A possible interface is:

```text
llaundry branch T --name A --name B
llaundry-work A1 &
llaundry-work B1 &
```

Names are human labels and need not be globally unique. Node IDs remain the
identity.

### 3.2 Variant lineage

An attempt is a single run; a variant can outlive that run. Continuing variant
A creates a new node derived from A's latest result:

```text
T
  +-- A1 -- A2 -- A3
  +-- B1 -- B2
```

`A2` is not a replacement for `A1`. Its result records A1's exact result and
output commit as an input. This makes the lineage queryable without a mutable
"variant head" record. The effective head of a variant is the newest
non-superseded continuation in the selected graph revision; ambiguity between
two continuations is itself another fan-out and must be resolved explicitly.

Continuing an unselected variant is identical to continuing a selected one.
Selection has no effect on readiness or permission to create descendants.

A convenience command may create the continuation node and its worktree:

```text
llaundry continue B1 --description fix-b-memory-use.md
```

The resulting node should record both the semantic derivation from `B1` and
the concrete project commit used as its base. Usually that commit is B1's
output. If the variant is being brought up to date with other work, the
operation is reconciliation instead (§7).

## 4. Selection nodes

A selection is ordinary graph work whose result is a decision. It must be a
node rather than a mutable field such as `selected = true` on a candidate.
Mutating a candidate would lose decision history, make concurrent selections
hard to merge, and confuse implementation provenance with integration policy.

A selection node depends on every candidate it claims to compare. Its
description states the decision criteria. Its result records:

* the IDs and pinned result/output versions of candidates considered;
* exactly one selected candidate;
* the rationale, preferably in `result.md`;
* any verification evidence used by the decision;
* the base projection against which the candidates were evaluated;
* whether the result can be applied directly or requires reconciliation;
* the identity of the human or machine that made the decision.

Illustrative structured data (exact TOML schema remains an implementation
choice):

```toml
outcome = "done"
decision = "select"
selected = "B2"
base_projection = "P1"
candidates = ["A3", "B2"]
```

The candidate pins already required for completed work are authoritative; the
short IDs above are navigation aids, not substitutes for pins.

Selecting B after A creates a second selection:

```text
S1: candidates A1, B1; selected A1
S2: candidates A3, B2; selected B2; derived from S1
```

S2 supersedes S1 for projections that include S2. S1 remains true as a record
of the earlier decision. "Current selection" means the latest applicable
selection in a projection, not the last file mutated globally.

## 5. The project projection

The project repository needs a concrete checked-out state, but that mutable
convenience state must not become the source of truth. A **projection node**
records construction of an integrated project commit from:

* a prior projection or initial project commit;
* the applicable selection nodes;
* any directly integrated, non-alternative work;
* reconciliation or merge results.

Its output commit is the exact project state exposed as the current integrated
branch. The workbench may maintain a convenience ref such as
`refs/llaundry/projection` pointing to that commit. The ref is a cache: it can
be reconstructed from the latest projection node and moving it does not alter
graph history.

Updating the user's conventional branch (`main`, for example) should be a
separate, explicit publish operation. Selection alone records policy; projection
materializes it; publishing moves an external Git ref. This separation makes
it possible to inspect or verify a proposed switch before changing `main`.

## 6. Switching to another variant

Switching means selecting a different lineage as the preferred implementation.
The operation has three phases.

### 6.1 Analyze

Given current projection `P1`, current selection `S1 -> A3`, and proposed
candidate `B2`, llaundry determines:

* the common input commit or merge base of A and B;
* commits introduced by the selected A lineage;
* commits introduced by the B lineage;
* nodes completed after P1 that pinned A or an A-containing projection;
* path-level overlap and whether B applies cleanly to the current projection;
* verifications whose input pins would change.

The analysis is read-only and should be shown before a new decision is
committed.

### 6.2 Decide

The user or a decision worker completes a new selection node choosing B2. This
does not yet assert that B2 can replace A3 in the current integrated tree.

### 6.3 Materialize

If no downstream work exists and B2 is based on the same projection as A, a
new projection can directly use B2.

If downstream work has accumulated on A, a raw ref move to B2 would discard or
strand that work. llaundry must instead require one of:

* **replace**: intentionally construct a projection from B2 without the
  A-dependent descendants, recording those descendants as excluded;
* **replay**: reapply compatible downstream commits onto B2, with new nodes
  recording the changed inputs;
* **reconcile**: perform explicit adaptation or merging (§7);
* **defer**: record the selection but do not create a new projection yet.

Only successful materialization creates the new projection output commit.
Selection can therefore be complete while projection is blocked on conflicts
or additional work.

## 7. Reconciliation

A reconciliation node is required when the desired variant and the current
downstream state cannot be combined mechanically or when their semantics need
review. It consumes at least:

* the newly selected variant result;
* the current projection;
* the selection decision requesting the switch.

Its description states the intended policy: preserve B's implementation while
adapting downstream features, preserve particular behavior from A, resolve a
conflict in favor of one side, and so forth. Its output is a new commit rather
than a mutation of B2. Thus B2 remains reproducible as originally produced,
while the reconciliation result is a new descendant that can become the
projection.

Git merge and rebase are implementation techniques, not provenance semantics.
Even when Git can cherry-pick or merge cleanly, llaundry should record which
node authorized that integration and what exact inputs were combined. A clean
textual merge does not prove semantic compatibility, so affected verification
must still be rerun.

## 8. Staleness and downstream impact

Changing the selected variant does not retroactively change pins. Existing
nodes continue to say truthfully that they were built and verified against A.
When evaluated relative to a projection containing B, they are reported as
inapplicable or stale because their pinned inputs are absent or have changed.

The impact query should classify descendants rather than merely print one
boolean:

* **unaffected**: no pin or file dependency crosses the changed result;
* **requires reverification**: implementation may still apply, but verification
  pins or affected files changed;
* **requires replay**: output was based on the old projection but can be applied
  cleanly to the new one;
* **requires reconciliation**: overlapping or semantic dependencies require
  new work;
* **excluded**: the switch policy intentionally leaves the node out;
* **unknown**: available provenance is insufficient to decide safely.

These classifications are derived from graph pins and Git commits. They are
not durable status flags. A projection or reconciliation result records the
chosen treatment, after which queries can derive the new state.

Verifications are never transferred from A to B merely because both satisfy
the same original task. A verification result applies only when its pinned
inputs match. The verification definition may be reused, but it must run again
against B or its reconciled projection.

## 9. Worktrees and Git commits

Every attempt and continuation runs in a dedicated worktree. Suggested
implementation sequence:

1. Resolve and record the exact input project commit before dispatch.
2. Create a temporary branch or detached worktree at that commit.
3. Run the worker with access only to that worktree and declared context.
4. On completion, reject undeclared dirty files as in the existing design.
5. Create one output commit carrying `Llaundry-Node: <id>`.
6. Record the output commit and all input pins in the node result.
7. Retain the commit even if the worktree is removed.

Commits must remain reachable. The project repository should maintain internal
refs for recorded outputs, for example `refs/llaundry/outputs/<node-id>`, or
another reachability mechanism derived from the store. These refs protect
unselected variants from garbage collection. They are indexes over result
records and can be checked or reconstructed; they are not the decision model.

The branch name used inside a temporary worktree is likewise not the variant's
identity. Branches can be renamed or deleted; node IDs, pinned commits, and
lineage edges define the variant.

## 10. Concurrency

Parallel attempts share inputs but never writable state. The dispatcher must
freeze the fan-out input commit and node versions before launching the first
worker. A definition edit after launch does not alter running workers; it makes
their completed results stale under the ordinary rules.

Two decisions may be made concurrently. They should produce two selection
nodes rather than racing to update a singleton. If neither derives from the
other, the projection has conflicting selection heads. Projection creation
must stop and require a new decision node that considers and resolves both.
This is the same explicit-merge rule used for divergent variant continuations.

## 11. Suggested commands and queries

Names are illustrative:

```text
llaundry branch <node> --name <label> [--name <label> ...]
llaundry continue <variant-head> --description <file>
llaundry candidates <node>
llaundry lineage <attempt-or-result>
llaundry select --candidate <node> [--compare <node> ...]
llaundry selection [--at <projection>]
llaundry switch --candidate <node> --analyze
llaundry switch --selection <node> --strategy replace|replay|reconcile|defer
llaundry impact <selection-or-projection>
llaundry project <selection> --verify
llaundry publish <projection> --branch main
```

`switch --analyze` and `impact` are read-only. Commands that create decisions,
work, projection commits, or move an external branch are separate so that each
state transition is inspectable and scriptable.

## 12. Example lifecycle

1. Task T is ready at project commit `C0`.
2. Fan-out creates A1 and B1. Both execute from C0.
3. A1 produces `CA`; B1 produces `CB`.
4. Verification VA passes against A1; VB passes against B1.
5. Selection S1 considers A1 and B1 and selects A1.
6. Projection P1 integrates CA and produces `CP1`; it is published to `main`.
7. Work X is completed against CP1, producing `CX`.
8. B is still developed: B2 derives from B1 and produces `CB2`.
9. Selection S2 considers A1 plus X versus B2 and selects B2, superseding S1.
10. Impact analysis finds that X was built on A and cannot be assumed valid on
    B2. S2 is complete, but no new projection exists yet.
11. Reconciliation R1 consumes B2, X, CP1, and S2. It adapts X to B2 and
    produces `CR`.
12. The relevant verification definitions run again against CR.
13. Projection P2 records CR as the integrated state and may be published.

At the end, S1, CA, CP1, and CX remain addressable. The system can explain both
why A was initially selected and why B later replaced it. Further work may
still continue from A if desired; doing so creates another lineage and does not
alter P2 until a future selection and projection choose it.

## 13. Minimal implementation stages

### Stage 1: preserve alternatives

* explicit fan-out into child nodes;
* frozen input commit and context pins;
* one isolated worktree and output commit per child;
* durable reachability refs for all output commits;
* lineage and candidate queries.

### Stage 2: decisions

* selection-node result convention and validation;
* comparison UI;
* superseding selections without mutation;
* detection of concurrent selection heads.

### Stage 3: projections and switching

* projection nodes and the reconstructible projection ref;
* switch analysis and downstream impact queries;
* replace, replay, reconcile, and defer policies;
* explicit publication to a conventional project branch.

### Stage 4: automation and review

* automatic dispatch of parallel candidates;
* verification matrices over candidates;
* review nodes attached to candidate diffs;
* assisted reconciliation and rerunning of invalidated verification.

This ordering provides useful parallel experimentation before attempting to
automate the more consequential operation of replacing an integrated lineage.

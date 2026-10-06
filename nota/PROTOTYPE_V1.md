# Nota prototype v1

## Purpose

Nota prototype v1 is a small Git-based review tool. A review is a branch that
starts at the exact commit under review. The commits added to that branch are
the review record.

Nota does not maintain a separate review database. Git supplies immutable
entry identities, ordering, concurrency detection, history, and distribution.

## Subjects

A review's subject is a Git revision in a repository, which Nota resolves to an
exact commit. Integrations resolve their own domain identities to a revision
before calling Nota; for example, Orka resolves a Linka candidate to its exact
Git artifact. Nota never interprets the external identity.

Follow-up creation is deliberately outside the prototype interface. It can be
added as a separate capability when Nota first needs to materialise a review
comment as work.

## Review representation

A review conventionally uses `nota/<review-id>` as its branch name. Callers may
supply the name; the default generates `nota/review-<ulid>`. An integration
normally passes its own identity instead — Orka uses `nota/<verification-id>` —
which Nota records and checks but never interprets. Starting a review creates
the branch without checking it out and adds one empty marker commit whose parent
is the resolved subject commit. The marker records the review id and subject in
the `Nota-Review` and `Nota-Subject` trailers.

Every later first-parent commit is one review entry:

- A prose note is an empty commit whose message is the note text followed by a
  `Nota-Note: true` trailer. Nota writes it with `commit-tree` and moves the
  branch with a compare-and-swap `update-ref`, so the branch need not be
  checked out, no working tree or index is touched, and a concurrent write to
  the branch makes the note fail rather than be lost. A note commit must not
  change files.
- A code edit or suggestion is an ordinary Git commit containing project
  changes. Its commit message is the review comment. A commit without the note
  trailer that changes no files is rejected.

Trailers are read only from a message's final paragraph, as Git does, so a
note's own text cannot be mistaken for Nota metadata.

Commit hashes are stable entry identities and first-parent history is entry
order. Published review branches are append-only: they must not be rebased,
amended, or force-pushed.

Nota does not create a worktree in this prototype. A caller may check out the
review branch normally or create a linked worktree for it.

## Applying suggestions

The review branch is a durable record and is not normally merged wholesale.
Accepted suggestion commits are cherry-picked individually in review order;
cherry-picking a range would stop at the empty note commits. Git conflicts
expose suggestions that no longer apply cleanly.

## Storage

Callers use the `ReviewStore` trait (`start_review`, `current_review`,
`add_note`, `load_review`). `GitTrailerStore` is the one implementation and
encodes reviews as described above. Every method names the review branch
explicitly; `current_review` resolves the checked-out branch for callers that
want that default.

## Commands

```text
nota start <revision> [--repository <path>] [--branch <name>]
nota note <message> [--repository <path>] [--branch <name>]
nota show [--repository <path>] [--branch <name>]
```

`start` prints the created branch, subject revision, and suggested worktree
command. `note` and `show` operate on `--branch`, or on the checked-out review
branch when it is omitted.
Reviewers record suggested edits with the ordinary `git add` and `git commit`
workflow. Nota validates those commits when it loads the review.

## Non-goals

- A review database.
- Creating or managing review worktrees.
- Dispatching follow-up work.
- Automatically merging or publishing suggestions.
- Structured reply, resolution, or approval state.
- Supporting non-Git review subjects.
- Interpreting Linka candidates or verification nodes.

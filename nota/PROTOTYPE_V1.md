# Nota prototype v1

## Purpose

Nota prototype v1 is a small Git-based review tool. A review is a branch that
starts at the exact commit under review. The commits added to that branch are
the review record.

Nota does not maintain a separate review database. Git supplies immutable
entry identities, ordering, concurrency detection, history, and distribution.

## Provider boundary

Nota resolves a provider-specific reference before starting a review:

```rust
pub trait ReviewProvider {
    fn resolve_subject(&self, reference: &str) -> Result<ReviewSubject>;
}

pub struct ReviewSubject {
    pub repository: PathBuf,
    pub revision: String,
    pub title: String,
}
```

The Git provider resolves a Git revision in a repository. Integrations resolve
their own domain identities before calling Nota; for example, Orka resolves a
Linka candidate to its exact Git artifact and then uses the ordinary Git
provider. Nota never interprets the external identity.

Follow-up creation is deliberately outside the prototype interface. It can be
added as a separate capability when Nota first needs to materialise a review
comment as work.

## Review representation

A review conventionally uses `nota/<review-id>` as its branch name. Starting a
review creates the branch without checking it out and adds one empty marker
commit whose parent is the resolved subject commit. The marker records the
review id and subject in Git trailers.

Every later first-parent commit is one review entry:

- A prose-only note adds one uniquely named Markdown file below
  `.nota/notes/`. Its commit message is the note's first non-empty line.
- A code edit or suggestion is an ordinary Git commit containing project
  changes. Its commit message is the review comment. Suggestion commits must
  not contain `.nota/` files.

Commit hashes are stable entry identities and first-parent history is entry
order. Published review branches are append-only: they must not be rebased,
amended, or force-pushed.

Nota does not create a worktree in this prototype. A caller may check out the
review branch normally or create a linked worktree for it.

## Applying suggestions

The review branch is a durable record and is not normally merged wholesale,
because it contains `.nota/notes/`. Accepted suggestion commits are
cherry-picked in review order. Git conflicts expose suggestions that no longer
apply cleanly.

## Commands

```text
nota start git <revision> [--repository <path>] [--branch <name>]
nota note <message> [--repository <path>]
nota show [--repository <path>]
```

`start` prints the created branch, subject revision, and suggested worktree
command. `note` and `show` operate on the currently checked-out review branch.
Reviewers record suggested edits with the ordinary `git add` and `git commit`
workflow. Nota validates those commits when it loads the review.

## Non-goals

- A review database or storage abstraction.
- Creating or managing review worktrees.
- Dispatching follow-up work.
- Automatically merging or publishing suggestions.
- Structured reply, resolution, or approval state.
- Supporting non-Git review subjects.
- Interpreting Linka candidates or verification nodes.

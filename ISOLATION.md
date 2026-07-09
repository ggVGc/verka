# Store isolation: the workbench layout

Status: adopted design. The mount-namespace approach it replaces is kept at
the end as an optional hardening layer.

## Problem

A node's context must be exactly what the graph says it is: description,
dependencies' results, declared pins. If a work session's file tools can read
the store directly — every node, log, and result — the connections stop
meaning anything. And the store's git history is the same database through a
side door.

Constraints discovered while designing:

- No userspace path filtering (a PreToolUse hook denying store paths): a
  blacklist with a pattern grammar can be miswritten or worked around.
- No per-session workspace copies (export the tree minus the store, grant
  the copy): projects can carry heavy data files needed for the work.
- No two-user split (store `0700`, session as a dedicated user): kernel
  enforced, but needs root setup, ACL plumbing, and separate credentials.
- No masking mounts as the *primary* mechanism: they fence a store that is
  inside the project tree, but the project's `.git` still carries store
  history and must be masked too, which walls the worker off from project
  history — legitimate context.

## Design

Every prior mechanism constructed a *view* of the project that excludes the
store. The workbench layout instead arranges reality so the store was never
inside the view:

```
workbench/               outer repo — the llaundry repo
  .git/                  store history
  .llaundry/             the store
  project/               inner repo — the actual project, ordinary in every way
    .git/                project history (legitimate context, readable)
    src/, data/, ...
```

Two completely separate git repositories. The session runs with its working
directory at `project/`, and everything it is entitled to see lives below
that directory.

**Confinement is a whitelist.** The session's tool grant is
`Read(./**), Glob(./**), Grep(./**), Edit(./**), Write(./**)` — capability
grants scoped to the working directory, no deny rules, no patterns
enumerating forbidden things. The store is not "blocked"; it is simply above
the granted subtree. A miswritten grant under-provides (the agent hits a
denial and says so) — it never leaks. Heavy data files sit in place inside
`project/`; nothing is copied anywhere.

**The git side door closes by geometry.** `project/.git` contains only
project history and is inside the granted subtree — readable, and that is
good (history is context). Store history lives in the outer `.git`, outside
the subtree, and — with the store's own history in the outer repo — one
directory level above anything the session can name.

**The project repo is completely ordinary.** No `.llaundry`, no gitignore
entry, no `llaundry:` commit noise, no awareness of being orchestrated.
Other tooling can run in `project/` and sees a plain repo. Adopting llaundry
for an existing project means moving its checkout into a workbench, not
modifying it.

**The MCP server stays a plain stdio child.** Tool permission rules
constrain the model's tool calls, not subprocess filesystem access — so the
`llaundry-mcp` child spawned by the backend reads `../.llaundry` normally
while the model's own file tools cannot. No daemon, no socket.

## Consequences for the model

- The clean-tree rule splits: the *project* repo must be clean (up to
  declared outputs) at completion time — that is where output provenance is
  enforced. The *workbench* repo is 100% machine-written and is committed by
  every mutating operation; the old "dirty work log is the one tolerated
  exception" disappears because log churn lives in the workbench repo, not
  the project repo.
- Graph edits (add, link, edit, fail) no longer require a clean project
  tree at all — jotting nodes while hacking is fine. Only `complete`
  checks the project tree, because only completion asserts output
  provenance.
- The store's history is a linear journal regardless of project branching:
  branching the project no longer forks the database, and llaundry can
  treat the project repo as a plain object of work (rebases included)
  without rewriting its own home.
- Completion is a two-repo sequence (commit outputs in the project repo,
  record the result in the store repo). Crash between the two leaves a
  committed output with no result — visible, and re-runnable; the store
  never references a commit that does not exist.

## Relating the two repos

- Store → project: precise. Results record output commit hashes and
  built-against pins, as before; these now point into the project repo.
  (Planned: record the project tree hash alongside, so history rewrites are
  detectable and relinkable, and stamp the observed project HEAD into every
  store commit as a trailer.)
- Project → store: stable keys. Output commits can carry a
  `Llaundry-Node: <id>` trailer; the id resolves through the store. A code
  checkout without the store cannot tell the story — that is the point.
- Repo pairing is positional: the store lives at `../.llaundry` relative to
  the project root. (Planned: record the project's root commit in the store
  and verify it at open, so a store can never be run against the wrong
  repository.)

## Residual risks, accepted or deferred

- The enforcement point is the harness's allow-rule scoping (`Read(./**)`
  denying paths outside the working directory, including via symlinks, and
  covering Grep/Glob). This is documented Claude Code behaviour but should
  be smoke-tested in `-p` mode; it fails closed if scoping is stricter than
  expected.
- A committed symlink inside `project/` pointing above the project root is
  the one geometric escape: the agent cannot create one (no shell), but a
  pre-existing one could exist. The permission system should deny on the
  resolved path; a driver preflight scan for out-of-tree symlinks is cheap
  belt-and-braces.

## Optional hardening: mount namespace

If kernel-grade certainty is ever wanted, the workbench layout makes it
*positive* instead of masking: launch the backend under bubblewrap with
`project/` as the only bound tree. Unprivileged (user namespaces), no rules
to miswrite, cannot be unmasked from inside (inherited mounts are
kernel-locked in nested namespaces). This composes with the layout; nothing
redesigns. Under a namespace the MCP child would be confined too, so this
variant brings back the daemon-outside-the-session requirement (localhost
HTTP MCP, or a unix socket bound into the namespace).

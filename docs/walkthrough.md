# llaundry v1 walkthrough

This file documents the happy-path flow an LLM host (Claude Code, etc.) is
expected to drive through `llaundry`'s MCP server. It is the contract the
integration test in `internal/mcp/run_tools_test.go` exercises programmatically.

## Setup

```
cd /path/to/your/project
llaundry init           # creates ./.llaundry/ with db.sqlite and nodes/ logs/
```

Register the MCP server in your host's MCP configuration, e.g.:

```json
{ "command": "llaundry", "args": ["mcp"] }
```

## Tool flow

### 1. Capture the user request
```
create_node(type="description", content={"text": "reverse a string CLI"})
  → id=D1
```

### 2. Plan tasks
```
create_node(type="task", parent=D1, content={"title":"impl reverse"})
  → id=T1
```

### 3. Implementation
```
create_node(type="implementation", depends_on=[T1], content={}) → id=I1
get_workspace_path(I1)                                            → {source_dir: ...}
# write files directly using the host's native filesystem tools
rehash(I1)                                                        → updated content_hash
```
Alternative for small files: `node_files(I1, op="write", path="...", content="...")`
(size-limited to 256 KB — beyond that use `get_workspace_path`).

### 4. Verification
```
create_node(type="verification", depends_on=[I1], content={}) → V1
run_verification(V1)                                          → {exit_code:0, status:"passed"}
```
If verification fails, spawn a new implementor that creates a successor:
```
create_node(type="implementation", depends_on=[T1], content={})  → I2
link(I2, I1, "supersedes")
```

### 5. Build
```
create_node(type="build", depends_on=[I1], content={}) → B1
run_build(B1)                                          → {exit_code:0, artifact_rel:"reverse"}
```
Default: `go build -o <build>/artifact/ ./...` run inside the implementation's
source dir. For multi-implementation builds provide `cmd` explicitly.

### 6. Staleness
Any change to an input node's content recomputes its `content_hash`; the next
`list_nodes(stale:true)` call returns every verification/build whose last run
observed a different hash.
```
update_node_content(I1, {"note":"tweaked"})
list_nodes(stale:true)                         → [V1, B1]
```

## CLI inspection

- `llaundry show <id>` — prints type, status (with STALE flag), content hash,
  content preview, edges, files, latest run, and on-disk workspace path.
- `llaundry graph [root-id]` — ASCII tree. Without an argument, prints every
  description root.

Example (after the flow above):

```
$ llaundry graph D1
D1 [description/draft] {"text":"reverse a string CLI"}
└── T1 [task/ready] {"title":"impl reverse"}
    └── I1 [implementation/ready] {"note":"tweaked"}
        ├── V1 [verification/passed] {}
        └── B1 [build/passed] {}
```

## Autonomous mode

`llaundry run` spawns an LLM agent (default `claude`) configured with ONLY
the llaundry MCP tools — no filesystem, no shell, no web. The agent reads
and writes code through `node_files`, runs verifications and builds through
`run_verification` / `run_build`, and loops until every task has a passing
verify + build. Auth is delegated to the agent binary, so whatever `claude`
uses interactively (OAuth login or `ANTHROPIC_API_KEY`) works here too.

The agent runs in five phases:

1. **Clarify** — calls `ask_user` to drill down on the brief (goal, shape,
   I/O, success criteria, non-goals, constraints).
2. **Propose + confirm** — drafts a task list, shows it to you via
   `ask_user`, and only materialises it into task nodes after you approve.
3. **Implement** — writes source via `node_files`.
4. **Verify** — `run_verification`; on failure, creates a superseding impl
   and retries up to 3 times before escalating via `ask_user`.
5. **Build** — `run_build`; same retry/escalate rules as verify.

Phases 1 and 2 are human-gated: the agent cannot create a task node until
you type `approve` (or whatever reply the agent asks for).

```
llaundry init
echo "build a CLI that reverses its argv[1]" | llaundry run
llaundry graph
```

Pin the run to an existing description instead:

```
llaundry run D1
```

Useful flags:

- `--dry-run` — print the full `claude` invocation without spawning it.
- `--agent-binary` — swap out `claude` for a compatible agent CLI.
- `--system-prompt <file>` — override the embedded workflow prompt at
  `internal/orchestrator/prompts/system.md`.
- `--max-turns` / `--timeout` — bound the run.

The raw stream-json output from the agent is tee'd to
`.llaundry/logs/orchestrator-<ts>.ndjson` for post-mortem; a condensed
tool-call log is streamed to stdout as the run progresses.

## Layout reference

```
.llaundry/
  db.sqlite            (+ -wal, -shm)
  nodes/<ulid>/
    source/            user-written source files
    build/go.work      generated for build nodes
    artifact/          output of build runs
  logs/<run_id>.stdout
  logs/<run_id>.stderr
```

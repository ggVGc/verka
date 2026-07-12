# llaundry-work design

## Responsibilities

`llaundry-work` is the execution application around a llaundry node graph. It
owns:

- Durable execution attempts and recovery.
- Isolated project worktrees.
- Agent backends and transcripts.
- Agent permissions and audit records.
- The MCP server presented to agents.
- Review, candidate, and publication workflows.

The neighboring projects have narrower responsibilities:

- `llaundry` owns the graph model, storage, graph operations, readiness,
  status, staleness, and version pins.
- `llaundry-viz` provides the human-facing graph visualization.

The MCP server is not a separate application. It is tightly coupled to an
execution attempt and is supplied by `llaundry-work`.

## Task-store boundary

`llaundry-work` defines a narrow `TaskStore` trait describing what the
orchestrator needs from a task graph. An initial `LlaundryTaskStore` adapter
implements it using the `llaundry` library.

Attempt, backend, review, and MCP code should depend on this trait rather than
on the concrete layout of `llaundry::Store`. The interface should support:

- Reading a task and its permitted dependency context.
- Finding tasks ready for a particular assignee.
- Reporting blockers and current versions.
- Freezing the inputs for a new attempt.
- Submitting a version-checked result.
- Creating and linking nodes when a grant permits it.
- Appending durable work-log and audit entries.

Beginning an attempt produces an immutable task snapshot containing at least:

- Node ID and definition version.
- Description and assignee.
- Dependency definitions, result versions, and output commits.
- Explicit context pins.
- Project input commit.

Submitting a result verifies that the definition and relevant inputs still
match the snapshot. A stale attempt must never silently complete a node.

## Attempt-scoped MCP server

Agents use the graph through an MCP server implemented and launched by
`llaundry-work`. A conceptual entry point is:

```text
llaundry-work mcp --attempt <attempt-id>
```

Before launching an agent, the harness:

1. Creates a durable attempt and isolated worktree.
2. Freezes the task inputs.
3. Creates a capability grant tied to that attempt.
4. Starts the scoped MCP server and configures the backend to use it.
5. Runs the agent.
6. Stores the transcript and MCP audit log with the attempt.
7. Finalizes or recovers the attempt using version-checked evidence.

For the normal stdio deployment, `llaundry-work` launches a private MCP
process for one agent and binds it to one immutable grant. Possession of that
private connection is the capability. The model does not need to receive or
repeat a secret in prompts or tool arguments.

If a shared, networked, or persistent MCP server is introduced, the client
authenticates once and binds its connection to a short-lived server-side
grant. Credentials must not be repeated in tool calls or stored in
transcripts.

## Capability grants

Authorization is enforced by the MCP server independently of prompts and tool
visibility. Hiding tools improves usability but is not a security boundary.

A grant is concrete and capability-oriented rather than only a broad role:

```rust
struct AgentGrant {
    attempt_id: String,
    subject: String,
    expires_at: Timestamp,
    readable_nodes: NodeScope,
    writable_nodes: NodeScope,
    can_complete_assigned_node: bool,
    can_fail_assigned_node: bool,
    can_create_children: bool,
    can_link_created_nodes: bool,
    can_edit_definitions: bool,
    allowed_output_paths: Vec<PathPattern>,
}
```

An implementation grant normally permits the agent to:

- Read its assigned node and transitive dependency context.
- Read nodes it creates during the attempt.
- Append to the assigned node's work log.
- Create scoped follow-up nodes and relationships.
- Complete or fail only its assigned node.
- Declare outputs only within its isolated workspace and allowed paths.

It normally forbids editing unrelated definitions, closing other nodes,
accepting reviews, publishing branches, and reading unrelated graph content.

Grant templates may exist for planning, implementation, verification, review,
and human administration. The resulting grant still names the exact scopes
and actions allowed for that session.

## MCP context and tools

Every MCP tool executes with context already bound to its identity and
authority:

```rust
struct McpContext {
    grant: AgentGrant,
    attempt: Attempt,
    tasks: Arc<dyn TaskStore>,
    audit: AuditWriter,
}
```

The server derives sensitive evidence from this context. It does not trust the
model to provide its attempt ID, author, backend, model, workspace, frozen
definition version, or project input commit.

An implementation-oriented tool registry may allow an agent to:

- Show the assigned node and permitted dependency context.
- Inspect permitted results and work logs.
- Append a work-log entry.
- Create a permitted follow-up node or relationship.
- Complete the assigned node with declared outputs and notes.
- Fail the assigned node with notes.

Planning and review grants expose different registries. Every tool still
performs a server-side authorization check.

Completion combines authorization with optimistic concurrency:

```rust
struct Completion {
    node_id: String,
    attempt_id: String,
    expected_definition: DefinitionVersion,
    expected_input_commit: String,
    outputs: Vec<PathBuf>,
    notes: String,
    producer: WorkEvidence,
}
```

Authorization answers whether this agent may complete this node. Version and
input checks answer whether it is still the work the agent was authorized to
complete.

## Audit and recovery

Every mutation and denied mutation is recorded with:

- Attempt and MCP session identity.
- Tool name and affected node.
- Expected and resulting versions.
- Backend and model evidence supplied by the harness.
- Timestamp, outcome, and error where applicable.

Audit records live with the durable attempt. They support diagnosis and
recovery but do not replace the graph's versioned result records.

The attempt remains the unit of recovery. Restarting an interrupted agent may
create a new scoped MCP session for the same recoverable attempt, but must not
broaden its grant or discard its frozen inputs.

## Suggested source layout

```text
src/
  task_store.rs
  adapters/llaundry.rs
  attempt.rs
  permissions.rs
  mcp/
    mod.rs
    server.rs
    registry.rs
    context.rs
    tools/
  backend/
  review/
```

Both orchestration and MCP tools use `TaskStore`. Neither reaches through the
adapter to manipulate `llaundry::Store` directly.

## Implementation order

1. Define the task snapshot, submitted-result types, and `TaskStore` trait.
2. Implement `LlaundryTaskStore` and its concurrency checks.
3. Migrate durable attempts and workspace management onto that boundary.
4. Define capability grants and authorization tests.
5. Implement the attempt-scoped stdio MCP server and audit log.
6. Integrate backend launchers with the generated MCP configuration.
7. Restore review and publication on top of durable attempt evidence.


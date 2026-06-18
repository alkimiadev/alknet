# ADR-016: Abort Cascade for Nested Calls

## Status

Accepted

## Context

The call protocol allows handlers to compose other operations through
`OperationEnv::invoke()`. This creates a call tree: a parent request spawns
children (via `parent_request_id`), which may spawn their own children. The
tree is the agency chain (ADR-015) — principal delegates to agent, agent may
delegate to sub-agent.

When `call.aborted` arrives for a parent request, the current `PendingRequestMap`
removes only that single entry. The children are unaware — they continue running,
consuming resources, and potentially producing side effects. This is the nested
abort problem:

```
Client calls /agent/chat (r1)
  agent handler calls /fs/readFile via env.invoke (r1-a)
    fs handler calls /db/query via env.invoke (r1-a-1)
  agent handler calls /bash/exec via env.invoke (r1-b)

Client aborts r1 (call.aborted { id: "r1" })
  → r1 removed from PendingRequestMap
  → r1-a, r1-a-1, r1-b continue running (ghost work)
  → bash/exec keeps executing (unwanted side effect)
  → db/query keeps running (wasted resources)
  → results produced that nobody consumes
```

The `@alkdev/flowgraph` TypeScript package solved this with a directed graph
that tracks the call tree and a `FailurePolicy` enum:

- `"abort-dependents"`: aborting a node cascades to all non-terminal descendants.
  This is the "whole tree should abort" behavior.
- `"continue-running"`: only idle/waiting dependents are aborted; started ones
  keep going. New ones don't start because their predecessors failed/aborted.

The agent use case makes this concrete and urgent: an LLM composes deep, dynamic
call trees (parallel tools, sequential tools, sub-agents calling sub-tools).
Aborting a chat should tear down the entire tree — the LLM HTTP stream, all tool
calls, all sub-calls. But this is a protocol-level concern, not an agent feature:
every consumer (NAPI adapter, Python adapter, any service speaking EventEnvelope)
inherits whatever abort model the protocol defines. The call protocol is a
general-purpose cross-boundary RPC mechanism; nested composition is a core
protocol feature, and abort semantics for that composition are protocol semantics.

## Decision

### 1. `call.aborted` cascades to descendants

When `call.aborted` arrives for a request, the protocol cascades the abort to
all non-terminal descendants in the call tree (identified via `parent_request_id`).
Each descendant receives a `call.aborted` event. The `PendingRequestMap` removes
all affected entries.

The cascade is protocol-level: the event schema carries cascade semantics. A
`call.aborted` for a parent implies abort of all descendants. This is not a
client-side convention — the server (CallAdapter) is responsible for discovering
descendants and propagating the abort.

### 2. Default policy: `abort-dependents`

The default policy is `abort-dependents`: aborting a request aborts everything
downstream, regardless of branch. This is the correct default because aborted
parent work has no consumer waiting for results — continuing is wasted work at
best and unwanted side effects at worst (e.g., a `bash/exec` that keeps running
after the caller stopped caring, a DB mutation that completes after the
transaction was aborted).

### 3. Opt-in policy: `continue-running`

An opt-in `continue-running` policy is available for cases where long-running
work should survive a parent's abort. Under `continue-running`:
- Descendants that have already started (status: running) continue to completion.
- Descendants that haven't started yet (status: pending/waiting) are aborted
  (their predecessors failed, so they can't proceed).
- No new descendants start (the parent is gone).

Use cases for `continue-running`: a long-running subscription that should keep
streaming after its parent's sibling failed; a background task that was spawned
by a handler and should survive the handler's abort.

The caller or handler specifies the policy at call time. The specific mechanism
(a field in the `call.requested` payload, a field on `OperationContext`, or a
per-operation declaration) is a two-way door for implementation.

### 4. Cleanup hooks

When a call is aborted, handlers need a mechanism to clean up resources: cancel
an HTTP stream, cancel a honker queue job, close a file handle, release a lock.
The protocol provides this through the call lifecycle — when a call is aborted,
the handler's task is cancelled (in Rust, the future is dropped). Cleanup is
handled by `Drop` implementations on resource guards, or by explicit
cancellation callbacks if the handler registers them.

This is a handler-level concern, not a protocol-level one. The protocol's job is
to cascade the abort; the handler's job is to clean up when cancelled. The
mechanism (tokio `CancellationToken`, `Drop` guards, explicit callbacks) is a
two-way door for implementation.

### 5. The call tree is tracked via `parent_request_id`

The call tree is already recorded: `OperationContext.parent_request_id` links
each call to its parent. The cascade mechanism walks this tree to find
descendants. No separate graph structure is required at the protocol level —
the `PendingRequestMap` can index entries by `parent_request_id` to enable
efficient descendant lookup.

The `@alkdev/flowgraph` package (directed graph with `descendants()`,
reactive status propagation, `FailurePolicy`) is prior art and may be adapted
as a separate Rust crate for consumers that need richer call-tree visualization
or reactive status tracking. It is not required for the protocol-level cascade
— a parent-indexed map suffices.

## Consequences

**Positive:**
- No ghost work. Aborting a parent call tears down the entire tree. Resources
  are released, side effects are halted, no results are produced for absent
  consumers.
- The default (`abort-dependents`) matches the intuitive expectation: if I
  stop caring about the parent, I stop caring about everything it spawned.
- The opt-in (`continue-running`) covers the legitimate exception (long-running
  work that should survive) without making it the default.
- The protocol carries cascade semantics, so every consumer inherits the
  correct behavior — no consumer needs to implement its own abort propagation.
- The `parent_request_id` chain already exists; the cascade mechanism is an
  index on it, not a new data structure.
- Cleanup hooks are handled by Rust's async drop semantics — dropping the
  handler's future cancels it, and `Drop` guards release resources. This is
  idiomatic Rust, not a custom mechanism.

**Negative:**
- The `PendingRequestMap` needs a parent-indexed lookup (a `HashMap<String,
  Vec<String>>` from parent_request_id to child request_ids, or a scan). This
  is a minor implementation cost, not a protocol change.
- The `call.aborted` event schema carries cascade semantics — clients that
  don't understand cascade (future versions, other implementations) would
  need to handle it. Mitigated: cascade is server-side (the CallAdapter walks
  the tree and sends `call.aborted` per descendant), so clients see individual
  abort events regardless of whether they understand the cascade concept.
- The `continue-running` policy adds a parameter to the call lifecycle. The
  specific location (payload field, context field, per-operation declaration)
  is a two-way door, but the existence of the policy is a one-way commitment.

## Assumptions

1. **Aborting a parent should abort descendants by default.** If the default
   should be `continue-running` (descendants survive), this ADR is wrong. The
   assumption is that ghost work is worse than premature cancellation — a
   cancelled descendant can be retried, but a ghost process consuming
   resources and producing unwanted side effects is harder to recover from.

2. **The server (CallAdapter) is responsible for cascade.** The client sends
   `call.aborted` for one request ID; the server discovers descendants and
   propagates. If the client were responsible for cascading, it would need to
   know the full tree — which it may not (server-side composition creates
   children the client never saw).

3. **`parent_request_id` is sufficient to discover descendants.** The call tree
   is a tree (acyclic, single parent per node). If future composition patterns
   create multi-parent relationships (e.g., a shared subcall invoked by two
   parents), the cascade model needs extension. The assumption is that
   composition creates a tree, not a DAG.

4. **Dropping the handler's future is sufficient for cleanup.** Rust's async
   drop semantics cancel the future and run `Drop` guards. If a use case
   requires explicit cleanup callbacks (e.g., external systems that need a
   signal), the mechanism needs extension. The assumption is that `Drop`
   guards cover the common cases (HTTP stream cancellation, file handle
   release, lock release).

5. **`continue-running` is per-call, not per-operation.** The policy is
   specified at call time, not declared at registration. If the policy should
   be a static property of the operation (declared in `OperationSpec`), the
   model changes. The assumption is that the caller or handler decides at call
   time based on the specific context.

## References

- ADR-012: Call protocol stream model (bidirectional streams, EventEnvelope,
  ID-based correlation)
- ADR-015: Privilege model (the call tree is the agency chain —
  `parent_request_id` traces principal → agent)
- OQ-17: Abort cascade semantics (resolved by this ADR)
- OQ-19: Session-scoped registries (session-scoped operations are in the call
  tree and participate in cascade)
- `@alkdev/flowgraph` TypeScript package — prior art for call-graph tracking
  with `descendants()`, `FailurePolicy`, reactive status propagation
- [call-protocol.md](../crates/call/call-protocol.md)
- [operation-registry.md](../crates/call/operation-registry.md)
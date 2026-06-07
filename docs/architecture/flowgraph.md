---
status: draft
last_updated: 2026-06-07
---

# FlowGraph

## What

The `alknet-flowgraph` crate provides graph data structures and operations,
mapping the TypeScript `@alkdev/flowgraph` package's call-graph and
operation-graph concepts to `petgraph::DiGraph`.

## Why

Call graphs and operation graphs are core observability and type-safety
constructs. Call graphs track request flow across services; operation graphs
validate type compatibility between composed operations. The crate is pure
computation (no I/O, no external state), making it safe to include in any
deployment topology.

## Architecture

### Core Abstraction

`petgraph::DiGraph` replaces graphology. The mapping is nearly 1:1 for the
operations used:

| TypeScript (graphology) | Rust (petgraph) |
|------------------------|-----------------|
| `graph.addNode(key, attrs)` | `graph.add_node(attrs)` + key_to_index |
| `graph.addEdge(source, target, attrs)` | `graph.add_edge(source, target, attrs)` |
| `hasCycle()` | `is_cyclic_directed(&graph)` |
| `topologicalSort()` | `toposort(&graph)` |

A `HashMap<String, NodeIndex>` provides node-key-to-index lookups, mirroring
the `key` column in the SQLite `nodes` table.

### FlowGraph<N, E>

```rust
pub struct FlowGraph<N, E>
where
    N: NodeAttributes,
    E: EdgeAttributes,
{
    graph: DiGraph<N, E>,
    key_to_index: HashMap<String, NodeIndex>,
}

pub trait NodeAttributes: Clone + Serialize + DeserializeOwned + Debug + Send + Sync {
    fn key(&self) -> &str;
    fn set_key(&mut self, key: String);
}

pub trait EdgeAttributes: Clone + Serialize + DeserializeOwned + Debug + Send + Sync {
    fn edge_type(&self) -> &str;
}
```

### Operation Graph (Static)

Built from `OperationSpec`s at startup. Answers structural questions: type
compatibility, cycle detection, reachability.

```rust
pub struct OperationNodeAttrs {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub input_schema: Value,
    pub output_schema: Value,
}

pub enum OperationType { Query, Mutation, Subscription }
```

Type compatibility compares `output_schema` (source) against `input_schema`
(target) using `jsonschema::validate()`. Exact match or subtype = compatible
edge. Structural mismatch = incompatible edge.

### Call Graph (Dynamic)

Populated at runtime from call protocol events. Every `call.requested` adds a
node; `call.responded`/`call.error`/`call.aborted` update status.

```rust
pub struct CallNodeAttrs {
    pub request_id: String,
    pub operation_id: String,
    pub status: CallStatus,
    pub parent_request_id: Option<String>,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<CallErrorInfo>,
    pub identity: Option<Identity>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

pub enum CallStatus { Pending, Running, Completed, Failed, Aborted }
```

### Key Operations

| Query | Method | Returns |
|-------|--------|---------|
| Topological order | `topological_order()` | `Result<Vec<String>, CycleError>` |
| Cycle detection | `has_cycles()` | `bool` |
| Ancestors/descendants | `ancestors()`, `descendants()` | `Vec<String>` |
| Status filtering | `filter_by_status()` | Keys with matching status |
| Duration | `duration()` | `completed_at - started_at` |

### DAG Invariants

- **Operation graph**: DAG-only enforced at construction. Cycles throw
  `CycleError`.
- **Call graph**: DAG by design. `parent_request_id` cannot create ancestor
  cycles.
- **No parallel edges**: `multi: false`.
- **No self-loops**: `allow_self_loops: false`.

### Integration with alknet-storage

Call graphs and operation graphs are stored as metagraph instances in
alknet-storage. The bridge is serialization: `FlowGraph` serializes to
`serde_json::Value`, which storage persists in the `nodes.attributes` and
`edges.attributes` columns.

### Integration with alknet-core (Call Protocol)

The call protocol's `EventEnvelope` drives call graph construction:

```rust
call_map.on_requested(|event| {
    call_graph.update_from_event(&CallEvent::Requested(event));
});
```

### Crate Dependencies

```toml
[dependencies]
petgraph = "0.x"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jsonschema = "0.x"
thiserror = "1"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.x", features = ["serde"] }
```

Does NOT depend on alknet-core, alknet-storage, or alknet-secret.

### Interface Back to Core

`OperationSpec` and `CallNodeAttrs` types must match alknet-core's definitions.
The bridge is serialization — flowgraph serializes to JSON, storage persists it.
alknet-flowgraph does not depend on alknet-core as a crate; it conforms to the
`OperationSpec` schema independently.

## Constraints

- Pure computation crate — no I/O, no database, no external state.
- No dependency on alknet-core, alknet-storage, or alknet-secret.
- Type compatibility with alknet-core's `OperationSpec` is via serialization
  conformance, not a crate dependency.

## Open Questions

- None specific to this spec. See [open-questions.md](open-questions.md) for
  general questions.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | alknet-flowgraph is independent of core, storage, secret |

## References

- [research/flow.md](../research/flow.md) — Full FlowGraph, operation graph, call graph design
- [research/integration-plan.md](../research/integration-plan.md) — Phase 2.3
- [call-protocol.md](call-protocol.md) — EventEnvelope, PendingRequestMap
- `@alkdev/flowgraph` — TypeScript call-graph and operation-graph implementation
- `@alkdev/operations` — OperationSpec, CallHandler, registry
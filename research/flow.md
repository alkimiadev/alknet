# Alknet Flowgraph: Operation Graph, Call Graph, and Graph Operations

> Status: Research / Draft
> Last updated: 2026-06-05

## Overview

`alknet-flowgraph` is a Rust crate providing graph data structures and operations, mapping the TypeScript `@alkdev/flowgraph` package's call-graph and operation-graph concepts to `petgraph::DiGraph`. It works with `alknet-storage` for persistence and `alknet-core` for call protocol event processing.

## Core Abstraction

`petgraph::DiGraph` replaces graphology. The mapping is nearly 1:1 for the operations used:

| TypeScript (graphology) | Rust (petgraph) |
|------------------------|-----------------|
| `graph.addNode(key, attrs)` | `graph.add_node(attrs)` returns `NodeIndex` |
| `graph.addEdge(source, target, attrs)` | `graph.add_edge(source, target, attrs)` returns `EdgeIndex` |
| `graph.getAttribute(key)` | `graph[node]` |
| `graph.forEachNode()` | `graph.node_indices().for_each()` |
| `graph.inNeighbors(node)` | `graph.neighbors_directed(node, Direction::Incoming)` |
| `graph.outNeighbors(node)` | `graph.neighbors_directed(node, Direction::Outgoing)` |
| `graph.hasCycle()` | `petgraph::algo::is_cyclic_directed(&graph)` |
| `graph.topologicalSort()` | `petgraph::algo::toposort(&graph)` returns `Result<Vec<NodeIndex>, Cycle>` |
| `graph.export()` | serde serialization |
| `FlowGraph.fromJSON(data)` | serde deserialization |

### Key Difference: Node Keys

graphology uses string node keys (`"call-001"`). petgraph uses `NodeIndex` (u32). We maintain a `HashMap<String, NodeIndex>` for node-key-to-index lookups, mirroring the `key` column in the `nodes` SQLite table.

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

## Operation Graph (Static)

Built from `OperationSpec`s at startup. Answers structural questions about the operation space: type compatibility, cycle detection, reachability.

### Construction

```rust
impl FlowGraph<OperationNodeAttrs, OperationEdgeAttrs> {
    pub fn from_specs(specs: &[OperationSpec]) -> Result<Self, CycleError> {
        let mut graph = Self::new();
        for spec in specs {
            graph.add_operation(spec.clone());
        }
        for (source, target) in graph.compute_type_edges(specs) {
            graph.add_typed_edge(&source, &target, TypeCompat::compatible(/*...*/))?;
        }
        if graph.has_cycles() {
            return Err(CycleError);
        }
        Ok(graph)
    }
}
```

### Type Compatibility

Compare `output_schema` (source) against `input_schema` (target) using `jsonschema`:

```rust
pub fn type_compat(
    output_schema: &Value,
    input_schema: &Value,
) -> TypeCompatResult {
    // 1. Exact match → compatible
    // 2. Subtype match (output has extra fields) → compatible
    // 3. Unknown on either side → skip (no edge)
    // 4. Structural mismatch → incompatible edge (added with compatible: false)
}
```

### Node Attributes

```rust
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OperationNodeAttrs {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum OperationType {
    Query,
    Mutation,
    Subscription,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OperationEdgeAttrs {
    pub edge_type: String,        // "typed"
    pub compatible: bool,
    pub detail: Option<String>,
}
```

### Queries

```rust
// petgraph delegations
pub fn topological_order(&self) -> Result<Vec<String>, CycleError>
pub fn has_cycles(&self) -> bool
pub fn find_cycles(&self) -> Vec<Vec<String>>
pub fn ancestors(&self, node_key: &str) -> Vec<String>
pub fn descendants(&self, node_key: &str) -> Vec<String>
pub fn predecessors(&self, node_key: &str) -> Vec<String>
pub fn successors(&self, node_key: &str) -> Vec<String>
pub fn reachable_from(&self, node_keys: &[String]) -> HashSet<String>
```

## Call Graph (Dynamic)

Populated at runtime from call protocol events. Every `call.requested` adds a node, every `call.responded`/`call.error`/`call.aborted` updates its status.

### Construction from Events

```rust
impl FlowGraph<CallNodeAttrs, CallEdgeAttrs> {
    pub fn from_call_events(events: &[CallEventMapValue]) -> Self {
        let mut graph = Self::new();
        for event in events {
            graph.update_from_event(event);
        }
        graph
    }

    pub fn update_from_event(&mut self, event: &CallEventMapValue) {
        match event {
            CallEvent::Requested(e) => {
                self.add_call(CallNodeAttrs {
                    request_id: e.request_id.clone(),
                    operation_id: e.operation_id.clone(),
                    status: CallStatus::Pending,
                    parent_request_id: e.parent_request_id.clone(),
                    input: e.input.clone(),
                    ..Default::default()
                });
            }
            CallEvent::Responded(e) => {
                self.update_status(&e.request_id, CallStatus::Completed, None);
            }
            CallEvent::Error(e) => {
                self.update_status(&e.request_id, CallStatus::Failed, Some(e.clone()));
            }
            CallEvent::Aborted(e) => {
                self.update_status(&e.request_id, CallStatus::Aborted, None);
            }
            CallEvent::Completed(e) => {
                self.update_status(&e.request_id, CallStatus::Completed, None);
            }
        }
    }
}
```

### Real-time Population

```rust
// Subscribe to call protocol events for live graph construction
let call_graph = FlowGraph::<CallNodeAttrs, CallEdgeAttrs>::new();

pubsub.subscribe("call.requested", |event| {
    call_graph.update_from_event(&event);
});
pubsub.subscribe("call.responded", |event| {
    call_graph.update_from_event(&event);
});
// ... etc for all call event types
```

### Node Attributes

```rust
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
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

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub enum CallStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CallEdgeAttrs {
    pub edge_type: EdgeType,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum EdgeType {
    Triggered,
    DependsOn,
}
```

### Status Lifecycle

```
              call.requested
                   │
                   ▼
              ┌─────────┐
              │ pending │
              └────┬────┘
                   │
              handler starts
                   │
                   ▼
              ┌─────────┐
         ┌────│ running │────┐
         │    └────┬────┘    │
    call.aborted  │    call.aborted
         │        │         │
         ▼        │         ▼
   ┌─────────┐    │   ┌─────────┐
   │ aborted │    │   │ aborted │
   └─────────┘    │   └─────────┘
                   │
         ┌─────────┼─────────┐
         │         │         │
   call.responded   │    call.error
         │         │         │
         ▼         │         ▼
   ┌───────────┐   │   ┌────────┐
   │ completed │   │   │ failed │
   └───────────┘   │   └────────┘
                   │
            call.completed
                   │
                   ▼
             ┌───────────┐
             │ completed │
             └───────────┘
```

### Abort Cascading

```rust
// Abort cascade: get all descendants of a call
let descendants = call_graph.descendants(&request_id);
// The protocol handler aborts each descendant via PendingRequestMap::abort()
```

### Observability Queries

| Query | Method | Returns |
|-------|--------|---------|
| Get running calls | `filter_by_status(CallStatus::Running)` | Node keys with running status |
| Get failed calls | `filter_by_status(CallStatus::Failed)` | Node keys with failed status |
| Get top-level calls | `get_roots()` | Nodes with no `parent_request_id` |
| Get children of call | `children(&request_id)` | Direct children via `triggered` edges |
| Get call duration | `duration(&request_id)` | `completed_at - started_at` |
| Get call lineage | `lineage(&request_id)` | Ancestor chain from root to this call |

### Serialization and Persistence

```rust
// Serialize via serde
let json = serde_json::to_value(&call_graph)?;
let restored: FlowGraph<CallNodeAttrs, CallEdgeAttrs> = serde_json::from_value(json)?;

// Persist via alknet-storage
storage.insert_call_graph("session-abc", &call_graph)?;
storage.load_call_graph("session-abc")?;
```

## Graph Operations (petgraph mapping)

All graph operations used in `@alkdev/flowgraph` map directly to petgraph:

| Flowgraph method | petgraph function |
|------------------|-------------------|
| `addNode(key, attrs)` | `add_node(attrs)` + `key_to_index.insert(key, idx)` |
| `addEdge(source, target, attrs)` | `add_edge(source_idx, target_idx, attrs)` |
| `addDirectedEdge(source, target, attrs)` | `add_edge(source_idx, target_idx, attrs)` |
| `getNodeAttributes(key)` | `graph[NodeIndex]` |
| `getEdgeAttributes(key)` | `graph[EdgeIndex]` |
| `getSource(key)` / `getTarget(key)` | `graph.edge_endpoints(EdgeIndex)` |
| `inDegree(key)` | `graph.neighbors_directed(idx, Incoming).count()` |
| `outDegree(key)` | `graph.neighbors_directed(idx, Outgoing).count()` |
| `inNeighbors(key)` | `graph.neighbors_directed(idx, Incoming)` |
| `outNeighbors(key)` | `graph.neighbors_directed(idx, Outgoing)` |
| `hasEdge(source, target)` | `graph.contains_edge(source_idx, target_idx)` |
| `forEachNode(callback)` | `graph.node_indices().for_each()` |
| `forEachEdge(callback)` | `graph.edge_indices().for_each()` |
| `findCycle()` | `is_cyclic_directed(&graph)` |
| `topologicalSort()` | `toposort(&graph, None)` |
| `export()` / `toJSON()` | `serde_json::to_value(&graph)` |
| `fromJSON()` | `serde_json::from_value(json)` |

### Cycle Detection

The operation graph rejects cycles at construction time. The call graph allows cycles in the parent-child hierarchy only via `parentRequestId` (which should not create actual cycles — a call cannot be its own ancestor).

```rust
pub fn add_call(&mut self, attrs: CallNodeAttrs) -> Result<(), CycleError> {
    if let Some(parent) = &attrs.parent_request_id {
        // Check if adding triggered edge would create a cycle
        if self.would_create_cycle(parent, &attrs.request_id) {
            return Err(CycleError);
        }
    }
    // ...
}
```

### DAG Invariants

- **Operation graph**: DAG-only enforced at construction. `add_typed_edge` throws `CycleError` if a cycle would result.
- **Call graph**: DAG-only by design (a call cannot be its own ancestor). `add_call` with a `parentRequestId` that would create a cycle throws `CycleError`.
- **No parallel edges**: `multi: false` — at most one edge per (source, target) pair.
- **No self-loops**: `allow_self_loops: false` — an operation cannot depend on its own output.

## Integration with alknet-storage

Call graphs and operation graphs are stored as metagraph instances:

```rust
// Create call-graph type definition
let call_graph_type = GraphType {
    name: "call-graph".to_string(),
    config: GraphConfig { graph_type: Directed, multi: false, allow_self_loops: false },
    scope: Scope::System,
    ..Default::default()
};

// Store a call graph instance
let graph = storage.create_graph("call-graph", "session-abc")?;

// Add call nodes
storage.add_node(graph.id, "call-001", &call_attrs)?;

// Query via petgraph
let pg: FlowGraph<CallNodeAttrs, CallEdgeAttrs> = storage.load_call_graph("session-abc")?;
let running = pg.filter_by_status(CallStatus::Running);
```

The `alknet-storage` crate handles persistence (SQLite via honker). The `alknet-flowgraph` crate handles in-memory graph operations (petgraph). The bridge is serialization: `FlowGraph` serializes to/from `serde_json::Value`, which `alknet-storage` stores in the `nodes.attributes` and `edges.attributes` columns.

## Integration with alknet-core (Call Protocol)

```rust
// The call protocol's EventEnvelope drives call graph construction
use alknet_core::call::PendingRequestMap;
use alknet_flowgraph::FlowGraph;

let mut call_graph = FlowGraph::<CallNodeAttrs, CallEdgeAttrs>::new();

// Wire up call protocol events to graph updates
call_map.on_requested(|event| {
    call_graph.update_from_event(&CallEvent::Requested(event));
});

call_map.on_responded(|event| {
    call_graph.update_from_event(&CallEvent::Responded(event));
    // Persist incrementally to storage
    storage.update_node(event.request_id, &call_graph)?;
});

call_map.on_error(|event| {
    call_graph.update_from_event(&CallEvent::Error(event));
});

call_map.on_completed(|event| {
    call_graph.update_from_event(&CallEvent::Completed(event));
});

// Abort cascading
call_map.on_aborted(|event| {
    let descendants = call_graph.descendants(&event.request_id);
    for desc in descendants {
        call_map.abort(&desc);
    }
    call_graph.update_from_event(&CallEvent::Aborted(event));
});
```

## Type Compatibility Between TS and Rust

| TypeScript (flowgraph) | Rust (alknet-flowgraph) |
|------------------------|-------------------------|
| `graphology.DirectedGraph` | `petgraph::DiGraph<N, E>` |
| `CallNodeAttrs` (TypeBox) | `CallNodeAttrs` (serde struct) |
| `CallEdgeAttrs` (TypeBox) | `CallEdgeAttrs` (serde struct) |
| `CallStatus` (enum) | `CallStatus` (Rust enum) |
| `EdgeType` (enum) | `EdgeType` (Rust enum) |
| `OperationNodeAttrs` | `OperationNodeAttrs` (serde struct) |
| `OperationEdgeAttrs` | `OperationEdgeAttrs` (serde struct) |
| `OperationType` (enum) | `OperationType` (Rust enum) |
| `Identity` | `Identity` (serde struct) |
| `AccessControl` | `AccessControl` (serde struct) |
| `typeCompat()` | `type_compat()` using jsonschema |
| `Value.Check()` (TypeBox) | `jsonschema::validate()` |
| `addCall()` | `add_call()` |
| `updateStatus()` with state machine | `update_status()` with `is_valid_transition()` |
| `addDependency()` | `add_dependency()` |
| `descendants()` | petgraph DFS |

## Crate Dependency Map

```toml
[dependencies]
petgraph = "0.x"                  # Core graph data structure
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jsonschema = "0.x"               # Type compatibility checks
thiserror = "1"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.x", features = ["serde"] }

[dev-dependencies]
tokio = { version = "1", features = ["full"] }
```

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| petgraph over custom graph | Nearly 1:1 mapping to graphology operations; well-maintained; fast |
| `HashMap<String, NodeIndex>` for key lookups | Matches SQLite `key` column pattern; O(1) lookup by string key |
| serde_json for attributes | Matches SQLite `attributes TEXT JSON` column; dynamic validation via jsonschema |
| Separate crates for flowgraph and storage | Flowgraph is pure in-memory graph ops; storage is SQLite persistence; different dependency sets |
| `NodeAttributes` / `EdgeAttributes` traits | Generic over attribute types, matching flowgraph's type parameter pattern |
| DAG enforcement at construction | Matches TypeScript flowgraph: `fromSpecs()` throws `CycleError` |
| `filter_by_status` is O(n) | Matches TypeScript: small graphs (tens to hundreds of nodes), no index needed |

## References

- `@alkdev/flowgraph` — TypeScript implementation (call-graph, operation-graph)
- `@alkdev/operations` — `OperationSpec`, `CallHandler`, `PendingRequestMap`
- `/workspace/petgraph` — Graph data structure crate
- `/workspace/jsonschema` — JSON Schema validation crate
- `/workspace/@alkdev/storage/docs/architecture/metagraph-module.md` — TypeBox Module pattern
- `/workspace/@alkdev/storage/docs/architecture/sqlite-host.md` — SQLite table definitions
- `/workspace/@alkdev/storage/docs/architecture/acl.md` — ACL as metagraph
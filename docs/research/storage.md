# Alknet Storage: Metagraph, Identity, ACL, Secrets, and Honker Integration

> Status: Research / Draft
> Last updated: 2026-06-06

## Overview

`alknet-storage` is a Rust crate providing SQLite-backed graph storage, identity management, access control, secrets management, and reactivity via honker. It mirrors the TypeScript `@alkdev/storage` package's design (`sqlite-host.md`, `metagraph-module.md`, `acl.md`) while leveraging Rust's type system and petgraph's performance.

## Terminology

This document uses **head/worker** terminology instead of hub/spoke:
- **Head node**: Coordinating node that can also be a worker
- **Worker node**: Node that connects to a head and registers services
- **Node**: Any participant in the network

## Crate Decomposition

```
alknet-storage
├── metagraph/     — GraphType, NodeType, EdgeType definitions and persistence
├── identity/      — accounts, organizations, peer_credentials, api_keys, audit_logs
├── acl/           — PrincipalNode, DelegatesEdge, access control graph
├── secrets/       — HD key derivation (BIP39/SLIP-0010), encrypted data, secret service bridge
├── honker/        — honker integration: notify, stream, queue, event bridge
├── graph/         — GraphInstance, Node, Edge CRUD with schema validation
└── schema/        — JSON Schema definitions (serde + jsonschema for runtime validation)
```

## Metagraph Data Model

The metagraph is a three-level type system (mirrors `@alkdev/storage` exactly):

1. **GraphType** — A class of graphs (e.g., "call-graph", "acl", "task-dependencies"). Defines structural constraints (directed/undirected/mixed, allows self-loops, multi-edges).
2. **NodeType** — A category of node within a graph type (e.g., "call", "account", "task"). Each node type has a JSON Schema that validates the `attributes` of nodes belonging to that type.
3. **EdgeType** — A category of edge within a graph type (e.g., "triggered", "can_read", "depends_on"). Each edge type has a JSON Schema for its attributes. Optionally constrains which source/target node types are valid.

**Graph instances** belong to a graph type and contain **Nodes** and **Edges** conforming to those type definitions.

### Rust Types

```rust
pub struct GraphType {
    pub id: String,
    pub name: String,                          // "call-graph", "acl"
    pub description: String,
    pub config: GraphConfig,                   // directed/undirected/mixed, multi, self-loops
    pub version: u32,
    pub scope: Scope,                          // System, Tenant, User
    pub metadata: serde_json::Value,
}

pub struct GraphConfig {
    pub graph_type: GraphDirection,             // Directed, Undirected, Mixed
    pub multi: bool,
    pub allow_self_loops: bool,
}

pub enum Scope {
    System,
    Tenant,
    User,
}

pub struct NodeType {
    pub id: String,
    pub graph_type_id: String,
    pub name: String,                           // "call", "account"
    pub description: String,
    pub schema: serde_json::Value,             // JSON Schema for node attributes
}

pub struct EdgeType {
    pub id: String,
    pub graph_type_id: String,
    pub name: String,                           // "triggered", "can_read"
    pub description: String,
    pub schema: serde_json::Value,             // JSON Schema for edge attributes
    pub allowed_source_types: Vec<String>,      // [] = no restriction
    pub allowed_target_types: Vec<String>,
}

pub struct Graph {
    pub id: String,
    pub graph_type_id: String,
    pub name: String,
    pub description: String,
    pub status: GraphStatus,                    // Active, Archived, Draft
    pub owner_id: Option<String>,
    pub project_id: Option<String>,
    pub metadata: serde_json::Value,
}

pub enum GraphStatus {
    Active,
    Archived,
    Draft,
}

pub struct Node {
    pub id: String,
    pub graph_id: String,
    pub key: String,                            // Consumer-defined identity within the graph
    pub attributes: serde_json::Value,          // Validated by node type schema
    pub metadata: serde_json::Value,
}

pub struct Edge {
    pub id: String,
    pub graph_id: String,
    pub key: Option<String>,                    // Null for anonymous edges
    pub source_node_key: String,
    pub target_node_key: String,
    pub attributes: serde_json::Value,          // Validated by edge type schema
    pub undirected: bool,
    pub metadata: serde_json::Value,
}
```

### SQLite Tables (mirrors `sqlite-host.md`)

Common columns on all tables: `id TEXT PK`, `metadata TEXT JSON DEFAULT '{}'`, `created_at INTEGER TIMESTAMP DEFAULT (strftime('%s','now'))`, `updated_at INTEGER TIMESTAMP DEFAULT (strftime('%s','now'))`.

**graph_types**: `id`, `name TEXT UNIQUE`, `description TEXT DEFAULT ''`, `config TEXT JSON NOT NULL`, `version INTEGER NOT NULL DEFAULT 1`, `scope TEXT NOT NULL DEFAULT 'system'`

**node_types**: `id`, `graph_type_id TEXT FK → graph_types.id CASCADE`, `name TEXT NOT NULL`, `description TEXT DEFAULT ''`, `schema TEXT JSON NOT NULL`. Unique constraint: `(graph_type_id, name)`.

**edge_types**: `id`, `graph_type_id TEXT FK → graph_types.id CASCADE`, `name TEXT NOT NULL`, `description TEXT DEFAULT ''`, `schema TEXT JSON NOT NULL`, `allowed_source_types TEXT JSON DEFAULT '[]'`, `allowed_target_types TEXT JSON DEFAULT '[]'`. Unique constraint: `(graph_type_id, name)`.

**graphs**: `id`, `graph_type_id TEXT FK → graph_types.id SET NULL`, `name TEXT NOT NULL`, `description TEXT DEFAULT ''`, `status TEXT NOT NULL DEFAULT 'draft'`, `owner_id TEXT`, `project_id TEXT`. Indexes on `(owner_id)`, `(project_id)`, `(owner_id, project_id)`.

**nodes**: `id`, `graph_id TEXT FK → graphs.id CASCADE`, `key TEXT NOT NULL`, `attributes TEXT JSON NOT NULL DEFAULT '{}'`. Unique constraint: `(graph_id, key)`. No `node_type_id` column (ADR-020).

**edges**: `id`, `graph_id TEXT FK → graphs.id CASCADE`, `key TEXT`, `source_node_key TEXT NOT NULL`, `target_node_key TEXT NOT NULL`, `attributes TEXT JSON NOT NULL DEFAULT '{}'`, `undirected INTEGER DEFAULT 0`. Unique constraint: `(graph_id, key)`. FK: `source_node_key`, `target_node_key` reference `(nodes.graph_id, nodes.key)` with CASCADE delete (ADR-022).

### System DB vs Tenant DB (ADR-040)

- **System DB** (`system.db`): Identity tables (accounts, organizations, peer_credentials, api_keys, audit_logs) + system-scoped graph types.
- **Tenant DB** (`tenant-{orgId}.db`): Metagraph tables (graph_types, node_types, edge_types, graphs, nodes, edges) + tenant-scoped graph types.

No FK constraints across database files. Consumer enforces referential integrity at application layer.

## Identity Tables

Mirrors `sqlite-host.md` identity tables with the same column definitions and FK cascades:

**accounts**: `email TEXT UNIQUE NOT NULL`, `display_name TEXT`, `access_level TEXT NOT NULL DEFAULT 'user'` (admin/user/service), `status TEXT NOT NULL DEFAULT 'active'` (active/suspended/deactivated).

**organizations**: `name TEXT UNIQUE NOT NULL`, `slug TEXT UNIQUE NOT NULL`, `owner_id TEXT FK → accounts.id RESTRICT`.

**organization_members**: `org_id TEXT FK → organizations.id CASCADE`, `account_id TEXT FK → accounts.id CASCADE`, `membership_level TEXT NOT NULL` (owner/admin/member). Unique constraint: `(org_id, account_id)`.

**api_keys**: `owner_id TEXT FK → accounts.id CASCADE`, `key_hash TEXT UNIQUE NOT NULL`, `name TEXT`, `enabled INTEGER NOT NULL DEFAULT 1`, `expires_at INTEGER TIMESTAMP`, `revoked_at INTEGER TIMESTAMP`, `rotated_to_id TEXT`, `last_used_at INTEGER TIMESTAMP`.

**peer_credentials**: `owner_id TEXT FK → accounts.id CASCADE`, `credential_type TEXT NOT NULL` (ssh_key/cert_authority), `fingerprint TEXT UNIQUE NOT NULL`, `public_key_data TEXT NOT NULL`, `name TEXT`, `enabled INTEGER NOT NULL DEFAULT 1`, `expires_at INTEGER TIMESTAMP`, `revoked_at INTEGER TIMESTAMP`.

**audit_logs**: `action TEXT NOT NULL`, `owner_id TEXT FK → accounts.id RESTRICT`, `credential_id TEXT`, `credential_type TEXT`, `org_id TEXT FK → organizations.id SET NULL`, `details TEXT JSON`.

## Access Control (ACL) as Metagraph

Mirrors `@alkdev/storage acl.md`:

### AclGraph Module

```rust
// Graph config: directed, multi=false, allowSelfLoops=false
pub const ACL_GRAPH_CONFIG: GraphConfig = GraphConfig {
    graph_type: GraphDirection::Directed,
    multi: false,
    allow_self_loops: false,
};

// Node types
pub const PRINCIPAL_NODE: &str = "principal";
pub const RESOURCE_NODE: &str = "resource";

// Edge types
pub const CAN_READ_EDGE: &str = "can_read";
pub const CAN_WRITE_EDGE: &str = "can_write";
pub const CAN_EXECUTE_EDGE: &str = "can_execute";
pub const BELONGS_TO_EDGE: &str = "belongs_to";
pub const DELEGATES_EDGE: &str = "delegates";

// PrincipalNode attributes
pub struct PrincipalNodeAttrs {
    pub identity_type: IdentityType,    // Account, Org, Service, Role
    pub identity_id: String,            // FK to accounts.id or organizations.id
    pub scopes: Vec<String>,
    pub resources: Option<HashMap<String, Vec<String>>>,
}

pub enum IdentityType {
    Account,
    Org,
    Service,
    Role,
}

// DelegatesEdge attributes
pub struct DelegatesEdgeAttrs {
    pub narrowed_scopes: Vec<String>,     // Subset of delegator's scopes
    pub narrowable: bool,                  // Can the delegate further narrow?
}
```

### Principal-Agent Hierarchy

- **Account** nodes represent individual users
- **Org** nodes represent organizations
- **Service** nodes represent automated agents (LLM workers, node credentials)
- **Role** nodes represent named permission sets

Delegation edges (`delegates`) carry `narrowed_scopes` — the delegate can only exercise scopes that are a subset of the delegator's. Liability flows upward; permissions flow downward with narrowing.

### BelongsToEdge (Derived from org_members)

ADR-045: The `organization_members` SQL table is the authoritative source. When membership changes, the consumer writes the SQL row first, then creates or removes the ACL `belongs_to` edge. The edge is derived, not the source of truth.

### Operation-Level ACL

`OperationSpec.access_control` maps to ACL graph traversal at runtime:

```rust
pub fn check_access(
    acl_graph: &Graph,
    principal_key: &str,
    operation_spec: &OperationSpec,
) -> bool {
    // Traverse from PrincipalNode to ResourceNode
    // Check if any path satisfies required_scopes (AND) and required_scopes_any (OR)
    // Honor delegation chains with scope narrowing
}
```

## Honker Integration

### Reactivity Pattern (ADR-047)

Every mutation is atomic with a notification:

```rust
// Insert a node and notify in one transaction
tx.execute(
    "INSERT INTO nodes (id, graph_id, key, attributes) VALUES (?, ?, ?, ?)",
    &[&node_id, &graph_id, &key, &attrs_json],
)?;
tx.stream_publish("nodes:created", &node_attrs_json)?;
```

This mirrors the TypeScript pattern from `sqlite-host.md` but in Rust, using honker's SQLite extension functions:

```rust
use honker::Database;

let db = Database::open("tenant.db")?;

// Transactional: business write + event stream publish commit together
let mut tx = db.transaction()?;
tx.execute("INSERT INTO nodes (id, graph_id, key, attributes) VALUES (?, ?, ?, ?)", ...)?;
tx.stream_publish("nodes:created", &attrs)?;
tx.commit()?;

// Subscribe to changes
let stream = db.stream("nodes:created");
async for event in stream.subscribe("alknet-node-watcher") {
    // event is a serde_json::Value
}
```

### Honker Features Used

| Feature | Use case |
|---------|----------|
| `stream_publish` / `subscribe` | Durable pub/sub for node/edge/membership changes with per-consumer offsets |
| `notify` / `listen` | Ephemeral pub/sub for real-time control channel events |
| `queue` / `claim` / `ack` | Task queue for async operations (key rotation, ACL evaluation) |
| `scheduler` | Periodic tasks (session cleanup, audit log pruning) |

### Database Concurrency

- WAL mode (default) for concurrent reads during writes
- Single writer per `.db` file
- `busy_timeout=5000` default
- `PRAGMA data_version` polling for cross-process wake (honker pattern)
- `max_readers=4` concurrent read connections in the reader pool

## JSON Schema Validation

TypeBox from TypeScript maps to `serde_json::Value` + `jsonschema` in Rust:

| TypeScript | Rust |
|-----------|------|
| `Type.Object({...})` | `serde_json::json!({...})` as JSON Schema |
| `Value.Check(schema, data)` | `jsonschema::validate(&schema, &data)` |
| `Type.Module({...})` | JSON Schema with `$defs` stored in DB |
| `Type.Composite([A, B])` | Merge + intersect via `serde_json` merge logic |

The `jsonschema` crate provides runtime validation analogous to TypeBox's `Value.Check()`. Schema definitions are stored as `serde_json::Value` in the `schema` column of `node_types` and `edge_types` tables.

## Crate Dependency Map

```toml
[dependencies]
honker = "0.x"                    # SQLite extension with pub/sub/queue
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jsonschema = "0.x"                # JSON Schema validation (runtime)
petgraph = "0.x"                  # Graph data structure (shared with alknet-flowgraph)
rusqlite = { version = "0.x", features = ["bundled"] }  # SQLite access (via honker)
uuid = { version = "1", features = ["v4"] }
chrono = "0.x"
thiserror = "1"
tokio = { version = "1", features = ["full"] }
```

## Multi-Tenant Replication Path

For the private use case: single `.db` files, honker for reactivity, no cross-database FK constraints.

For the distributed use case (later):

1. **Smart contracts** (Base L2) own namespace identity → `ownerId` field on `graphs` table
2. **alknet-relay** gossips namespace availability via iroh-gossip or call protocol subscriptions
3. **ACL inference** — Contract `collaborators` → ACL graph `DelegatesEdge` entries
4. **Honker streams** — `stream_subscribe("nodes:modified")` carries mutations to relay subscribers

Replication mindset from the start: **every write is atomic with a notification**. The honker stream event is the replication unit. A future replicator reads `_honker_stream_*` tables and propagates changes to subscribed relays.

### Event Boundary Discipline

Following [event_source_types.md](/workspace/research/event_sourcing/event_source_types.md), honker streams serve different roles in different contexts. Preventing conflation is critical:

| Event Type | Source | Consumer | Boundary |
|-----------|--------|----------|----------|
| **Domain events** (Event Sourcing) | Service that owns the data | Same service, for state reconstruction | Internal — never published directly to other services |
| **Integration events** (State Transfer) | Projected from domain events | Other services/nodes, for cache updates | Cross-service — simple, versioned, stripped of internals |
| **Notifications** (Thin Events) | Service that owns the data | Any subscriber, for triggering workflows | Cross-node — just entity ID + action, consumer fetches details |

Conflation anti-patterns to avoid:
- **Leaky event store**: Don't let other services read honker stream events directly to drive business logic. Project domain events into integration events first.
- **Boomerang coupling**: If a consumer of an integration event must call back to the source service synchronously, the event payload is too thin. Upgrade to a fat event.
- **Fat notification trap**: If a notification event carries the full entity state, use state transfer instead.

The call protocol's `EventEnvelope` is the **integration boundary** between nodes. Domain events in honker streams stay within the service that owns them.

## Secrets and HD Key Derivation

### Key Categories

Different categories of secrets require different storage and derivation strategies:

| Category | Example | Derived from seed? | Storage |
|-----------|---------|-------------------|---------|
| **Identity keys** | Ed25519 keypair for alknet auth | Yes — SLIP-0010 `m/74'/0'/0'/0'` | Only derivation path in DB |
| **Encryption keys** | AES-256-GCM key for encrypted nodes | Yes — SLIP-0010 `m/74'/2'/0'/0'` | Only derivation path in DB |
| **External credentials** | OpenAI API key, OAuth token | No — third-party issued | Encrypted in DB with derived key |
| **On-chain identity** | Ethereum key for contract signing | Yes — SLIP-0010 `m/44'/60'/0'/0/0` | Only derivation path in DB |
| **Service registration** | NFT token ID, replicator endpoint | No — on-chain data | Plain in DB or on-chain |

### BIP39 Seed Phrase as Root of Trust

The master seed phrase (BIP39 mnemonic) is the single recovery mechanism for the entire system. From one seed phrase, all self-generated secrets can be derived on demand:

```rust
// Seed phrase → master seed (BIP39)
let mnemonic = Mnemonic::from_phrase(&phrase, Language::English)?;
let seed = mnemonic.to_seed(Some(&passphrase));

// Master seed → SLIP-0010 Ed25519 master key
let master_key = ExtendedPrivKey::new_master(Network::Alknet, &seed)?;

// Derive identity keypair
let identity_key = master_key.derive_path("m/74'/0'/0'/0'")?;

// Derive encryption key material (use first 32 bytes of derived key as AES-256 key)
let encryption_key = master_key.derive_path("m/74'/2'/0'/0'")?;

// Derive Ethereum signing key (for smart contract interactions)
let eth_key = master_key.derive_path("m/44'/60'/0'/0/0")?;
```

### External Credentials: Encryption with Derived Keys

For external credentials (API keys, OAuth tokens) that can't be derived, the existing `EncryptedDataSchema` pattern from `@alkdev/storage` applies — but the encryption key is itself derived from the seed:

1. The secret service derives an AES-256-GCM key via SLIP-0010 path `m/74'/2'/0'/0'`
2. External credentials are encrypted with this derived key using the existing encrypt/decrypt functions
3. The encrypted data is stored as a `SecretNode` in the metagraph
4. Only the derivation path and key version are stored in plain attributes
5. The seed phrase (or derived encryption key) is held only by the secret service — never in the database

### Secret Service

The secret service is an irpc service (see [services.md](services.md)) that:

- Holds the master seed phrase in memory (never persisted to disk in plain text)
- Derives keys on demand via SLIP-0010/BIP39
- Encrypts/decrypts external credentials using derived keys
- Is the **only** component that ever sees the master seed

Workers request derived keys through the secret service's irpc protocol. They never see the seed or the encryption key.

### Derivation Path Conventions

| Path | Purpose |
|------|---------|
| `m/74'/0'/0'/0'` | Primary Ed25519 identity keypair (alknet auth) |
| `m/74'/0'/0'/1'` | Secondary identity keypair (device key) |
| `m/74'/0'/1'/0'` | SSH host key (for server identity) |
| `m/74'/1'/0'/{site_hash}'` | Site-specific password derivation |
| `m/74'/2'/0'/0'` | AES-256-GCM encryption key (for external credentials) |
| `m/44'/60'/0'/0/0` | Ethereum signing key (for smart contract interactions) |

The `74'` coin type is unallocated per SLIP-0044 and can be registered for alknet. The `0'`/`1'`/`2'` account levels divide identity, password, and encryption purposes.

### Rust Crates Required

| Crate | Purpose |
|-------|---------|
| `bip39` | Mnemonic generation and seed derivation |
| `ed25519-bip32` (IOHK) or `rust-bip32-ed25519` (BitBoxSwiss) | SLIP-0010 Ed25519 HD key derivation |
| `aes-gcm` | AES-256-GCM encryption for external credentials |
| `sha2` | SHA-256 for key hashing |
| `irpc` | Service protocol definitions |

## Design Decisions (mapped from TypeScript ADRs)

| Original ADR | Decision | Rust adaptation |
|-------------|----------|-----------------|
| 002 | Metagraph over domain tables | Same 6-table schema, same graph type/node type/edge type model |
| 008 | Common columns pattern | `id`, `metadata`, `created_at`, `updated_at` on all tables |
| 019 | JSON text for schema columns | `serde_json::Value` stored as TEXT in SQLite |
| 020 | No nodeTypeId on nodes | Node type enforced at application layer |
| 022 | Composite FKs for node refs | `source_node_key` + `target_node_key` with cascade |
| 034 | ACL as metagraph | AclGraph is a metagraph instance |
| 038 | SQLite-first, PG removed | SQLite only via honker |
| 040 | System DB + tenant DB | Two `.db` files |
| 041 | Identity tables in storage | Same tables, same constraints |
| 045 | org_members authoritative | SQL table is source of truth, BelongsToEdge is derived |
| 047 | Honker event target | honker stream/notify as pub/sub mechanism |
| 049 | Identity schema restructuring | Separate credential tables, no Gitea columns |
| 050 | SHA-256 for API key hashing | Fast hash for high-entropy machine keys |
| 051 | BIP39/SLIP-0010 for HD key derivation | Seed phrase as root of trust for identity, encryption, and signing keys |
| 052 | Secrets as irpc service | Secret service holds seed, derives keys, encrypts/decrypts external creds |
| 053 | Event boundary discipline | Honker streams are domain events; call protocol is integration boundary |

## References

- `@alkdev/storage` — TypeScript metagraph, identity, ACL, encrypted data implementation
- `@alkdev/flowgraph` — TypeScript call-graph and operation-graph (maps to petgraph in Rust)
- `@alkdev/operations` — TypeScript OperationSpec, CallHandler, registry
- `/workspace/honker` — SQLite extension with pub/sub, streams, queues
- `/workspace/polyglot` — SQL transpiler (future: schema migration validation)
- `/workspace/petgraph` — Graph data structure library (used in alknet-flowgraph)
- `/workspace/jsonschema` — JSON Schema validation (Rust, replaces TypeBox at runtime)
- `/workspace/iroh/iroh-dns` — DNS resolver and endpoint info
- `/workspace/@alkdev/storage/docs/architecture/encrypted-data.md` — Original encrypted data design (TypeScript)
- `/workspace/research/event_sourcing/event_source_types.md` — Event-driven architecture patterns
- [services.md](services.md) — Service layer architecture (irpc protocols)
- [core.md](core.md) — Core overview, head/worker terminology
---
status: draft
last_updated: 2026-06-07
---

# Storage

> **Phase note**: `alknet-storage` is a future crate (Phase 2+). This spec
> defines its contract — the data model, the `IdentityProvider` impl, the
> irpc service protocol — so that alknet-core can define the traits
> (`IdentityProvider`) that storage will later implement. The crate itself
> hasn't been built yet. Phase 1 uses `ConfigIdentityProvider` backed by
> `ArcSwap<DynamicConfig>`.

## What

The `alknet-storage` crate will provide SQLite-backed graph storage, identity
management, access control, and reactivity via honker. It mirrors the
TypeScript `@alkdev/storage` package's design while leveraging Rust's type
system and honker's built-in pub/sub.

## Why

alknet-core needs persistent identity data (authorized keys, accounts, ACLs)
and a way to store and query graph-structured data (call graphs, operation
graphs, metagraph). But alknet-core cannot take a database dependency. The
solution: alknet-storage implements alknet-core's `IdentityProvider` trait,
providing SQLite-backed identity resolution without core knowing about SQLite.

The metagraph (three-level type system: GraphType → NodeType → EdgeType → Graph
→ Node → Edge) is the foundation for ACL, flowgraph persistence, and any
future graph-structured data.

## Architecture

### Crate Structure

```
alknet-storage/
├── metagraph/     — GraphType, NodeType, EdgeType persistence
├── identity/      — accounts, organizations, peer_credentials, api_keys, audit_logs
├── acl/           — PrincipalNode, DelegatesEdge, access control graph
├── secrets/       — Encrypted node type, encrypt/decrypt bridge
├── honker/        — honker integration: notify, stream, queue
├── graph/         — GraphInstance, Node, Edge CRUD with schema validation
└── schema/        — JSON Schema definitions (serde + jsonschema)
```

### Metagraph Data Model

Three-level type system:

1. **GraphType** — A class of graphs (e.g., "call-graph", "acl",
   "task-dependencies"). Defines structural constraints.
2. **NodeType** — A category of node within a graph type. Each has a JSON Schema
   for attribute validation.
3. **EdgeType** — A category of edge within a graph type. Each has a JSON Schema
   and optional source/target constraints.

Graph instances belong to a graph type and contain nodes and edges conforming
to those type definitions.

### SQLite Table Schema

Common columns: `id TEXT PK`, `metadata TEXT JSON DEFAULT '{}'`,
`created_at INTEGER TIMESTAMP`, `updated_at INTEGER TIMESTAMP`.

| Table | Key columns |
|-------|------------|
| `graph_types` | id, name (UNIQUE), config JSON, version, scope |
| `node_types` | id, graph_type_id FK, name, schema JSON |
| `edge_types` | id, graph_type_id FK, name, schema JSON, allowed_source/target types |
| `graphs` | id, graph_type_id FK, name, description, status, owner_id, project_id |
| `nodes` | id, graph_id FK, key (UNIQUE per graph), attributes JSON |
| `edges` | id, graph_id FK, key, source_node_key, target_node_key, attributes JSON, undirected |

No FK constraints across database files. Referential integrity is enforced at
the application layer.

### System DB vs Tenant DB

- **System DB** (`system.db`): Identity tables (accounts, organizations,
  peer_credentials, api_keys, audit_logs) + system-scoped graph types.
- **Tenant DB** (`tenant-{orgId}.db`): Metagraph tables + tenant-scoped graph
  types.

### Identity Tables

| Table | Key columns |
|-------|------------|
| `accounts` | email (UNIQUE), display_name, access_level (admin/user/service), status |
| `organizations` | name (UNIQUE), slug (UNIQUE), owner_id FK → accounts |
| `organization_members` | org_id FK, account_id FK, membership_level (owner/admin/member) |
| `api_keys` | owner_id FK, key_hash (UNIQUE), name, enabled, expires_at, revoked_at |
| `peer_credentials` | owner_id FK, credential_type (ssh_key/cert_authority), fingerprint (UNIQUE), public_key_data |
| `audit_logs` | action, owner_id FK, credential_id, org_id FK, details JSON |

### ACL as Metagraph

The ACL graph is a directed, non-multi metagraph:

- **PrincipalNode**: IdentityType (Account, Org, Service, Role) + identity_id + scopes + resources
- **ResourceNode**: The thing being accessed
- **Edges**: can_read, can_write, can_execute, belongs_to, delegates

Delegation edges carry `narrowed_scopes` — the delegate can only exercise scopes
that are a subset of the delegator's.

### StorageIdentityProvider (Future — Phase 2+)

Implements alknet-core's `IdentityProvider` trait (ADR-029). This is defined
here as a contract. When alknet-storage is built, it will provide this
implementation. Phase 1 uses `ConfigIdentityProvider` backed by `ArcSwap`.

```rust
impl IdentityProvider for StorageIdentityProvider {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        // 1. Find peer_credentials row by fingerprint
        // 2. Resolve to account → organization membership → effective scopes
        // 3. Return Identity { id: account_uuid, scopes, resources }
    }

    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
        // 1. Verify Ed25519 signature against api_keys or peer_credentials
        // 2. Resolve to account → effective scopes
        // 3. Return Identity { id: account_uuid, scopes, resources }
    }
}
```

### StorageProtocol irpc Service

```rust
#[rpc_requests(message = StorageMessage)]
enum StorageProtocol {
    #[rpc(tx=oneshot::Sender<Graph>)]
    #[wrap(CreateGraph)]
    CreateGraph { graph_type_id: String, name: String },

    #[rpc(tx=oneshot::Sender<Node>)]
    #[wrap(AddNode)]
    AddNode { graph_id: String, key: String, attributes: Value },

    // ... (full protocol in research/services.md)
}
```

### Honker Integration

| Feature | Use case |
|---------|----------|
| `stream_publish` / `subscribe` | Durable pub/sub for node/edge/membership changes |
| `notify` / `listen` | Ephemeral pub/sub for real-time control channel events |
| `queue` / `claim` / `ack` | Task queue for async operations |

Per ADR-032, honker streams are domain events internal to the storage service.
They are projected to call protocol `EventEnvelope` events when crossing service
boundaries.

### Encrypted Data

alknet-storage references alknet-secret's `EncryptedData` wire format for
storing encrypted nodes (API keys, OAuth tokens). The format (key_version,
salt, iv, ciphertext) is shared by type-level compatibility, not a crate
dependency. alknet-secret encrypts; alknet-storage stores the blob.

### Crate Dependencies

```toml
[dependencies]
honker = "0.x"
rusqlite = { version = "0.x", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jsonschema = "0.x"
petgraph = "0.x"
irpc = "0.x"
```

Does NOT depend on alknet-core or alknet-secret. Implements alknet-core's
`IdentityProvider` trait by conforming to its signature, not by direct crate
dependency.

## Constraints

- alknet-storage does NOT depend on alknet-core as a crate. It implements the
  `IdentityProvider` trait by conforming to the signature. The CLI binary
  wires them together.
- alknet-storage does NOT depend on alknet-secret. They share the `EncryptedData`
  wire format by type-level compatibility, not a crate dependency.
- WAL mode for concurrent reads during writes. Single writer per `.db` file.
- JSON Schema validation uses the `jsonschema` crate at runtime (replaces
  TypeBox from TypeScript).
- Per ADR-032, honker stream events never cross service boundaries without
  projection to `EventEnvelope`.

## Open Questions

- **OQ-SVC-03**: How does the secret service integrate with the existing
  `EncryptedDataSchema` from `@alkdev/storage`? See [open-questions.md](open-questions.md).

- **OQ-SVC-04**: Should workers cache derived keys locally? See [open-questions.md](open-questions.md).

- **OQ-SVC-05**: How does the NFT-based ACL smart contract interact with the
  secret service? See [open-questions.md](open-questions.md).

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | alknet-storage is independent of core and secret |
| [029](decisions/029-identity-core-type.md) | Identity as core type | alknet-storage implements IdentityProvider trait |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Honker streams stay internal; projection to EventEnvelope at boundaries |

## References

- [research/storage.md](../research/storage.md) — Full metagraph, identity, ACL, honker definitions
- [research/services.md](../research/services.md) — StorageProtocol, StorageIdentityProvider
- [research/integration-plan.md](../research/integration-plan.md) — Phase 2.2
- [identity.md](identity.md) — IdentityProvider trait, Identity struct
- [secret-service.md](secret-service.md) — EncryptedData format, derivation paths
---
status: draft
last_updated: 2026-06-07
---

# Services

## What

The irpc service layer decomposes alknet's core responsibilities into
independently testable, deployable, and replaceable components. Auth, Secret,
Config, and Storage are irpc protocol enums that work both as in-process async
boundaries (tokio channels) and cross-process/cross-network (QUIC streams via
noq). OperationEnv is the universal composition mechanism that unifies local
dispatch, irpc service dispatch, and remote call protocol dispatch.

## Why

Without the service layer, auth verification, key derivation, and config reload
are scattered across the codebase with no async boundary. For head nodes serving
many users, in-memory key lookup doesn't scale — auth needs to query a database
on demand. For secret management, the seed must be isolated in its own process
boundary.

Without OperationEnv, handlers calling other operations would need to know
whether the target is local, in-cluster, or on a remote node. OperationEnv
abstracts this away: `context.env.invoke("secrets", "derive", input)` works
regardless of dispatch path.

## Architecture

### Service Definition Pattern

Services are defined as irpc protocol enums:

```rust
#[rpc_requests(message = AuthMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum AuthProtocol {
    #[rpc(tx=oneshot::Sender<AuthResult>)]
    #[wrap(VerifyPubkey)]
    VerifyPubkey { fingerprint: String, key_data: Vec<u8> },
    // ...
}
```

The `#[rpc_requests]` macro generates two versions:
- **Serializable** (`Request`): for remote communication (postcard encoding)
- **With channels** (`RequestWithChannels`): for local communication (tokio channels)

Both use the same `Client<S>` type. The local/remote distinction is transparent
at the call site.

### Core Services

| Service | Protocol | Purpose | Always Local? |
|---------|----------|---------|---------------|
| **Auth** | `AuthProtocol` | Verify identities, check credentials | Can be remote |
| **Secret** | `SecretProtocol` | Derive keys, encrypt/decrypt | Local or remote |
| **Config** | `ConfigProtocol` | Dynamic config reload | Local |
| **Storage** | `StorageProtocol` | Graph CRUD, metagraph operations | Local or remote |

### OperationContext

Every handler receives an `OperationContext`:

```rust
pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,
    pub metadata: HashMap<String, Value>,
    pub env: OperationEnv,
    pub trusted: bool,  // set by buildEnv(), not by callers
}
```

- **`identity`**: The authenticated identity making the call. Populated by
  `IdentityProvider` from the interface layer.
- **`env`**: The operation environment — namespaced access to other operations.
- **`trusted`**: When a handler calls another operation through `env`, the
  nested call is `trusted` (skips ACL checks).

### OperationEnv — Universal Composition Mechanism

OperationEnv provides namespace + operation name → invoke with input, return
output. The handler doesn't know or care whether the dispatch is local, irpc,
or remote.

Three dispatch paths:

| Path | Mechanism | Serialization | Scope |
|------|-----------|---------------|-------|
| **Local** | Direct function call through registry | None (in-process) | Same process |
| **Service** | irpc protocol enum dispatch | postcard (binary) | Same cluster |
| **Remote** | Call protocol `EventEnvelope` | JSON | Cross-node |

All three produce the same `ResponseEnvelope`.

Service assembly determines which path each operation uses:

```rust
// Minimal deployment (single node, all local)
let env = OperationEnv::local(local_registry);

// Production deployment (mix of local and remote)
let env = OperationEnv::new()
    .local("auth", auth_registry)
    .local("config", config_registry)
    .service("secrets", secret_irpc_client)
    .remote("worker-1", call_protocol_conn);
```

### Service vs Call Protocol vs External Service

These are different concepts that compose through OperationEnv:

- **irpc service**: In-cluster, Rust-to-Rust, type-safe, postcard serialization.
  Dispatched by enum variant. Example: `AuthProtocol::VerifyPubkey`.
- **Call protocol operation**: Cross-node, cross-language, path-based, JSON
  `EventEnvelope`. Dispatched by namespace + name. Example:
  `/head/auth/verify`.
- **External service**: Any endpoint reachable via the call protocol.
  Example: a vast.ai instance, an HTTP API, another head node.

An irpc service can back a call protocol operation. The OperationEnv routes to
the appropriate dispatch path:

```
Call Protocol (Layer 3, external, JSON)
    └── irpc Service (Layer 3, internal, postcard)
            └── Honker Streams (Domain events, within service boundary)
```

### Adapters

HTTP, MCP, DNS, and WebSocket adapters all resolve through OperationEnv:

- HTTP: `POST /v1/{namespace}/{op}` → `context.env.invoke(namespace, op, input)`
- MCP: `tools/call` with tool name → `context.env.invoke(namespace, op, input)`
- DNS: `{op}.{namespace}.alk.dev TXT?` → `context.env.invoke(namespace, op, input)`
- Call protocol: `call.requested` with `operationId` → `context.env.invoke(namespace, op, input)`

### Deployment Topologies

**Minimal (single node, CLI)**: All services run locally via tokio channels.

```
┌──────────────────────────────────────────────┐
│                 Single Process                │
│  Auth (ArcSwap) | Secret (seed in RAM) |     │
│  Config (ArcSwap) | alknet-core Server        │
└──────────────────────────────────────────────┘
```

**Production (multi-node)**: Auth and secrets on dedicated nodes; workers
access them remotely.

```
Auth Node (SQLite)           Secret Node (seed in RAM)
       ↑                              ↑
       │ QUIC (irpc)                  │ QUIC (irpc)
       │                              │
Head Node (Config, Storage, alknet-core Server)
       │
       │ SSH / iroh / TLS
       │
Worker Node (alknet-core Client)
```

## Constraints

- Services are **internal** — they run within a node or cluster.
- The call protocol is **external** — it's how nodes talk to each other.
- Per ADR-032, domain events (Honker streams) stay within the owning service.
  irpc calls are synchronous request-response within a node. Call protocol
  `EventEnvelope` is the integration boundary between nodes.
- OperationEnv is a hard constraint: the handler-facing API must match the
  behavioral contract from `@alkdev/operations`. Namespace + operation name →
  invoke with input, return output.
- irpc is behind a feature flag in alknet-core. Nodes that only do SSH tunneling
  don't need the service layer overhead.

## Open Questions

- **OQ-SVC-01**: Should the secret service support multiple seed phrases (one
  per tenant)? Defer for now — one seed per node. Multi-seed can be added
  later by indexing the `Unlock` call with a tenant ID.

- **OQ-SVC-02**: Should service protocols use postcard (binary) or JSON for
  remote calls? Postcard for irpc (Rust-to-Rust, efficient). JSON for call
  protocol (cross-language, universal). The irpc remote path naturally uses
  postcard.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | Service crates are independent of core |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service | AuthProtocol behind feature flag |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Domain events never cross service boundaries |
| [033](decisions/033-operationenv-irpc-call-protocol.md) | OperationEnv | Universal composition mechanism with three dispatch paths |

## References

- [research/services.md](../research/services.md) — Service protocol definitions, OperationContext, deployment topologies
- [research/integration-plan.md](../research/integration-plan.md) — OperationEnv, three dispatch paths, adapter patterns
- [secret-service.md](secret-service.md) — SecretProtocol definition
- [identity.md](identity.md) — IdentityProvider, AuthProtocol
- [configuration.md](configuration.md) — ConfigProtocol, DynamicConfig reload
- [interface.md](interface.md) — Interface layer, auth across interfaces
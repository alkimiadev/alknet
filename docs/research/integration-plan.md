# Integration Plan: Services, PubSub, and Operations

> Status: Research / Draft
> Last updated: 2026-06-07

## Purpose

This document organizes the findings from the research phase (core.md, services.md, configuration.md, storage.md, flow.md) into an actionable integration plan. It identifies what requires changes to the core, what becomes new crates, what can be carried over from existing research specs, and what needs further specification before implementation.

The plan is organized into phases because not everything can be front-loaded. Earlier phases change the core architecture; later phases build on top. Things learned during implementation may adjust later phases.

## Key Clarifications

### Transport / Interface / Protocol — Three Layers

Carrying forward the distinction raised during review, the architecture has three distinct layers:

```
Layer 3: Application Protocol  (Call Protocol, Operations, Service Calls)
Layer 2: Interface              (SSH, raw EventEnvelope framing, HTTP/WS, DNS control channel)
Layer 1: Transport              (TCP, TLS, iroh, WebTransport, DNS)
```

A **connection** is always a (Transport, Interface) pair. The call protocol runs at Layer 3 and is agnostic to both layers below it.

This means:

| Combination | What it does | Example |
|---|---|---|
| (TLS, SSH) | Standard alknet tunnel | `alknet connect --transport tls` |
| (TCP, SSH) | Plain SSH tunnel | `alknet connect --transport tcp` |
| (iroh, SSH) | P2P SSH tunnel | `alknet connect --transport iroh` |
| (DNS, raw framing) | DNS control channel | Call protocol frames as DNS TXT queries |
| (WebTransport, SSH) | Browser SSH tunnel | Future: browser client |
| (WebTransport, raw framing) | Browser call protocol | Future: browser-to-head direct |
| (TCP, raw framing) | Direct call protocol | Local service mesh, no SSH overhead |

"Raw framing" means the 4-byte length prefix + JSON EventEnvelope format without SSH wrapping. The DNS "control channel" concept from the research is a (DNS transport, raw framing interface) pair. It carries call protocol events directly — it does NOT wrap SSH inside DNS.

### Services vs Call Protocol — Two Different Layers

From services.md:

> Services are internal — they run within a node or cluster. The call protocol is external — it's how nodes communicate with each other over SSH/QUIC/WebSocket/DNS transports.

- **irpc service calls**: Internal, synchronous request-response. Rust-to-Rust, postcard serialization, over tokio channels (local) or QUIC streams (remote). Domain-level.
- **Call protocol events**: External, cross-node, cross-language. JSON EventEnvelope frames, over any (Transport, Interface) pair. Integration-level.

A call protocol handler MAY call an irpc service internally. For example, `/head/auth/verify` receives a call protocol `call.requested` event, then calls the local `AuthProtocol::VerifyPubkey` irpc service to actually perform the check. The layers compose:

```
Call Protocol (Layer 3, external, JSON)
    └── irpc Service (Layer 3, internal, postcard)
            └── Honker Streams (Domain events, within service boundary)
```

Future work on binary encoding (replacing JSON with postcard or similar for Rust-to-Rust cross-node communication) is possible but deferred — JSON works well across platforms and the performance characteristics are acceptable for control-plane traffic.

### OperationEnv — The Universal Composition Mechanism

The `OperationEnv` pattern from `@alkdev/operations` is not a TypeScript implementation detail. It is the **universal composition mechanism** that all operation handlers receive. It maps identically across every modern boundary:

- HTTP: `POST /v1/{namespace}/{op}` → `context.env[namespace][op](input)`
- MCP: `tools/call` with tool name `{namespace}_{op}` → `context.env[namespace][op](input)`
- DNS: `{op}.{namespace}.alk.dev TXT?` → `context.env[namespace][op](input)`
- Call protocol: `call.requested` with `operationId: "/{node}/{namespace}/{op}"` → `context.env[namespace][op](input)`
- irpc: service enum dispatch → wraps the same handler → `context.env[namespace][op](input)`

The handler always sees the same interface: given a namespace and operation name, invoke it with input. The OperationEnv implements the routing. The three dispatch paths are:

```
OperationEnv (handler-facing composition)
    │
    ├── Local dispatch (in-process, direct function call through registry)
    ├── Service dispatch (in-cluster, irpc protocol enum to service backend)
    └── Remote dispatch (cross-node, call protocol EventEnvelope to head)
```

All three resolve the same way from the handler's perspective. A handler calling `context.env.secrets.derive(input)` doesn't know or care whether it becomes a local function call, an irpc protocol message, or a cross-node call protocol event. The OperationEnv chooses the routing based on where the operation is registered.

This means:
- **irpc services are one dispatch backend for OperationEnv**, not a replacement for it.
- **irpc protocol enums** (`AuthProtocol::VerifyPubkey`, `SecretProtocol::DeriveEd25519`) define the wire format for in-cluster communication. They're the Rust-to-Rust optimization path.
- **Call protocol operations** define the cross-node, cross-language wire format. They use path-based routing (`/head/auth/verify`).
- **An irpc service can be exposed as a call protocol operation** — the registry maps the path to a handler that internally calls the irpc service.
- **Both coexist** and both are needed. irpc gives you type-safe, efficient in-cluster calls. Call protocol gives you universal, cross-language, cross-node calls. OperationEnv unifies them from the handler's perspective.

The Rust implementation of OperationEnv doesn't have to be a literal `HashMap<String, HashMap<String, fn(...)>>` — it can be a struct with typed method dispatch or a registry that resolves to irpc clients — but the **behavioral contract** must match: namespace + operation name → invoke with input, return output. Handlers compose through this interface. Adapters (MCP, OpenAPI, HTTP, DNS) map to operations through this interface.

This is a hard constraint: the OperationEnv composition model must survive the Rust port intact. It's what makes operations universally composable across all interfaces.

---

## What Exists Already

### Existing Architecture Specs (reviewed/stable)

| Doc | Status | Carries Over? |
|---|---|---|
| overview.md | reviewed | Yes — needs updates for expanded scope (services, identity, interface layer) |
| transport.md | reviewed | Yes — transport trait is unchanged |
| client.md | reviewed | Yes — client behavior unchanged |
| server.md | reviewed | Yes — server handler needs minor updates for DynamicConfig/AuthService |
| tun-shim.md | deprecated | No — remains deprecated |
| napi-and-pubsub.md | reviewed | Yes — NAPI layer needs call protocol additions |

### Existing Architecture Specs (draft)

| Doc | Status | Needs |
|---|---|---|
| auth.md | draft | Promote Identity to a first-class concern. Add IdentityProvider vs AuthService relationship. |
| call-protocol.md | draft | Add OperationEnv as universal composition mechanism. Update hub/spoke → head/worker. Clarify Layer 3 position. Show three dispatch paths (local, irpc, remote). |

### Research Documents (source material)

| Doc | Content | Spec Readiness |
|---|---|---|
| core.md | Transport, call protocol, auth, services, DNS | High for most parts. DNS section needs rewrite for transport/interface separation. |
| services.md | irpc service protocols, operation context, application services | High for core services. Application services are sketches — defer to phase 4+. |
| configuration.md | Static/dynamic split, forwarding policy, multi-transport | High — this was nearly spec-ready already. Needs ADR extraction. |
| storage.md | Metagraph, identity, ACL, secrets, honker | High for data model. Integration points with core need spec work. |
| flow.md | FlowGraph, petgraph mapping, call/operation graphs | High — straightforward port of TypeScript design. |

### Existing ADRs (25 accepted)

ADR-001 through ADR-025 are accepted. Several new ADRs are needed (see Phase 0). Existing ADRs to update:
- ADR-018 (control channel for pubsub) — superseded/extended by bidirectional call protocol (ADR-024) and the Layer 2/3 model
- ADR-024, ADR-025 — update terminology from hub/spoke to head/worker

---

## Phase 0: Architecture Foundation

**Goal**: Establish the structural decisions that everything else depends on. Write ADRs, create new spec documents, adjust existing specs for the three-layer model and crate decomposition.

**Why first**: Every subsequent phase depends on knowing where types live, what the layer boundaries are, and which crates depend on which. These decisions are architectural and cheap to change now but expensive to change later.

### ADRs to Write

| ADR | Title | Key Decision |
|---|---|---|
| 026 | Transport-interface separation | Three-layer model: Transport (Layer 1) produces byte streams, Interface (Layer 2) parses them into sessions, Protocol (Layer 3) carries semantics. Valid (Transport, Interface) pairs are enumerated. SSH is an interface, not a transport. DNS control channel is a (DNS transport, raw framing interface) pair. |
| 027 | Crate decomposition | alknet-core (transport, SSH, call protocol, config, auth types, identity), alknet-secret (BIP39, SLIP-0010, AES-GCM), alknet-storage (SQLite, honker, metagraph, ACL, identity tables), alknet-flowgraph (petgraph, type compatibility). Core depends on no heavy service crates. |
| 028 | Auth as irpc service | Auth verification via IdentityProvider trait (in core). Default impl: ArcSwap<DynamicConfig>. Production impl: irpc AuthService backed by SQLite. Callers don't know the difference. |
| 029 | Identity as core type | `Identity` struct (id, scopes, resources) and `IdentityProvider` trait live in alknet-core. Derivation and storage are external concerns. |
| 030 | Static/dynamic config split | StaticConfig (transport binding, TLS, host key) vs DynamicConfig (auth, forwarding, rate limits). ArcSwap for hot reload. ConfigService wraps reloads. Promoted from research/configuration.md. |
| 031 | Forwarding policy | Rule-based allow/deny for channel_open_direct_tcpip. Default-allow for migration, default-deny for production. TransportKind-aware rules. |
| 032 | Event boundary discipline | Domain events (honker streams) stay within the owning service. Integration events (call protocol EventEnvelope) cross node boundaries. Service calls (irpc) are synchronous and internal. Never conflate the three. |
| 033 | Call protocol / irpc relationship / OperationEnv | OperationEnv is the universal composition mechanism. irpc services are one dispatch backend for OperationEnv (in-cluster, postcard). Call protocol operations are another backend (cross-node, JSON). Handlers compose through `context.env[namespace][op](input)` regardless of dispatch path. Both are Layer 3, at different scope boundaries. |
| 034 | Head/worker terminology | Replace hub/spoke with head/worker throughout. A head is also a worker. Mesh topologies are natural. |

### Spec Documents to Create or Update

| Document | Action | Source |
|---|---|---|
| `interface.md` | **Create new** | Defines Layer 2. SSH as interface. Raw framing as interface. DNS control channel as (DNS transport, raw framing interface). |
| `services.md` | **Create new** | Defines irpc service layer. Auth, Secret, Config, Storage service protocols. How irpc services relate to call protocol operations and OperationEnv. Carries from research/services.md and research/core.md service layer section. |
| `identity.md` | **Create new** | `Identity` type, `IdentityProvider` trait, auth flow for SSH and token. Carries from architecture/auth.md + research/services.md Identity section. |
| `configuration.md` | **Promote from research** | StaticConfig, DynamicConfig, ConfigService, forwarding policy, auth service relationship. Needs cleanup: remove duplicate "Problem" heading, resolve open questions per ADRs. |
| `secret-service.md` | **Create new** | Slides from research/services.md SecretProtocol definition. BIP39/SLIP-0010, key derivation paths, encryption model, lock/unlock lifecycle. |
| `storage.md` | **Create new** (or reference alknet-storage's own docs) | Metagraph data model, identity tables, ACL graph, honker integration. Carries from research/storage.md. |
| `flowgraph.md` | **Create new** (or reference alknet-flowgraph's own docs) | FlowGraph<N,E>, operation graph, call graph, petgraph mapping. Carries from research/flow.md. |
| `overview.md` | **Update** | Add crate structure, Layer 3 description, service layer concept, updated dependency list. |
| `auth.md` | **Update** | Add IdentityProvider vs AuthService relationship. Update for irpc AuthProtocol. Note: this is mostly a rename/reorg since the current auth.md already defines IdentityProvider. |
| `call-protocol.md` | **Update** | Add OperationEnv as universal composition mechanism with three dispatch paths (local, irpc service, remote). Update hub/spoke → head/worker. Show how irpc is one backend for OperationEnv, not a replacement for it. |
| `README.md` | **Update** | Add new docs and ADRs to the tables. |

### Review Checklist (Phase 0)

After writing specs and ADRs:

1. **No inline decision rationale** — all "why" decisions are in ADRs, specs reference ADR numbers
2. **No inline open questions** — all OQs are in open-questions.md, specs reference OQ numbers
3. **Terminology is consistent** — head/worker everywhere (no hub/spoke remaining)
4. **Layer boundaries are clear** — every component belongs to exactly one layer
5. **Crate dependencies are acyclic** — core doesn't depend on secret, storage, or flowgraph
6. **Every spec has YAML frontmatter** with status and last_updated

---

## Phase 1: Core Modifications

**Goal**: Modify alknet-core to support the architectural changes. This is the "adjust the foundation" phase.

**Why second**: The core changes (config split, auth service, identity type, forwarding policy) are prerequisites for the service layer and the external crates. Implementation can begin after Phase 0 ADRs and specs are reviewed and stable.

### 1.1 Configuration: Static/Dynamic Split

**Source**: research/configuration.md (nearly spec-ready)

**Changes to alknet-core**:
- Introduce `StaticConfig` struct (transport mode, listen addr, TLS config, iroh config, host key, stealth, max_auth_attempts, max_connections_per_ip)
- Introduce `DynamicConfig` struct (auth policy, forwarding policy, rate limits)
- Replace `Arc<ServerAuthConfig>` with `Arc<ArcSwap<DynamicConfig>>` in ServerHandler
- Add `ConfigReloadHandle` with `reload(DynamicConfig)` method
- Expose `reloadAuth()` / `reloadForwarding()` on the NAPI AlknetServer object

**What stays the same**: `ServeOptions` builder pattern is preserved. `StaticConfig` is constructed from `ServeOptions`. `DynamicConfig` starts with what was in `ServerAuthConfig` and gains `ForwardingPolicy`.

**New crate**: None. This is all in alknet-core.

**ADR**: 030 (static/dynamic split)

**Risk**: Low — internal refactor, no protocol changes. Default-allow forwarding preserves current behavior.

### 1.2 Identity Type and IdentityProvider Trait

**Source**: architecture/auth.md (already defines IdentityProvider), research/services.md (Identity struct)

**Changes to alknet-core**:
- Define `Identity` struct in `alknet_core::auth` (id, scopes, resources)
- Define `IdentityProvider` trait in `alknet_core::auth`
- Implement `ConfigIdentityProvider` (reads from DynamicConfig's authorized_keys)
- Wire `IdentityProvider` into `ServerHandler::auth_publickey()` — currently reads from `ServerAuthConfig`, now goes through trait
- Wire `IdentityProvider` into token auth (WebTransport path) when that lands

**What stays the same**: SSH key verification logic. The `auth_publickey()` callback just delegates to the trait instead of reading directly.

**New crate**: None. Identity is core.

**ADR**: 029 (identity as core type)

**Risk**: Low — adding a trait abstraction over existing behavior.

### 1.3 Forwarding Policy

**Source**: research/configuration.md (ForwardingPolicy section)

**Changes to alknet-core**:
- Define `ForwardingPolicy`, `ForwardingRule`, `TargetPattern` structs
- Add policy check in `channel_open_direct_tcpip` before proxy spawn
- Default: `ForwardingPolicy::allow_all()` (preserves current behavior)
- Policy is part of `DynamicConfig` and reloadable

**New crate**: None. This is in alknet-core.

**ADR**: 031 (forwarding policy)

**Risk**: Low — new check, default-allow preserves current behavior.

### 1.4 Auth Service (irpc Protocol)

**Source**: research/services.md (AuthProtocol definition), research/configuration.md (auth service approach)

**Changes to alknet-core**:
- Define `AuthProtocol` enum with `#[rpc_requests]` (behind `irpc` feature flag)
- Define `AuthResult` and `Identity` types shared between SSH auth path and irpc auth path
- Implement `AuthServiceImpl` backed by `ConfigIdentityProvider` (ArcSwap path) — the default for minimal deployments
- Future: `AuthServiceImpl` backed by SQLite (in alknet-storage) — not in this phase

**What stays the same**: The `IdentityProvider` trait is the contract. Default impl uses ArcSwap. SQL impl is additive.

**New crate**: None. Auth service types live in alknet-core.

**Feature flag**: `irpc` feature in alknet-core. When disabled, auth goes through `IdentityProvider` directly (no irpc overhead).

**ADR**: 028 (auth as irpc service), 029 (identity as core type)

**Risk**: Medium — introduces irpc dependency behind feature flag. Needs careful API design so the trait-based path and the irpc path produce identical results.

### 1.5 OperationEnv and OperationRegistry

**Source**: research/services.md (OperationContext, OperationEnv), existing call-protocol.md (OperationSpec, OperationRegistry)

**Changes to alknet-core**:
- Define `OperationContext` struct (request_id, parent_request_id, identity, metadata, env, trusted)
- Define `OperationEnv` — the universal composition mechanism with three dispatch backends:
  - **Local dispatch**: Direct function call through the operation registry
  - **Service dispatch**: irpc protocol call to a service backend
  - **Remote dispatch**: Call protocol EventEnvelope to a remote node
- Extend the existing `OperationRegistry` to support all three dispatch paths
- Define `ResponseEnvelope` as the universal return type (matching `@alkdev/operations`)
- Operation handlers receive `(input: Value, context: OperationContext) -> ResponseEnvelope`
- The `env` field on `OperationContext` allows handlers to call other operations without knowing the dispatch path

**Hard constraint**: The OperationEnv composition model must match the behavioral contract from `@alkdev/operations`. Namespace + operation name → invoke with input, return output. This is what makes operations universally composable across HTTP, MCP, DNS, call protocol, and irpc. The Rust implementation can differ in its internal dispatch mechanism, but the handler-facing API must preserve this contract.

**New crate**: None. OperationEnv, OperationContext, and OperationRegistry are core concepts in `alknet_core::call`.

**ADR**: 033 (call protocol / irpc relationship)

**Risk**: Medium — OperationEnv is a new abstraction that must coexist with the existing call protocol handler pattern. The registry currently maps paths to handlers; OperationEnv adds namespace-aware composition on top. Need to ensure the two models compose cleanly.

### 1.6 Config Service (irpc Protocol)

**Source**: research/configuration.md, research/services.md (ConfigProtocol definition)

**Changes to alknet-core**:
- Define `ConfigProtocol` enum with `#[rpc_requests]` (behind `irpc` feature flag)
- Implement `ConfigServiceImpl` backed by `ArcSwap<DynamicConfig>`
- Expose reload methods through the service

**New crate**: None. Config is core.

**Feature flag**: `irpc` feature.

**ADR**: 030 (static/dynamic split)

**Risk**: Low — thin wrapper over ArcSwap.

### 1.7 Multi-Transport Listeners

**Source**: research/configuration.md (multi-transport section)

**Changes to alknet-core**:
- Change `ServeTransportMode` from single enum to `Vec<ListenerConfig>`
- `Server::run()` spawns one accept loop per listener, sharing `DynamicConfig`, `ConnectionRateLimiter`, sessions, and shutdown signal
- Add `TransportKind::WebTransport` and `TransportKind::Dns` variants (initially tags only — no acceptor implementation)
- TOML config file support: `[[listeners]]` array-of-tables syntax

**New crate**: None. This is alknet-core server logic.

**ADR**: 026 (transport-interface separation) — TransportKind enum includes all Layer 1 types

**Risk**: Medium — changes the primary API surface of `serve()`. Backwards compat via accepting both single `transport` and `listeners` array.

### 1.8 Interface Abstraction

**Source**: New concept from review (not in research docs explicitly)

**Changes to alknet-core**:
- Define `Interface` trait that consumes a `Transport::Stream` and produces call protocol events
- `SshInterface` — wraps existing russh handler, produces SSH channels + control channel
- `RawFramingInterface` — reads length-prefixed JSON EventEnvelope frames, produces call protocol events directly (no SSH)
- The call protocol is interface-agnostic — it receives `EventEnvelope` frames from any interface

This is the most architecturally significant change in Phase 1. Currently, SSH is deeply embedded in the server handler. Extracting it into an Interface trait means:

```rust
#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
    // The session produces call protocol events and handles responses
}
```

The existing `ServerHandler` logic (auth, channel open, proxy) becomes `SshInterface`. The raw framing interface becomes a simple length-prefix reader. DNS control channel becomes (DNS transport + raw framing interface).

**This requires careful design review**. The SSH handler currently owns auth, channel management, and proxy logic. Much of that moves to Layer 3 (call protocol) or stays in the interface. The split needs to be clean.

**ADR**: 026 (transport-interface separation)

**Risk**: High — refactoring the core server handler. This is the most invasive change in Phase 1. May need to be split into sub-phases or deferred partially.

---

## Phase 2: External Crates

**Goal**: Create the new crates that core depends on by type but not by implementation.

**Why after Phase 1**: The crate boundaries are defined in Phase 0. The core types (Identity, EventEnvelope, OperationSpec, etc.) must be stable before building crates that reference them. Also, the interface abstraction from Phase 1 determines how these crates interact with the server.

### 2.1 alknet-secret

**Source**: research/services.md (SecretProtocol), research/storage.md (secrets section, key derivation)

**Contents**:
- BIP39 mnemonic generation and seed derivation
- SLIP-0010 Ed25519 HD key derivation (SLIP-0044 coin type 74')
- AES-256-GCM encryption/decryption for external credentials
- `SecretProtocol` irpc service implementation (Unlock, Lock, DeriveEd25519, DeriveEncryptionKey, Encrypt, Decrypt)
- `EncryptedData` type (key_version, salt, iv, ciphertext)
- Derivation path constants

**Dependencies**: bip39, ed25519-bip32 (or rust-bip32-ed25519), aes-gcm, sha2, irpc

**Does NOT depend on**: alknet-core, alknet-storage

**Interface back to core**: alknet-secret types (EncryptedData, derivation paths) are referenced by alknet-storage when storing encrypted nodes. The wire format is stable; core never sees the seed or derived keys.

**ADR**: 027 (crate decomposition)

**Risk**: Low — new crate, no existing code to refactor. Crypto dependencies are well-understood.

### 2.2 alknet-storage

**Source**: research/storage.md (entire document)

**Contents**:
- SQLite-backed metagraph (GraphType, NodeType, EdgeType, Graph, Node, Edge)
- Identity tables (accounts, organizations, peer_credentials, api_keys, audit_logs)
- ACL as metagraph (PrincipalNode, DelegatesEdge, access control graph)
- Encrypted node type (bridges to alknet-secret's EncryptedData format)
- Honker integration (stream_publish/subscribe, notify/listen, queue/claim)
- System DB vs Tenant DB separation
- `StorageProtocol` irpc service

**Dependencies**: rusqlite (via honker or direct), honker, serde_json, jsonschema, petgraph, irpc

**Does NOT depend on**: alknet-core, alknet-secret (but references EncryptedData type format)

**Interface back to core**: 
- `StorageIdentityProvider` implements alknet-core's `IdentityProvider` trait (queries peer_credentials + ACL graph)
- `StorageProtocol` is called via irpc from alknet-core's service layer

**ADR**: 027 (crate decomposition), 032 (event boundary discipline)

**Risk**: Medium — honker integration is new. SQLite schema needs to match the TypeScript version for compatibility.

### 2.3 alknet-flowgraph

**Source**: research/flow.md (entire document)

**Contents**:
- `FlowGraph<N, E>` generic graph over `petgraph::DiGraph`
- `NodeAttributes` / `EdgeAttributes` traits
- Operation graph construction from `OperationSpec`s
- Call graph population from `EventEnvelope` events
- Type compatibility checking (jsonschema)
- Cycle detection, topological sort, reachability queries
- Serde serialization/deserialization

**Dependencies**: petgraph, serde, serde_json, jsonschema, thiserror

**Does NOT depend on**: alknet-core, alknet-storage, alknet-secret

**Interface back to core**: `OperationSpec` and `CallNodeAttrs` types must match alknet-core's definitions. Bridge is serialization — flowgraph serializes to JSON, storage persists it.

**ADR**: 027 (crate decomposition)

**Risk**: Low — pure computation crate, no I/O, no external state. Straight port of TypeScript design.

---

## Phase 3: Integration and Wiring

**Goal**: Wire the crates together. The CLI binary and NAPI layer assemble everything.

**Why after Phase 2**: Integration requires all pieces to exist. Phase 1 defines the interfaces; Phase 2 builds the implementations; Phase 3 connects them.

### 3.1 CLI Binary (alknet crate)

**Source**: research/configuration.md (CLI config, --config flag)

**Contents**:
- `alknet serve` — parse TOML config, assemble StaticConfig + initial DynamicConfig, create services, run multi-transport server
- `alknet connect` — parse CLI flags or TOML profile, create ConnectOptions, run client
- Service assembly: for minimal deployments, use ArcSwap-backed services. For production, wire in SQLite-backed services.
- TOML config file parsing (`alknet serve --config stack.toml`)

**New dependency**: `toml` crate (for config file parsing)

### 3.2 Service Assembly

The CLI or NAPI layer is responsible for wiring services together:

```rust
// Minimal deployment (single-node, CLI)
let auth = ConfigIdentityProvider::new(dynamic_config.clone());
let config = ConfigServiceImpl::new(dynamic_config.clone());
let secret = None; // No secret service in minimal mode

// Production deployment (head node)
let auth = StorageIdentityProvider::new(storage_db);
let config = ConfigServiceImpl::new(dynamic_config.clone());
let secret = SecretServiceImpl::new(storage_db); // Holds seed in memory
```

Core doesn't know about this assembly — it receives `IdentityProvider` and `DynamicConfig` through its public API.

### 3.3 OperationEnv Wiring — Three Dispatch Paths

The OperationEnv is the universal composition mechanism. When a handler calls `context.env.secrets.derive(input)`, the runtime resolves which dispatch path to take:

**Local dispatch** (in-process):
```
handler calls context.env[namespace][op](input)
    → OperationEnv resolves the handler function from the local registry
    → Direct function call, zero serialization
    → Returns ResponseEnvelope
```

**Service dispatch** (in-cluster, irpc):
```
handler calls context.env[namespace][op](input)
    → OperationEnv resolves that this operation is backed by an irpc service
    → Serializes input via postcard, sends to AuthProtocol::VerifyPubkey via mpsc channel (local) or QUIC stream (remote)
    → Receives AuthResult, wraps in ResponseEnvelope
```

**Remote dispatch** (cross-node, call protocol):
```
handler calls context.env[namespace][op](input)
    → OperationEnv resolves that this operation lives on a remote node
    → Sends call.requested EventEnvelope via the interface (SSH channel, raw framing, DNS, etc.)
    → Receives call.responded EventEnvelope, deserializes payload
```

All three paths produce the same `ResponseEnvelope`. The handler neither knows nor cares which path was taken. The OperationEnv is wired at startup based on deployment topology:

```rust
// Minimal deployment (single node, all local)
let env = OperationEnv::local(local_registry);

// Production deployment (mix of local and remote)
let env = OperationEnv::new()
    .local("auth", auth_registry)           // Auth runs locally
    .local("config", config_registry)       // Config runs locally
    .service("secrets", secret_irpc_client) // Secret service via irpc
    .remote("worker-1", call_protocol_conn) // Worker-1 operations via call protocol
;
```

The irpc service layer is thus **one dispatch backend** for OperationEnv — the path chosen when an operation is registered as backed by an in-cluster service. It is not a replacement for OperationEnv or for the call protocol.

### 3.4 NAPI Layer Updates

**Changes to alknet-napi**:
- Expose `reloadAuth()`, `reloadForwarding()`, `reloadAll()` on the AlknetServer object
- Call protocol integration: expose operation registry for NAPI consumers to register handlers
- Service layer: expose irpc service creation for NAPI consumers

---

## Phase 4: Application Services and Advanced Features

**Goal**: Build services that register with the operation registry but don't change core.

**Why last**: These are pluggable. They depend on the core being stable (Phases 1-3) but don't affect core's architecture.

### 4.1 DNS Transport + Control Channel Interface

**Source**: research/core.md (DNS transport section)

**Scope**:
- `DnsTransport` implements `Transport` trait (Phase 1)
- `DnsAcceptor` implements `TransportAcceptor` trait
- Raw framing Interface over DNS query/response pairs
- Call protocol over DNS (not SSH over DNS — that's a separate, future goal)

**Crate**: `alknet-core` (transport module, behind `dns` feature flag)

**ADR**: 026 (transport-interface separation) — DNS is a (DNS transport, raw framing interface) pair

**Risk**: Medium — DNS protocol implementation is non-trivial. Framing, chunking, and retransmission need R&D.

### 4.2 WebTransport Transport

**Source**: architecture/auth.md (WebTransport section)

**Scope**:
- `WebTransportAcceptor` implements `TransportAcceptor` trait
- Token auth for WebTransport sessions (already designed in auth.md)
- `TransportKind::WebTransport` variant

**Crate**: `alknet-core` (behind `webtransport` feature flag)

**Risk**: Medium — requires wtransport crate dependency, QUIC listener coexistence questions (OQ-15).

### 4.3 Docker Service, Node Service, etc.

**Source**: research/services.md (application services section)

These are all pluggable services that register operations with the core's `OperationRegistry`. They don't require core changes. They're candidates for a `alknet-services` crate or individual crates.

**Crate**: New crate(s) per service, or a consolidated `alknet-services` crate

**Risk**: Low — purely additive, no core changes needed.

### 4.4 Flow Graph Real-time Construction

**Source**: research/flow.md

Wire call protocol events (call.requested, call.responded, etc.) to `FlowGraph::update_from_event()`. This is application-level wiring, not a core concern.

**Crate**: Application code in `alknet` binary or a `alknet-head` crate.

**Risk**: Low — event subscription pattern is well-established.

---

## Phase Summary

| Phase | What | Core Changes? | New Crates? | ADR Dependency |
|---|---|---|---|---|
| 0 | Architecture: ADRs, specs, review | No | No | Write all |
| 1 | Core: config split, identity, forwarding, auth service, OperationEnv, interface abstraction | Yes | No | 026-034 |
| 2 | External crates: secret, storage, flowgraph | No | Yes (3) | 027 |
| 3 | Integration: CLI assembly, NAPI, service wiring | Minor (exports) | No | 027 |
| 4 | Advanced: DNS, WebTransport, app services | Minimal (feature flags) | Maybe | 026 |

## Dependency Graph

```
                    alknet-secret
                   /             \
                  /               \
alknet-core ←────                ←── alknet-storage
     ↑               \           /
     │                alknet-flowgraph
     │
alknet-napi
alknet (CLI binary — assembles everything)
```

alknet-core depends on: russh, tokio, irpc (feature flag), serde
alknet-secret depends on: bip39, ed25519-bip32, aes-gcm, irpc
alknet-storage depends on: honker, rusqlite, petgraph, jsonschema, irpc
alknet-flowgraph depends on: petgraph, serde, jsonschema
alknet-napi depends on: alknet-core
alknet (CLI) depends on: alknet-core, alknet-secret (feature), alknet-storage (feature), alknet-flowgraph (feature), toml

No crate depends on alknet-core's internal types through a circular path. The `Identity` type, `IdentityProvider` trait, and `OperationSpec` are the narrow interface points.

---

## Open Questions to Resolve Before Phase 1

These must have answers before implementation begins:

| OQ | Question | Proposed Resolution | ADR |
|---|---|---|---|
| OQ-12 | Per-user forwarding scope vs global rules | Start with global rules + principal matching. Per-user scope from peer_credentials.metadata.scopes via IdentityProvider. | 031 |
| OQ-15 | TLS + WebTransport + iroh QUIC coexistence | Defer WebTransport to Phase 4. TLS and iroh already coexist (TCP vs UDP). | — (Phase 4) |
| OQ-16 | Transport-specific forwarding policy | Add `TransportKind` match in ForwardingRule. WebTransport clients can be restricted to alknet-* channels. | 031 |
| OQ-18 | Source of Identity.scopes — IdentityProvider, ForwardingPolicy, or both? | IdentityProvider owns scopes. ForwardingPolicy uses scopes from Identity. Both contribute. | 029 |
| OQ-19 | Separate TLS identity for WebTransport vs shared | Share certificates. QUIC is UDP, TLS is TCP, so same port works. Different subject alt names possible but not required. | — (Phase 4) |
| OQ-20 | Spoke registration and discovery on connect/disconnect | Register on connect, cleanup on disconnect. Heartbeat for liveness. Spec in call-protocol.md. | — (Phase 1) |
| OQ-22 | Client streaming in call protocol | Defer. Current model (single request, optional streaming response) covers all identified use cases. | — (defer) |
| NEW | irpc dependency: always or behind feature flag? | Feature flag. Nodes that only do SSH tunneling don't need the service layer. | 027 |
| NEW | DNS control channel scope for initial implementation? | Call protocol frames only (no SSH tunneling over DNS). That's Phase 4+ for SSH-over-DNS. | 026 |
| NEW | Should alknet-storage and alknet-secret share an irpc dependency, or each depend on it independently? | Independently. They're separate crates. irpc is a shared library they both use. | 027 |

---

## Inconsistencies and Conflations to Clean Up

The research documents have a few areas that need reconciliation:

1. **Hub/spoke vs head/worker**: core.md and services.md use head/worker. call-protocol.md still uses hub/spoke in several places. All docs need to be updated consistently. ADR-034 formalizes this.

2. **DNS as transport vs interface**: core.md conflates "DNS as transport" (encoding bytes as DNS queries) with "DNS as naming/discovery" (TXT records). The three-layer model cleanly separates these: DNS transport is Layer 1, DNS naming is a separate concern (similar to DNS-SD or iroh-dns).

3. **Service naming collision — irpc service vs call protocol operation**: The research uses "service" for both irpc protocol enums (AuthProtocol, SecretProtocol) and call protocol path-based handlers (`/head/auth/verify`, `/head/secrets/derive`). These are different concepts that compose through OperationEnv. The architecture should consistently use:
   - **irpc service** for in-cluster, Rust-to-Rust protocol enums dispatched by variant (AuthProtocol::VerifyPubkey)
   - **operation** for path-based call protocol handlers dispatched by namespace + name (`/head/auth/verify`)
   - An irpc service can back an operation — the OperationEnv routes to the right dispatch path automatically
   - Both are "services" in the broad sense, but the dispatch mechanism differs. OperationEnv unifies them.

4. **Identity model divergence**: auth.md defines `Identity` with `{id, scopes, resources}`. services.md defines `Identity` with `{node_id, fingerprint, scopes}`. These need to be unified. Proposed: `{id, scopes, resources}` where `id` is a fingerprint (for key-based auth) or account UUID (for database-backed auth).

5. **OperationEnv is a universal composition mechanism, not an implementation detail**: services.md defines `OperationEnv` as `HashMap<String, HashMap<String, fn(Value, OperationContext) -> ResponseEnvelope>>`. This is not a TypeScript pattern to be "translated" to Rust as an irpc Client<S>. The OperationEnv composition model is what makes operations universally addressable across HTTP, MCP, DNS, call protocol, and irpc. The Rust implementation can use typed method dispatch or a registry behind the scenes, but the behavioral contract — namespace + operation name → invoke with input, return output — must match. Adapters (MCP, HTTP, DNS) map to this interface. Handlers compose through this interface. irpc is one dispatch backend for OperationEnv, not a replacement for it.

6. **Event boundary discipline needs to be a hard constraint, not a suggestion**: storage.md and services.md both call this out, but it's presented as a pattern rather than a rule. The ADR (032) should make it a hard architectural constraint: domain events never cross service boundaries without projection. This prevents the "leaky event store" anti-pattern.

7. **Config file vs programmatic API**: configuration.md proposes TOML config files. ADR-011 says "no config file, programmatic-first." These need reconciliation. Proposed: TOML is an optional convenience layer that builds `StaticConfig`/`DynamicConfig`. `ServeOptions` builder pattern remains the primary API. ADR-011 is amended, not superseded — the config file is an alternative input format, not a replacement for the programmatic API.
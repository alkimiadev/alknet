# Integration Plan: Services, PubSub, and Operations

> Status: Research / Draft
> Last updated: 2026-06-09

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

## Phase 2: Core Bridge

**Goal**: Complete the interface-to-protocol bridge and add the core types that external crates and HTTP interfaces depend on. Phase 1 established the interface trait and SSH extraction but left the call protocol bridge (SshSession recv/send) as stubs and deferred key interface model refinements. Phase 2 closes those gaps so that Phase 3 crates can reference a stable, functional core.

**Why before external crates**: The external crates (alknet-secret, alknet-storage) depend on a core where the Layer 2→3 bridge actually works. Without `SshSession::recv()`/`send()` producing and consuming `InterfaceEvent` frames, the call protocol is inert for SSH sessions. Without `RawFramingInterface` implemented, there's no non-SSH path either. And without `StreamInterface`/`MessageInterface` split and `CredentialProvider`, the phase 2 research docs (interface-model, credential-provider, tls-transport) describe a target architecture that doesn't exist in code yet. These must exist before crates can wire against them.

### 2.1 SshSession Call Protocol Bridge

**Source**: interface.md (OQ-IF-01, resolved), ssh-interface-extraction task, control_channel.rs

**Current state**: `SshSession::recv()` always returns `None` and `SshSession::send()` silently discards. The `ControlChannelRouter` exists but has no handler wired. The `alknet-control:0` SSH channel is detected in `channel_open_direct_tcpip` but not bridged to `InterfaceEvent` frames.

**Changes to alknet-core**:
- Implement `SshSession::recv()` — read `EventEnvelope` frames from the `alknet-control:0` channel stream, wrap in `InterfaceEvent` with the session's `Identity`
- Implement `SshSession::send()` — write `EventEnvelope` frames to the `alknet-control:0` channel stream
- Wire `ControlChannelRouter` to bridge SSH channel data to the call protocol handler
- The session's `Identity` (from SSH auth) is attached to every `InterfaceEvent`

**Prerequisites**: Verify that `call::frame::{encode, decode}` exists and produces/consumes frames compatible with the SSH channel data stream. The `ControlChannelRouter` in `control_channel.rs` needs a handler wired — check its current API for how to register a call protocol handler.

**Why this is Phase 2 not Phase 4**: This is the duct work that connects Layer 2 (interface) to Layer 3 (protocol). Without it, SSH sessions can only forward ports — they cannot invoke call protocol operations. This is core functionality, not an advanced feature.

**New crate**: None. This is alknet-core.

**Risk**: Medium — the SSH channel → call protocol bridge needs careful framing (4-byte length prefix over the SSH channel data stream, matching `RawFramingInterface`'s wire format). The `SshHandler` already detects `alknet-*` destinations; the bridge is connecting that detection to the channel stream.

### 2.2 RawFramingInterface Implementation

**Source**: interface.md, integration-plan Phase 1.8

**Current state**: `RawFramingInterface` and `RawFramingSession` are stub types. `accept()` returns an error, `recv()` returns `None`, `send()` returns an error.

**Changes to alknet-core**:
- Implement `RawFramingInterface::accept()` — read the 4-byte length prefix + JSON `EventEnvelope` frame from the transport stream, return a `RawFramingSession` that wraps the stream
- Implement `RawFramingSession::recv()` — read length-prefixed `EventEnvelope` frames from the stream, produce `InterfaceEvent`
- Implement `RawFramingSession::send()` — write length-prefixed `EventEnvelope` frames to the stream
- Auth for raw framing: first frame on the session is an auth event carrying token data, resolved via `IdentityProvider::resolve_from_token()`. After auth succeeds, subsequent frames are call protocol `EventEnvelope` data. The `RawFramingSession` is not considered authenticated until the auth frame is processed.

**Auth design decision**: Raw framing sessions use a first-frame auth pattern. The first `InterfaceEvent` on a `RawFramingSession` carries an auth token (in the `InterfaceEvent.identity` field or a dedicated auth event type). After authentication, all subsequent frames are call protocol events. This is simpler and more secure than per-frame auth — the session has a clear auth state transition, and the token is only transmitted once. For sessions that fail auth, the session is terminated immediately.

**Why this is Phase 2**: Raw framing is the simplest interface and the foundation for all non-SSH paths (TCP mesh, WebTransport, DNS). Without it, no `MessageInterface` or `StreamInterface` other than SSH can carry call protocol traffic. HTTP interfaces (Phase 4) build on the framing logic established here.

**New crate**: None. This is alknet-core.

**Risk**: Low — straightforward length-prefixed frame reader/writer. The frame format already exists in `call::frame::{encode, decode}`. The auth design (first-frame auth) is simple and matches the `InterfaceEvent` model where `identity: Option<Identity>` is set on auth and carried forward.

### 2.3 StreamInterface / MessageInterface Split

**Source**: research/phase2/interface-model.md

**Current state**: The `Interface` trait has one form (`accept(stream) → Session`). Phase 2 research identifies that HTTP and DNS are not stream-based — they're message-based (individual request/response pairs, no persistent session). The research proposes splitting into `StreamInterface` and `MessageInterface`.

**Changes to alknet-core**:
- Rename `Interface` → `StreamInterface` (the current trait becomes the stream-specific variant)
- Rename `InterfaceSession` → `StreamInterfaceSession` (or keep as `InterfaceSession` — it's already specific to stream sessions)
- Add `MessageInterface` trait: `handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>`
- Add `InterfaceRequest` and `InterfaceResponse` types
- Add `HttpInterface` stub (struct and impl signature, axum not wired yet)
- Add `DnsInterface` stub (struct definition only)
- Restructure `InterfaceConfig` enum: current `InterfaceConfig::Ssh(SshInterfaceConfig)` and `InterfaceConfig::RawFraming(RawFramingConfig)` become `StreamInterfaceConfig::Ssh` and `StreamInterfaceConfig::RawFraming`. Add `MessageInterfaceConfig` variants for HTTP and DNS.
- Update `ListenerConfig` to include `Stream`, `Http`, and `Dns` variants (per ADR-035 and updated interface.md)
- Add `TransportKind::WebTransport` as a tag-only variant (no acceptor implementation) — this was planned for Phase 1 but never added. It's a trivial addition that prevents a breaking change later.
- Note: `TransportKind::Dns` was never added to the code, so no removal is needed. The updated specs correctly show DNS as a `MessageInterface` with its own `ListenerConfig::Dns` variant, not a transport.

**Why this is Phase 2**: This is a type-system change that affects how all future interfaces are implemented. If we build HTTP on top of `Interface` (singular) and then need to split later, we'd refactor HTTP, DNS, WebSocket, and any other interface added in Phases 4+. Doing the split now is cheap — it's a rename + new trait + two stubs — and prevents a larger refactor later.

**New crate**: None. This is alknet-core.

**ADR**: 035 (StreamInterface/MessageInterface split — supersedes the Layer 2 aspects of ADR-026)

**Risk**: Low — rename and new trait. Existing `SshInterface` and `RawFramingInterface` become `StreamInterface` implementations. No behavior change for stream-based interfaces. The `InterfaceConfig` enum restructuring and `TransportKind::WebTransport` addition are mechanical changes.

**Scheduling note**: This task should be done early in Phase 2 because all subsequent tasks (2.1, 2.2, 2.4, 2.5, 2.6, 2.7) reference the new trait names. It can be done in parallel with 2.1 and 2.2 since they're mostly additive.

### 2.4 CredentialProvider Trait and CredentialSet

**Source**: research/phase2/credential-provider.md

**Current state**: No outbound credential resolution exists. Each service wrapper would need to independently retrieve and manage credentials.

**Changes to alknet-core**:
- Define `CredentialProvider` trait in `alknet_core::credentials`
- Define `CredentialSet` enum: `ApiKey`, `Basic`, `Bearer`, `S3AccessKey`, `OidcToken`, `Custom`
- Implement `ConfigCredentialProvider` — a config-backed stub that reads API keys and static credentials from `DynamicConfig`. This is the Phase 2 default: simple, no secret service dependency, sufficient for testing and single-node deployments.
- Wire into `OperationEnv` so handlers can access credentials through `context.env` (or a separate `CredentialProvider` field on `OperationContext` — implementation detail)
- Define the `SecretStoreCredentialProvider` type and its interface (reads from `SecretProtocol::Decrypt`, holds in RAM) but **do not implement the body** — leave it as a stub that returns `None`. Full implementation requires alknet-secret (Phase 3).

**Why this is Phase 2**: The secret crate (Phase 3) needs `CredentialProvider` as a consumer of `SecretProtocol::Decrypt`. The trait and enum must exist in core before the secret crate can wire against them. This is the same pattern as `IdentityProvider` — trait in core, default impl uses simple storage, production impl uses the secret service.

**New crate**: None. Trait and enum in alknet-core.

**Risk**: Low — new trait and enum, no existing code changes. `ConfigCredentialProvider` is a simple config-backed lookup. `SecretStoreCredentialProvider` stub returns `None` until Phase 3 provides the secret service dependency.

**Split note**: This task is naturally split into:
- **2.4a** (this phase): Define `CredentialProvider` trait, `CredentialSet` enum, `ConfigCredentialProvider` impl, wire into `OperationEnv`/`OperationContext`. This is self-contained and testable.
- **2.4b** (Phase 3, after alknet-secret exists): Implement `SecretStoreCredentialProvider` backed by `SecretProtocol::Decrypt`. This requires alknet-secret as a dependency.

### 2.5 ListenerConfig Update and HTTP Listener Stub

**Source**: research/phase2/tls-transport.md

**Current state**: Phase 1 added `ListenerConfig` with `Stream` variant (transport + interface pair). Phase 2 research adds `Http` and `Dns` listener variants for message-based interfaces. The Phase 1 implementation also added `TransportKind::Dns` which should be removed (DNS is a `MessageInterface`, not a transport).

**Changes to alknet-core**:
- `TransportKind::Dns` removal: **No-op** — `TransportKind` in the current code has `Tcp`, `Tls`, and `Iroh` only. `Dns` was never added to the enum. The updated specs correctly show DNS as a `MessageInterface` with its own `ListenerConfig::Dns` variant (per ADR-035), not as a transport variant.
- Add `ListenerConfig::Http` variant: `{ bind_addr, tls, stealth }`
- Add `ListenerConfig::Dns` variant: `{ bind_addr, tls }` (DNS as a MessageInterface with its own listener)
- Extend the server accept loop to handle `ListenerConfig::Http` by spawning an axum router when `stealth` mode detects HTTP traffic (replacing `send_fake_nginx_404`)
- `HttpInterface` stub defined in 2.3 gets its structural types but no route implementations yet

**Why this is Phase 2**: The `ListenerConfig` is the server's primary configuration type. Adding HTTP and DNS listener variants now means Phase 3+ crates and Phase 4 HTTP implementation can reference the right type from the start. Removing `TransportKind::Dns` before any code depends on it prevents a breaking change later.

**New crate**: None. This is alknet-core. New dependency: `axum` (behind `http` feature flag).

**Risk**: Low — type changes and a stub axum router. The `send_fake_nginx_404` → axum handoff is a small change to the existing stealth detection code. Full HTTP route implementations are Phase 4.

### 2.6 API Keys in DynamicConfig

**Source**: research/phase2/interface-model.md (Config section), research/phase2/credential-provider.md

**Current state**: `DynamicConfig.auth` has `authorized_keys` for SSH auth and `token` settings but no simple bearer API keys for service accounts or automation.

**Changes to alknet-core**:
- Add `[[auth.api_keys]]` section to `DynamicConfig`: prefix, hash (SHA-256), scopes, description, optional TTL
- Extend `ConfigIdentityProvider::resolve_from_token()` to verify API keys in addition to AuthTokens
- API keys are shorter and simpler than AuthTokens — no Ed25519 key pair needed, just a hash-verified bearer string
- `SecretStoreCredentialProvider` can also resolve API keys when database-backed storage is available

**Why this is Phase 2**: The HTTP interface (Phase 4) needs bearer token auth, and the simplest path is API keys that already work with `IdentityProvider::resolve_from_token()`. Without this, Phase 4 HTTP auth has no config-based auth mechanism.

**New crate**: None. This is alknet-core.

**Risk**: Low — additive config section and an additional lookup path in an existing trait method.

### 2.7 Axum HTTP Router Scaffold

**Source**: research/phase2/tls-transport.md

**Changes to alknet-core** (behind `http` feature flag):
- Add `axum` dependency (behind feature flag)
- Create `alknet_core::http` module with an axum `Router` scaffold:
  - Auth middleware that extracts `Authorization: Bearer <token>` and calls `IdentityProvider::resolve_from_token()`, attaching the resolved `Identity` to the request extensions
  - Stealth handoff: replace `send_fake_nginx_404` with axum router serving the `BufReader<TlsStream>`
  - A default 404 handler for any unmatched routes (no hardcoded operation paths)
- No operational routes yet — the question of how HTTP paths map to operation invocations depends on the from_openapi / spec-generation work and is deferred to Phase 5. Custom routes (git, S3, OpenAI proxy) will register directly with the axum router at their own paths, sharing the auth middleware but with their own routing logic.
- The `ListenerConfig::Http` variant and stealth mode handoff are established here so that HTTP traffic reaches axum with auth context. Routing *inside* axum is a later concern.

**Why this is Phase 2**: The auth middleware and stealth handoff are prerequisites for any HTTP endpoint. Without this, the only way to reach call protocol operations is via SSH. The scaffold gets HTTP traffic to axum with identity — the specific routes and path conventions are intentionally not specified here.

**New crate**: None. In alknet-core behind `http` feature flag.

**Risk**: Low — structural scaffold with auth middleware and stealth handoff only. No operational routes or path conventions.

**Open question**: How should external HTTP paths map to alknet operations? The internal path convention (`/{namespace}/{op}` over call protocol channels) is one design; external HTTP paths are determined by the API being exposed (OpenAI `/v1/chat/completions`, S3 `/{bucket}/{key}`, git `/{repo}.git/info/refs`). The inverse of `from_openapi` — generating an OpenAPI spec from registered operations and mapping those to HTTP routes — will determine the answer. This is deferred to Phase 5.

---

## Phase 3: External Crates

**Goal**: Create the new crates that core depends on by type but not by implementation.

**Why after Phase 2**: The core types and bridges must be stable before building crates that reference them. Phase 2 ensures that the `InterfaceSession` bridge works, `CredentialProvider` exists, and `ListenerConfig` has its final shape. The external crates can then wire against a functional core.

### 3.1 alknet-secret

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

### 3.2 alknet-storage

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

### 3.3 alknet-flowgraph

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

## Phase 4: Integration and Wiring

**Goal**: Wire the crates together. The CLI binary and NAPI layer assemble everything.

**Why after Phase 3**: Integration requires all pieces to exist. Phase 1 defines the interfaces; Phase 2 completes the core bridge; Phase 3 builds the crate implementations; Phase 4 connects them.

### 4.1 CLI Binary (alknet crate)

**Source**: research/configuration.md (CLI config, --config flag)

**Contents**:
- `alknet serve` — parse TOML config, assemble StaticConfig + initial DynamicConfig, create services, run multi-transport server
- `alknet connect` — parse CLI flags or TOML profile, create ConnectOptions, run client
- Service assembly: for minimal deployments, use ArcSwap-backed services. For production, wire in SQLite-backed services.
- TOML config file parsing (`alknet serve --config stack.toml`)

**New dependency**: `toml` crate (for config file parsing)

### 4.2 Service Assembly

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

### 4.3 OperationEnv Wiring — Three Dispatch Paths

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

### 4.4 NAPI Layer Updates

**Changes to alknet-napi**:
- Expose `reloadAuth()`, `reloadForwarding()`, `reloadAll()` on the AlknetServer object
- Call protocol integration: expose operation registry for NAPI consumers to register handlers
- Service layer: expose irpc service creation for NAPI consumers

### 4.5 Architecture Doc Sync

After Phase 2 core bridge changes are implemented and before Phase 3 crate development begins, the architecture docs should be updated to reflect the implementation state. The first round of doc sync has already been completed (commit `cfc4400`) based on Phase 2 research findings — this covered:

- StreamInterface/MessageInterface split in interface.md
- CredentialProvider/CredentialSet in credentials.md
- API keys in auth.md and configuration.md
- ListenerConfig variants for HTTP and DNS
- Resolved open questions (OQ-IF-01, OQ-IF-02, etc.)
- New ADRs (035, 036, 037)

A **second doc sync** will be needed after Phase 2 implementation is complete to capture any deviations between the spec and the actual implementation (e.g., if `InterfaceConfig` was restructured differently, or if the raw framing auth design differs from the first-frame approach specified here). This second sync should be done before Phase 3 crate development begins.

---

## Phase 5: Application Services and Advanced Features

**Goal**: Build services that register with the operation registry but don't change core.

**Why last**: These are pluggable. They depend on the core being stable (Phases 1-4) but don't affect core's architecture.

### 5.1 DNS Transport + Control Channel Interface

**Source**: research/core.md (DNS transport section)

**Scope**:
- `DnsInterface` (already defined as a `MessageInterface` stub in Phase 2) gets full implementation
- DNS server that encodes/decodes `EventEnvelope` frames as DNS TXT query/response pairs
- Call protocol over DNS (not SSH over DNS — that's a separate, future goal)
- AuthToken embedded in DNS query labels

**Crate**: `alknet-core` (behind `dns` feature flag)

**ADR**: 026 (transport-interface separation) — DNS is a `MessageInterface`, not a (DNS transport, raw framing) pair

**Risk**: Medium — DNS protocol implementation is non-trivial. Framing, chunking, and retransmission need R&D.

### 5.2 WebTransport Transport

**Source**: architecture/auth.md (WebTransport section), research/phase2/tls-transport.md

**Scope**:
- `WebTransportAcceptor` implements `TransportAcceptor` trait
- Token auth for WebTransport sessions (AuthToken in CONNECT URL, `IdentityProvider::resolve_from_token()`)
- `TransportKind::WebTransport` variant
- QUIC listener coexistence with iroh on UDP 443

**Crate**: `alknet-core` (behind `webtransport` feature flag)

**Risk**: Medium — requires wtransport crate dependency, QUIC listener coexistence questions (OQ-15).

### 5.3 Full HTTP Interface Implementation

**Source**: research/phase2/tls-transport.md

**Scope**:
- Replace stub handlers in the Phase 2 axum scaffold with actual operation dispatch
- `POST /v1/{namespace}/{op}` → `registry.invoke(namespace, op, input)` (mutation)
- `GET /v1/{namespace}/{op}` → `registry.invoke(namespace, op, input)` (query, params as input)
- `GET /v1/{namespace}/{op}` SSE → `registry.subscribe(namespace, op, input)` (subscription)
- `GET /v1/schema` → `registry.list_operations()`
- OpenAPI spec generation from `OperationRegistry`
- WebSocket upgrade handler for persistent browser connections

**Crate**: `alknet-core` (behind `http` feature flag)

**Risk**: Medium — full HTTP routing, SSE streaming, auth middleware integration with OperationEnv.

### 5.4 Docker Service, Node Service, Git Service, etc.

**Source**: research/services.md (application services section), research/references/gitserver/

These are all pluggable services that register operations with the core's `OperationRegistry`. They don't require core changes. They're candidates for a `alknet-services` crate or individual crates.

**Git Service** path (see research/references/gitserver/ and research/references/gitlfs/):
- Use `gitserver-core` as the git protocol engine (transport-agnostic, library-first design)
- `gitserver-http` nested in alknet's axum router for HTTPS git
- `rudolfs` (or a fork) as the LFS layer, backed by rustfs S3 storage
- Auth via `IdentityProvider` → gitserver's `AuthConfig`
- Operations: `git.clone`, `git.push`, `git.pull` registered in OperationRegistry

**Crate**: New crate(s) per service, or a consolidated `alknet-services` crate

**Risk**: Low — purely additive, no core changes needed.

### 5.5 Flow Graph Real-time Construction

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
| 2 | Core bridge: SshSession recv/send, RawFramingInterface, StreamInterface/MessageInterface split, CredentialProvider (trait+stub), HTTP listener stub, API keys | Yes | No | 035, 036, 037, phase2 research |
| 3 | External crates: secret, storage, flowgraph | No | Yes (3) | 027 |
| 4 | Integration: CLI assembly, NAPI, service wiring, doc sync | Minor (exports) | No | 027 |
| 5 | Advanced: DNS, WebTransport, full HTTP, application services | Minimal (feature flags) | Maybe | 026 |

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

alknet-core depends on: russh, tokio, irpc (feature flag), serde, axum (feature flag)
alknet-secret depends on: bip39, ed25519-bip32, aes-gcm, sha2, irpc
alknet-storage depends on: honker, rusqlite, petgraph, jsonschema, irpc
alknet-flowgraph depends on: petgraph, serde, jsonschema
alknet-napi depends on: alknet-core
alknet (CLI) depends on: alknet-core, alknet-secret (feature), alknet-storage (feature), alknet-flowgraph (feature), toml

No crate depends on alknet-core's internal types through a circular path. The `Identity` type, `IdentityProvider` trait, and `OperationSpec` are the narrow interface points.

---

## Open Questions to Resolve Before Phase 2

These must have answers before Phase 2 implementation begins. Phase 0/1 questions are resolved.

| OQ | Question | Proposed Resolution | Phase | ADR |
|---|---|---|---|---|
| ~~OQ-12~~ | Per-user forwarding scope vs global rules | **Resolved**: Start with global rules + principal matching. Per-user scope from peer_credentials.metadata.scopes via IdentityProvider. | 1 | 031 |
| ~~OQ-16~~ | Transport-specific forwarding policy | **Resolved**: Add `TransportKind` match in ForwardingRule. | 1 | 031 |
| ~~OQ-18~~ | Source of Identity.scopes | **Resolved**: IdentityProvider owns scopes. ForwardingPolicy uses scopes from Identity. | 1 | 029 |
| ~~OQ-22~~ | Client streaming in call protocol | **Resolved**: Defer. Single request + optional streaming response covers all identified use cases. | — | — |
| ~~OQ-IF-01~~ | How does InterfaceSession relate to EventEnvelope? | **Resolved**: `InterfaceSession::recv()` returns `Option<InterfaceEvent>` where `InterfaceEvent` carries `EventEnvelope` + `Identity`. `send()` accepts `EventEnvelope`. The SshSession bridge implements this over `alknet-control:0`. For `MessageInterface`, `InterfaceRequest`/`InterfaceResponse` normalize request/response pairs. See interface.md, ADR-035. | 2 | 035 |
| ~~OQ-IF-02~~ | Should SshInterface own ForwardingPolicy checks? | **Resolved**: ForwardingPolicy is Layer 3 (policy), channel open/close lifecycle is Layer 2. SshInterface reports channel requests to Layer 3; Layer 3 applies policy. Current implementation already does this. | 2 | 031 |
| OQ-15 | TLS + WebTransport + iroh QUIC coexistence | Defer WebTransport to Phase 5. TLS and iroh already coexist (TCP vs UDP). | 5 | — |
| OQ-19 | Separate TLS identity for WebTransport vs shared | Share certificates. QUIC is UDP, TLS is TCP, same port works. Different subject alt names possible but not required. | 5 | — |
| OQ-20 | Worker registration and discovery on connect/disconnect | Register on connect, cleanup on disconnect. Heartbeat for liveness. Spec in call-protocol.md. | 2+ | — |
| OQ-P2-01 | Should MessageInterface and StreamInterface share a common trait? | **Resolved**: Independent traits. Different signatures (`handle_request` vs `accept` + session lifecycle), different transport ownership (self-managed vs provided). A common super-trait adds complexity without benefit. ADR-035 accepted. | 2 | 035 |
| OQ-P2-02 | Should HTTP share a port with the SSH listener? | **Resolved**: Start with separate ports. Stealth mode byte-peek on shared port 443 already detects SSH vs HTTP. ALPN multiplexing is a future optimization that doesn't change the interface abstraction. | 2 | — |
| OQ-P2-03 | Should the HTTP interface auto-generate OpenAPI specs from OperationRegistry? | **Resolved**: Yes, but Phase 5+. The HTTP interface needs to exist first (Phase 5.3). | 5 | — |
| OQ-P2-04 | How do self-hosted services authenticate via alknet? | **Resolved**: Three-phase approach. Phase A: shared secret (`CredentialSet::Bearer` or `S3AccessKey`). Phase C: identity-bound credentials via `ManagedCredentialProvider`. Phase D: alknet as OIDC provider. `CredentialProvider` trait in core enables Phase A immediately. ADR-036 accepted. | 2-5 | 036 |

---

## Inconsistencies and Conflations to Clean Up

The research documents have a few areas that need reconciliation:

1. **Hub/spoke vs head/worker**~~: core.md and services.md use head/worker. call-protocol.md still uses hub/spoke in several places. All docs need to be updated consistently. ADR-034 formalizes this.~~ **Fixed**: call-protocol.md, auth.md, open-questions.md, and napi-and-pubsub.md updated to head/worker terminology. ADRs are historical records and retain original terminology. ADR-034 still needed to formalize the decision.

2. **DNS as transport vs interface**: core.md conflates "DNS as transport" (encoding bytes as DNS queries) with "DNS as naming/discovery" (TXT records). The three-layer model cleanly separates these: DNS is a `MessageInterface`, not a transport. **Phase 2 removes `TransportKind::Dns`** and adds `ListenerConfig::Dns`.

3. **Service naming collision — irpc service vs call protocol operation vs external service**: The research uses "service" for both irpc protocol enums and call protocol path-based handlers. See research/phase2/definitions.md for full disambiguation. The architecture should consistently use: **irpc service** (in-cluster, Rust-to-Rust), **operation** (path-based call protocol handler), **external service** (third-party endpoint), and **application service** (handler registered in OperationRegistry).

4. **Identity model divergence**~~: auth.md defines `Identity` with `{id, scopes, resources}`. services.md defines `Identity` with `{node_id, fingerprint, scopes}`.~~ **Fixed**: auth.md has the correct unified definition `{id, scopes, resources}`.

5. **OperationEnv is a universal composition mechanism, not an implementation detail**~~: services.md defines `OperationEnv` as `HashMap<String, HashMap<String, fn(...)>>`.~~ **Acknowledged**: The behavioral contract (namespace + operation name → invoke) must match. The Rust implementation can use typed dispatch behind the scenes.

6. **Event boundary discipline needs to be a hard constraint, not a suggestion**~~: storage.md and services.md both call this out, but it's presented as a pattern rather than a rule.~~ **Formalized**: ADR-032 makes it a hard architectural constraint. See also research/phase2/definitions.md (Domain Events vs Integration Events).

7. **Config file vs programmatic API**: configuration.md proposes TOML config files. ADR-011 says "no config file, programmatic-first." **Proposed**: TOML is an optional convenience layer that builds `StaticConfig`/`DynamicConfig`. `ServeOptions` builder pattern remains the primary API. ADR-011 is amended, not superseded.

8. **Interface model needs StreamInterface/MessageInterface split**: The current `Interface` trait assumes persistent byte streams. HTTP and DNS don't fit (they handle individual requests, not sessions). **Phase 2 addresses this** — rename `Interface` → `StreamInterface`, add `MessageInterface`, add `HttpInterface` stub. See research/phase2/interface-model.md.

9. **SshSession recv/send stubs are core, not "Phase 4"**: The Phase 1 implementation left `SshSession::recv()` and `SshSession::send()` as stubs returning `None` / silently discarding. This makes the interface model inert for call protocol operations. The bridge between SSH channels and `InterfaceEvent`/`EventEnvelope` frames is a **Phase 2** concern, not a future feature. See Phase 2.1.

10. **CredentialProvider is missing from core**: Outbound auth (how alknet authenticates to external services) has no trait or implementation. This is needed before any HTTP API integration work. **Phase 2.4** adds the trait and enum to core; Phase 3 (alknet-secret) provides the storage-backed implementation. See research/phase2/credential-provider.md.

11. **Architecture docs need sync after Phase 2**: The current architecture docs (interface.md, auth.md, services.md, call-protocol.md) reflect the pre-Phase-0/1 state. After Phase 2 core bridge changes land, these must be updated to reflect StreamInterface/MessageInterface, CredentialProvider, HTTP listener, and the functional call protocol bridge. **Phase 4.5** is the doc sync point.
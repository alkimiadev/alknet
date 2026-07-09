---
status: draft
last_updated: 2026-07-09
---

# Open Questions

Each open question lives in its own file under [`questions/`](questions/),
named `NNN-slug.md` (mirroring the ADR convention). This file is the index:
theme-grouped tables for scannability, plus a cross-theme
[Deferred / Blocked](#deferred--blocked) section that surfaces the
safe-exit deferrals with their blocking conditions inline — so "what's
currently parked and why" is answerable at a glance.

**Status values**:
- `open` — Needs to be resolved now. Has a clear path to resolution.
- `resolved` — Decided. The resolution is stated cleanly, without caveats about how it could be changed later.
- `deferred(scope)` — Cannot be resolved yet. The information doesn't exist. Has a concrete blocking condition (e.g., "blocked on: alknet-agent crate spec"). Not a failure — scope management.
- `partially resolved` — Some aspects decided, others deferred or open.
- `dissolved` — The question was reframed out of existence (e.g., superseded by an ADR that retires the premise). Kept for reference.

Door type classifications follow ADR-009 — they describe **reversal cost** (how expensive it is to undo), not urgency:
- **One-way door**: Reversal requires rewriting significant code or permanently closes a capability. Getting it wrong is expensive — requires ADR before implementation.
- **Two-way door**: Reversal is cheap or additive. Getting it wrong is recoverable — decide, implement, revert if needed.

Door type is separate from whether a decision is made. A two-way door is a decision you make now and can revert later, not a decision to defer. See ADR-009 §"What this framework is NOT."

## By Theme

### Core Types

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-01](questions/001-bistream-type-definition.md) | BiStream Type Definition | resolved | one | high |
| [OQ-02](questions/002-authcontext-resolution-timing.md) | AuthContext Resolution Timing | resolved | one | high |

### ALPN and Routing

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-03](questions/003-alpn-string-naming-convention.md) | ALPN String Naming Convention | resolved | one | med |
| [OQ-04](questions/004-dynamic-handler-registration-at-runtime-vs-static-at-startup.md) | Dynamic Handler Registration at Runtime vs Static at Startup | resolved | two | low |

### Transport and Endpoint

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-05](questions/005-multi-connectivity-endpoint.md) | Multi-Connectivity Endpoint | resolved | one | high |
| [OQ-06](questions/006-server-side-alpn-vs-client-side-alpn.md) | Server-Side ALPN vs Client-Side ALPN | resolved | one | low |

### Call Protocol

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-07](questions/007-call-protocol-scope-within-a-connection.md) | Call Protocol Scope Within a Connection | resolved | two | med |

### Security

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-08](questions/008-vault-integration-point.md) | Vault Integration Point | resolved | one | med |

### Deferred Questions

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-09](questions/009-wasm-target-boundaries.md) | WASM Target Boundaries | deferred | one | low |
| [OQ-10](questions/010-git-adapter-scope-smart-protocol-only-or-full-server.md) | Git Adapter Scope — Smart Protocol Only or Full Server? | deferred | two | low |

### alknet-core

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-11](questions/011-handler-level-auth-resolution-observability.md) | Handler-Level Auth Resolution Observability | resolved | two | med |
| [OQ-12](questions/012-tls-identity-provisioning-in-alknetendpoint.md) | TLS Identity Provisioning in AlknetEndpoint | resolved | one | high |
| [OQ-13](questions/013-operation-path-format-and-routing-scope.md) | Operation Path Format and Routing Scope | resolved | two | med |
| [OQ-14](questions/014-batch-operation-semantics.md) | Batch Operation Semantics | resolved | two | low |

### alknet-call

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-15](questions/015-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | resolved | one | high |
| [OQ-16](questions/016-safe-vault-operations-for-call-protocol-exposure.md) | Safe Vault Operations for Call Protocol Exposure | resolved | one | high |
| [OQ-17](questions/017-abort-cascade-semantics-for-nested-calls.md) | Abort Cascade Semantics for Nested Calls | resolved | one/two | high |
| [OQ-18](questions/018-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | resolved | one/two | high |
| [OQ-19](questions/019-session-scoped-operation-registries-and-agent-written-operations.md) | Session-Scoped Operation Registries and Agent-Written Operations | resolved | one/two | med |

### alknet-vault

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-20](questions/020-salt-kdf-and-encryption-key-derivation-method.md) | Salt/KDF and Encryption Key Derivation Method | resolved | one/two | high |
| [OQ-21](questions/021-remote-vault-administration.md) | Remote Vault Administration | resolved | one | med |
| [OQ-22](questions/022-key-rotation-mechanism.md) | Key Rotation Mechanism | resolved | one/two | med |
| [OQ-23](questions/023-handler-identity-registration-path-and-composition-authority.md) | Handler Identity Registration Path and Composition Authority | resolved | one/two | high |
| [OQ-24](questions/024-operation-error-schemas.md) | Operation Error Schemas | resolved | one/two | high |

### Call Client and Adapters

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-25](questions/025-remote-safe-marking-shape-for-callclient-peer-scoped-filtering-dissolved-by-adr-029.md) | ~~Remote-Safe Marking Shape for CallClient Peer-Scoped Filtering~~ (Dissolved by ADR-029) | dissolved | one/two | med |
| [OQ-26](questions/026-operationadapter-error-type-adaptererror-variants.md) | OperationAdapter Error Type (AdapterError Variants) | resolved | two | med |
| [OQ-27](questions/027-from-call-re-import-trigger.md) | from_call Re-Import Trigger | resolved | two | low |
| [OQ-28](questions/028-from-call-namespace-collision-behavior.md) | from_call Namespace Collision Behavior | resolved | two | low |
| [OQ-29](questions/029-callclient-tls-client-auth-and-remote-identity-verification.md) | CallClient TLS Client-Auth and Remote-Identity Verification | resolved | one/two | high |
| [OQ-30](questions/030-peerref-any-routing-policy.md) | PeerRef::Any Routing Policy | resolved | two | low |
| [OQ-31](questions/031-services-list-peers-re-export-semantics.md) | services/list-peers Re-Export Semantics | resolved | two | low |
| [OQ-32](questions/032-multi-hop-federation.md) | Multi-Hop Federation | deferred(scope) | one/two | low |
| [OQ-33](questions/033-peerid-cryptographic-identity-vs-stable-logical-identifier.md) | PeerId — Cryptographic Identity vs Stable Logical Identifier | resolved | one/two | high |
| [OQ-34](questions/034-persistent-peer-registry-cross-node-state-storage.md) | Persistent Peer Registry (Cross-Node State Storage) | resolved | one/two | med |

### Storage and Adapters

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-35](questions/035-api-key-identity-vs-peer-identity-dissolved.md) | ~~API Key Identity vs Peer Identity~~ (Dissolved) | dissolved | one | med |
| [OQ-36](questions/036-concrete-persistence-adapter-shapes.md) | Concrete Persistence Adapter Shapes | resolved | two | med |

### TLS Identity

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-37](questions/037-x-509-outgoing-only-case-three-peer-roles.md) | X.509 Outgoing-Only Case (Three Peer Roles) | resolved | one | med |

### alknet-http

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-38](questions/038-webtransport-standalone-relay-service-scope.md) | WebTransport Standalone Relay Service Scope | open | one/two | low |
| [OQ-39](questions/039-to-openapi-published-spec-versioning.md) | `to_openapi` Published-Spec Versioning | resolved | one/two | med |
| [OQ-40](questions/040-reqwest-client-config-and-connection-pooling.md) | reqwest Client Config and Connection Pooling | resolved | two | low |
| [OQ-41](questions/041-stream-operators-library.md) | Stream Operators Library | deferred(scope) | two | low |

### Runtime-Spawned Resources and Ownership

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-42](questions/042-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership for Runtime-Spawned Resources | resolved | one | high |

### alknet-tty

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-43](questions/043-ttycontrol-as-a-clone-trait-object.md) | `TtyControl` as a `Clone` trait object | resolved | one | med |
| [OQ-44](questions/044-terminal-modes-tty-modes.md) | Terminal Modes (TTY modes) | deferred(scope) | two | low |
| [OQ-45](questions/045-flow-control-for-high-throughput-stdout.md) | Flow Control for High-Throughput stdout | resolved | two | low |
| [OQ-46](questions/046-runner-api-surface.md) | Runner API Surface | deferred(scope) | two | low |
| [OQ-47](questions/047-stdin-closure-canonical-signal.md) | Stdin Closure Canonical Signal | resolved | two | low |

### alknet-docker

| OQ | Title | Status | Door | Pri |
|----|-------|--------|------|-----|
| [OQ-48](questions/048-network-and-volume-operation-surface.md) | Network and Volume Operation Surface | deferred(scope) | two | low |
| [OQ-49](questions/049-image-build-buildkit-scope.md) | Image Build (buildkit) Scope | deferred(scope) | two | low |
| [OQ-50](questions/050-docker-system-events-subscription.md) | Docker System Events Subscription | deferred(scope) | two | low |
| [OQ-51](questions/051-container-create-options-surface.md) | Container Create Options Surface | deferred(scope) | two | med |

## Deferred / Blocked

The safe-exit visibility surface. These questions are parked because the
information needed to resolve them does not exist yet — each has a concrete
blocking condition. They are not failures; they are scope management. See
ADR-009 §"Safe Exit: Deferred Decisions." This section exists so "what's
currently blocking the architect" is answerable at a glance, not by
filtering the tables above.

### OQ-09: WASM Target Boundaries

- **Blocked on**: A concrete server-side WASM use case, or a deliberate confirmation that WASM stays a client-side design constraint. Tracked as `architecture/oq-09-wasm-server-use-case` in `tasks/architecture/`.
- **Priority**: low
- **Amendment (2026-07-09)**: The `Connection` door is now open via `Connection::from_stream` (ADR-065) — a `Connection` can be constructed from any wasm-compatible stream. What remains closed is the **accept-loop runtime** (`tokio::spawn` does not run on WASM; `PendingRequestMap`/`CallAdapter` use tokio channels). The blocking condition (a concrete server-side WASM use case) is unchanged.
- **Full file**: [OQ-09](questions/009-wasm-target-boundaries.md)

### OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Blocked on**: Speccing the alknet-git crate — resolve this when that crate is specified, not deferred past it. Tracked as `architecture/oq-10-git-adapter-spec` in `tasks/architecture/`.
- **Priority**: low
- **Full file**: [OQ-10](questions/010-git-adapter-scope-smart-protocol-only-or-full-server.md)

### OQ-32: Multi-Hop Federation

- **Blocked on**: A concrete use case for multi-hop federation. The one-hop model covers all current use cases (head→worker, runner→hub).
- **Priority**: low
- **Full file**: [OQ-32](questions/032-multi-hop-federation.md)

### OQ-41: Stream Operators Library

- **Blocked on**: A handler that needs stream operators and finds the existing combinators (`Box::pin(stream::iter(...))`, `async_stream::stream!`, `futures::stream`) insufficient. The operators library is a convenience, not a prerequisite for any handler.
- **Priority**: low
- **Full file**: [OQ-41](questions/041-stream-operators-library.md)

### OQ-44: Terminal Modes (TTY modes)

- **Blocked on**: a concrete mode-control use case (a deployment that needs to set echo/raw/canonical/etc. modes on a PTY, beyond the backend's defaults).
- **Priority**: low
- **Full file**: [OQ-44](questions/044-terminal-modes-tty-modes.md)

### OQ-46: Runner API Surface

- **Blocked on**: a concrete runner-policy use case that forces the API surface (job management, log persistence, task graph integration).
- **Priority**: low
- **Full file**: [OQ-46](questions/046-runner-api-surface.md)

### OQ-48: Network and Volume Operation Surface

- **Blocked on**: a concrete use case for network or volume management over the call protocol. Dev containers use the default bridge network; hosted services declare networks/volumes in `docker compose`.
- **Priority**: low
- **Full file**: [OQ-48](questions/048-network-and-volume-operation-surface.md)

### OQ-49: Image Build (buildkit) Scope

- **Blocked on**: a concrete use case for building images over the call protocol. The current use cases pull pre-built images, not build them.
- **Priority**: low
- **Full file**: [OQ-49](questions/049-image-build-buildkit-scope.md)

### OQ-50: Docker System Events Subscription

- **Blocked on**: a concrete use case for prompt stale-ownership cleanup. The base model (ADR-060 §4) tolerates inert stale entries; the events subscription is a refinement.
- **Priority**: low
- **Full file**: [OQ-50](questions/050-docker-system-events-subscription.md)

### OQ-51: Container Create Options Surface

- **Blocked on**: v1 implementation — the `create` input JSON Schema is finalized when `register_docker_ops` is written and tested against bollard's `Config` struct. An architectural decision (ADR-060 §5), not a deferral past implementation.
- **Priority**: medium
- **Full file**: [OQ-51](questions/051-container-create-options-surface.md)


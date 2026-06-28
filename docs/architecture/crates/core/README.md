---
status: draft
last_updated: 2026-06-27
---

# alknet-core

Core library for ALPN-based protocol dispatch. Every handler crate depends on alknet-core.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [core-types.md](core-types.md) | draft | ProtocolHandler trait, HandlerError, Connection, BiStream, StreamError |
| [endpoint.md](endpoint.md) | draft | ALPN router, HandlerRegistry, accept loop, graceful shutdown |
| [auth.md](auth.md) | draft | AuthContext, Identity, IdentityProvider, AuthToken, resolution flow, PeerEntry, CredentialStore |
| [config.md](config.md) | draft | StaticConfig, DynamicConfig, ArcSwap, ConfigReloadHandle, AuthPolicy.peers |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [001](../../decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Core architectural model |
| [002](../../decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | The trait every handler implements |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-core's position in the crate graph |
| [004](../../decisions/004-auth-as-shared-core.md) | Auth as Shared Core | IdentityProvider in core |
| [006](../../decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention | ALPN format, one-ALPN-per-connection |
| [007](../../decisions/007-bistream-type-definition.md) | BiStream Type Definition | Connection, BiStream trait, SendStream, RecvStream |
| [009](../../decisions/009-one-way-door-decision-framework.md) | One-Way Door Framework | Decision classification |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Endpoint, HandlerRegistry, accept loop |
| [011](../../decisions/011-authcontext-structure.md) | AuthContext Structure | AuthContext fields and resolution flow |
| [015](../../decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | Per-request identity on OperationContext; admin scope for config reload |
| [030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | PeerEntry and Identity.id Decoupling | `authorized_fingerprints` → `peers: Vec<PeerEntry>`; `Identity.id` = `peer_id` (stable) |
| [031](../../decisions/031-credentialstore-repo-trait.md) | CredentialStore Repo Trait | Second repo trait in core; `InMemoryCredentialStore` default adapter |
| [033](../../decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Storage Boundary and Repo/Adapter Pattern | Core defines traits + in-memory defaults; persistence adapters are separate crates |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-04 | Dynamic handler registration | resolved (start static) | HandlerRegistry is immutable at startup |
| OQ-05 | Multi-connectivity endpoint | resolved (quinn + iroh) | AlknetEndpoint supports both, both feature-gated |
| OQ-11 | Handler-level auth resolution observability | resolved | Handlers store resolved identity on Connection; two identity scopes (connection-level for observability, per-request for ACL) |
| OQ-33 | PeerId — logical id vs crypto identity | resolved by ADR-030 | `PeerId` = `Identity.id` = `PeerEntry.peer_id` (stable across key rotation) |
| OQ-34 | Persistent peer registry (storage boundary) | resolved by ADR-030+031+033 | Core defines repo traits + in-memory defaults; persistence adapters are separate crates |
| OQ-35 | ~~API key asymmetry~~ | dissolved | `PeerEntry` supports multiple credential paths; `ApiKeyEntry` is for tokens that ARE the identity |
| OQ-36 | Concrete persistence adapter shapes | resolved by ADR-035 | Read-sync / write-async split (`IdentityStore`); SQLite adapter caches in memory, honker NOTIFY for no-restart cache invalidation; `alknet-store-sqlite` crate |
| OQ-37 | X.509 outgoing-only case | resolved by ADR-034 | Three remote roles (public X.509 endpoint, transport relay, hub); `PeerEntry` asymmetry correct; client-side verifier by `PeerEntry` presence (CA vs fingerprint pin) |

## Key Design Principles

1. **One trait, one dispatch point**: `ProtocolHandler` is the only abstraction handlers implement. No StreamInterface/MessageInterface split.
2. **ALPN does the routing**: The endpoint dispatches by ALPN string. No byte-peeking, no ListenerConfig enum.
3. **Handlers own their wire format**: Each handler manages its own protocol parsing. alknet-core provides the Connection, not the framing.
4. **Auth is hybrid**: The endpoint provides what it can (TLS-level auth). Handlers complete what they need. AuthContext may be partial.
5. **WASM door preserved**: BiStream is a trait, Connection is an opaque type. Core types don't assume tokio or quinn in public APIs.
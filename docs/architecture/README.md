---
status: draft
last_updated: 2026-06-28
---

# Alknet Architecture

## Current State

**Pre-implementation of the storage/repo pattern.** The project has completed a pivot from a three-layer model to an ALPN-as-service model. The greenfield workspace contains `alknet-vault` (stable — implementation complete and verified, local-only by construction per ADR-025, HD-derivation key model per ADR-026) and research/reference material. Foundational ADRs (001–035) are in place, with the call crate implemented and reviewed.

The storage and auth strategy research (`docs/research/alknet-storage-strategy/findings.md`) surfaced the repo/adapter pattern as the answer to cross-node state (peer identity, credentials). This has now landed as four ADRs:

- **ADR-030** (PeerEntry and Identity.id decoupling): `authorized_fingerprints: HashSet<String>` → `peers: Vec<PeerEntry>`; `Identity.id` becomes the stable `peer_id` (not the fingerprint); key rotation changes the fingerprint, not the identity. Supersedes ADR-029's v1 UUID source (the one-way door — `PeerId` is logical, not crypto — is preserved; the source changes from UUID to `Identity.id` from `PeerEntry`). Resolves OQ-33 and the storage-boundary half of OQ-34.
- **ADR-031** (CredentialStore repo trait): the second repo trait in core (alongside `IdentityProvider`), with `InMemoryCredentialStore` default adapter. Establishes the credential-persistence abstraction.
- **ADR-032** (Forwarded-for identity): `forwarded_for` field on `call.requested` and `OperationContext`; metadata only — `AccessControl::check` never reads it; the `from_call` handler populates it. Wire-format one-way door, included with the ADR-029 migration window.
- **ADR-033** (Storage boundary and repo/adapter pattern): core defines repo traits + in-memory defaults; persistence adapters are separate crates; the assembly layer wires the adapter. Resolves OQ-34's storage-boundary question. Concrete adapter shapes now committed by ADR-035 (was OQ-36).

The alknet-call crate is **implemented and reviewed** — both the server-side core and the client/adapter surface (207 lib + 2 integration tests passing). The alknet-core and alknet-call crate specs are in draft; the alknet-vault crate specs are stable.

**alknet-http specs drafted.** The alknet-http crate (HTTP interface — `h2`/`http/1.1`/`h3` server + `from_openapi`/`to_openapi`/`from_mcp`/`to_mcp` adapters) now has architecture specs: [crates/http/](crates/http/) (overview, http-server, http-adapters, http-mcp, webtransport) and four new ADRs — [ADR-036](decisions/036-http-to-call-operation-mapping.md) (HTTP-to-call mapping), [ADR-037](decisions/037-mcp-stdio-transport-exclusion.md) (MCP stdio exclusion), [ADR-038](decisions/038-http3-and-webtransport-as-first-class.md) (HTTP/3 + WebTransport as first-class, correcting the Phase 0 deferral framing), [ADR-039](decisions/039-http-server-and-client-host-colocated.md) (HTTP server + client host colocated in one crate). ADR-003 Amendment 1 clarifies that `alknet-call` is a protocol-foundation crate (the `alknet-http` → `alknet-call` dependency edge). The specs are in draft; implementation has not started. Three open questions carried: OQ-38 (WebTransport relay-as-proxy scope), OQ-39 (`to_openapi` published-spec versioning), OQ-40 (reqwest client config).

**Next step**: The storage/repo-pattern ADRs (030–033) are accepted and amend the core and call specs. The next implementation phase is the ADR-029 migration (peer-keyed overlays, `PeerRef` routing, retire `remote_safe`/`trusted_peer`) with the ADR-030 `PeerEntry` change and the ADR-032 `forwarded_for` field folded in — the `OperationContext`, `from_call` handler, and `AuthPolicy` are all under edit, making this the cheapest window. After that: alknet-http implementation (specs drafted, ADRs 036–038 proposed), which consumes the `CredentialStore` trait and the `OperationAdapter` contract. The alknet-ssh crate (the other post-core crate, specced in parallel) proceeds independently — it depends on `alknet-core`, not `alknet-call`.

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph, shared types, design principles |
| [open-questions.md](open-questions.md) | draft | Centralized OQ tracker with door-type classifications |
| [crates/core/README.md](crates/core/README.md) | draft | alknet-core crate index |
| [crates/core/core-types.md](crates/core/core-types.md) | draft | ProtocolHandler, HandlerError, Connection, BiStream, StreamError |
| [crates/core/endpoint.md](crates/core/endpoint.md) | draft | ALPN router, HandlerRegistry, accept loop, shutdown |
| [crates/core/auth.md](crates/core/auth.md) | draft | AuthContext, Identity, IdentityProvider, AuthToken, resolution flow |
| [crates/core/config.md](crates/core/config.md) | draft | StaticConfig, DynamicConfig, ArcSwap, ConfigReloadHandle |
| [crates/call/README.md](crates/call/README.md) | draft | alknet-call crate index |
| [crates/call/call-protocol.md](crates/call/call-protocol.md) | draft | CallAdapter, EventEnvelope framing, stream model, PendingRequestMap, bidirectional calls, streaming subscribe example |
| [crates/call/operation-registry.md](crates/call/operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, capability injection, service discovery, irpc integration |
| [crates/call/client-and-adapters.md](crates/call/client-and-adapters.md) | draft | CallClient (outbound connection opener), from_call / from_jsonschema, OperationAdapter trait, adapter location map, no-env-vars invariant, exchange-of-operations pattern |
| [crates/http/README.md](crates/http/README.md) | draft | alknet-http crate index |
| [crates/http/overview.md](crates/http/overview.md) | draft | Crate purpose, two roles (server + client host), dependencies, adapter location map |
| [crates/http/http-server.md](crates/http/http-server.md) | draft | HttpAdapter for h2/http1.1, axum over QUIC, Bearer auth, stealth, /healthz |
| [crates/http/http-adapters.md](crates/http/http-adapters.md) | draft | from_openapi (reqwest) and to_openapi (projection); no-env-vars injection point |
| [crates/http/http-mcp.md](crates/http/http-mcp.md) | draft | from_mcp / to_mcp (feature-gated), streamable-HTTP-only, stdio exclusion |
| [crates/http/webtransport.md](crates/http/webtransport.md) | draft | h3/WebTransport handler — the browser streaming path |
| [crates/vault/README.md](crates/vault/README.md) | stable | alknet-vault crate index |
| [crates/vault/mnemonic-derivation.md](crates/vault/mnemonic-derivation.md) | stable | BIP39, SLIP-0010, BIP-0032, derivation paths, key types |
| [crates/vault/encryption.md](crates/vault/encryption.md) | stable | AES-256-GCM, EncryptedData, key versioning, salt (Phase B reserved) |
| [crates/vault/service.md](crates/vault/service.md) | stable | VaultServiceHandle lifecycle, direct dispatch, cache, error model |
| [crates/vault/protocol.md](crates/vault/protocol.md) | stable | DerivedKey redaction, KeyType, serialization behavior |

## ADR Table

| ADR | Title | Status |
|-----|-------|--------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Accepted |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | Accepted |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | Accepted |
| [004](decisions/004-auth-as-shared-core.md) | Auth as Shared Core (IdentityProvider) | Accepted |
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | Accepted |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | Accepted |
| [007](decisions/007-bistream-type-definition.md) | BiStream Type Definition | Accepted |
| [008](decisions/008-secret-service-integration.md) | Vault Integration Point | Accepted |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Accepted |
| [010](decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Accepted |
| [011](decisions/011-authcontext-structure.md) | AuthContext Structure and Resolution Flow | Accepted |
| [012](decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Accepted |
| [013](decisions/013-rust-canonical-implementation.md) | Rust as Canonical Implementation Language | Accepted |
| [014](decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Accepted |
| [015](decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | Accepted |
| [016](decisions/016-abort-cascade-for-nested-calls.md) | Abort Cascade for Nested Calls | Accepted |
| [017](decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | Accepted |
| [018](decisions/018-vault-standalone-crate.md) | Vault as Standalone Crate | Accepted |
| [019](decisions/019-vault-assembly-layer-only.md) | Vault Assembly-Layer-Only Access | Accepted |
| [020](decisions/020-hd-derivation-for-encryption-keys.md) | HD Derivation for Encryption Keys | Accepted |
| [021](decisions/021-key-rotation-via-version-indexed-paths.md) | Key Rotation via Version-Indexed Paths | Accepted |
| [022](decisions/022-handler-registration-provenance-and-composition-authority.md) | Handler Registration, Provenance, and Composition Authority | Accepted |
| [023](decisions/023-operation-error-schemas.md) | Operation Error Schemas | Accepted |
| [024](decisions/024-operation-registry-layering.md) | Operation Registry Layering | Accepted |
| [025](decisions/025-vault-local-only-dispatch.md) | Vault Local-Only Dispatch | Accepted |
| [026](decisions/026-vault-key-model-hd-derivation.md) | Vault Key Model — HD Derivation | Accepted |
| [027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | TLS Identity Redesign — ACME + RawKey Decoupling | Accepted |
| [028](decisions/028-callclient-peer-scoped-registry-filtering.md) | Peer-Scoped Registry Filtering for CallClient Inbound Dispatch | ~~Accepted~~ → **Superseded** by ADR-029 |
| [029](decisions/029-peer-graph-routing-model.md) | Peer-Graph Routing Model for alknet-call Composition | Accepted (Assumption 1's `PeerId` source superseded by ADR-030) |
| [030](decisions/030-peerentry-and-identity-id-decoupling.md) | PeerEntry and Identity.id Decoupling | Accepted (supersedes ADR-029 Assumption 1's UUID source) |
| [031](decisions/031-credentialstore-repo-trait.md) | CredentialStore Repo Trait | Accepted |
| [032](decisions/032-forwarded-for-identity.md) | Forwarded-For Identity (Metadata, Not Authority) | Accepted |
| [033](decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Storage Boundary and Repo/Adapter Pattern | Accepted |
| [034](decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Outgoing-Only X.509 and the Three Peer Roles | Accepted |
| [035](decisions/035-concrete-persistence-adapter-shapes.md) | Concrete Persistence Adapter Shapes — Read/Write Split, honker+SQLite | Accepted |
| [036](decisions/036-http-to-call-operation-mapping.md) | HTTP-to-Call Operation Mapping | Proposed |
| [037](decisions/037-mcp-stdio-transport-exclusion.md) | MCP Stdio Transport Exclusion | Proposed |
| [038](decisions/038-http3-and-webtransport-as-first-class.md) | HTTP/3 and WebTransport as First-Class HTTP Transports | Proposed |
| [039](decisions/039-http-server-and-client-host-colocated.md) | HTTP Server and Client Host Colocated in alknet-http | Proposed |

## Open Questions

See [open-questions.md](open-questions.md) for the full tracker.

**Resolved one-way doors:**
- **OQ-01**: BiStream type — trait with Connection parameter (ADR-007)
- **OQ-02**: AuthContext timing — hybrid model (ADR-004)
- **OQ-03**: ALPN naming — `alknet/` prefix, no version (ADR-006)
- **OQ-05**: Multi-connectivity endpoint — quinn + iroh, both feature-gated (ADR-010)
- **OQ-06**: ALPN per connection, not per stream (ADR-006)
- **OQ-08**: Vault integration — CLI-embedded, assembly-layer only (ADR-008, ADR-014)
- **OQ-16**: Safe vault operations for call protocol exposure — none for now (ADR-014)
- **OQ-18**: Privilege model — `internal` = authority switch, External/Internal visibility, handler identity + scoped env (ADR-015)
- **OQ-17**: Abort cascade — `call.aborted` cascades to descendants; default `abort-dependents`, `continue-running` opt-in (ADR-016)
- **OQ-15**: Call protocol client and adapter contract — `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction (ADR-017)

**Resolved two-way doors:**
- **OQ-04**: Dynamic handler registration — static at startup (ADR-010); scoped to the `HandlerRegistry` (ALPN-level) by ADR-024, which governs `OperationRegistry` mutability separately
- **OQ-07**: Call protocol scope — bidirectional streams, EventEnvelope, ID-based correlation (ADR-012)
- **OQ-11**: Handler-level auth resolution observability — handlers store resolved identity on Connection (Option B); two identity scopes: connection-level (observability) and per-request (ACL)
- **OQ-12**: TLS identity provisioning — two use cases: RFC 7250 raw keys (default, P2P) and X.509 certs (domain-hosted, browsers). ACME designed in ADR-027; RawKey decoupled from iroh feature.
- **OQ-13**: Operation path format — `/{service}/{op}` is the correct design for alknet-call, not a simplification
- **OQ-14**: Batch operation semantics — multiple correlated `call.requested` events is the correct protocol design, not a simplification
- **OQ-19**: Session-scoped registries — agent-written operations via `OperationEnv` trait layering; protocol doesn't need changes; `OperationEnv` must remain a trait. Generalized by ADR-024 to cover connection-scoped overlays as well.
- **OQ-20**: Encryption key derivation — HD derivation from BIP39 seed, not PBKDF2; salt field unused in v2 (wire-format compat) (ADR-020)
- **OQ-21**: Remote vault access — resolved (ADR-025): vault is local-only by construction; remote access requires a separate vault-server crate with its own ADR
- **OQ-22**: Key rotation — version-indexed derivation paths; `rotate` method re-encrypts (ADR-021)
- **OQ-23**: Handler identity registration path — registration bundle with provenance, composition authority, scoped env, capabilities (ADR-022)
- **OQ-24**: Operation error schemas — declared domain errors with typed `details` payload; adapter fidelity for `from_openapi`/`to_openapi` (ADR-023)

**Resolved by the storage/repo-pattern ADRs (ADR-030–033):**
- **OQ-33**: ~~PeerId stability~~ — **resolved by ADR-030** (logical id; source is `Identity.id` = `PeerEntry.peer_id`, stable across key rotation; UUID workaround removed)
- **OQ-34**: ~~Persistent peer registry~~ — **resolved by ADR-030 + ADR-031 + ADR-033** (storage boundary: core defines repo traits + in-memory defaults; persistence adapters are separate crates)
- **OQ-35**: ~~API key asymmetry~~ — **dissolved** (the framing was wrong; `PeerEntry` supports multiple credential paths)

**Resolved by the call-completion / ADR-029 work:**
- **OQ-27**: ~~`from_call` re-import trigger~~ — **resolved** (auto-re-import on connection establishment; `refresh()` is a feature addition)
- **OQ-28**: ~~`from_call` namespace collision~~ — **resolved** (same-peer collision = error; cross-peer dissolved by ADR-029)
- **OQ-29**: ~~CallClient TLS client-auth~~ — **resolved** (wire quinn client-auth; key-type-aware server cert verification; fingerprint normalization to `ed25519:` across quinn/iroh)
- **OQ-30**: ~~`PeerRef::Any` routing policy~~ — **resolved** (insertion-order first-match; richer routing is a feature extension)
- **OQ-31**: ~~`services/list-peers` re-export semantics~~ — **resolved** (opt-in `services/list-peers`; `services/list` is "own ops only")

**Open (feature extensions, not blocking):**
- **OQ-32**: Multi-hop federation — the one-hop model is the architectural commitment; multi-hop is a feature extension that doesn't break downstream
- **OQ-36**: ~~Concrete persistence adapter shapes~~ — **resolved by ADR-035** (read-sync / write-async / honker-NOTIFY cache invalidation; `alknet-store-sqlite` crate; `IdentityStore` write trait; `CredentialStore::put`/`delete` async)
- **OQ-37**: ~~X.509 outgoing-only case~~ — **resolved by ADR-034** (three remote roles named: public X.509 endpoint, transport relay, hub; `PeerEntry` asymmetry is correct; client-side verifier selection by `PeerEntry` presence)
- **OQ-38**: WebTransport relay-as-proxy scope — does the proxy live in `alknet-http` or a separate relay crate? (scope question, not deferral; ADR-038 brought h3 into scope)
- **OQ-39**: `to_openapi` published-spec versioning — versioning strategy for generated OpenAPI specs (one-way after first publication)
- **OQ-40**: reqwest client config and connection pooling — two-way-door config shape for the outbound HTTP client

**Deferred (not active):**
- **OQ-09**: WASM target boundaries — design constraint, not deliverable
- **OQ-10**: Git adapter scope — start with smart protocol, add ERC721 later

## Document Lifecycle

| Status | Meaning | Transitions |
|--------|---------|-------------|
| `draft` | Under active development. May change significantly. | → `reviewed` when open questions are resolved |
| `reviewed` | Architecture is final. Implementation may begin. Changes require review. | → `stable` when implementation is complete and verified |
| `stable` | Locked. Changes require review and may warrant an ADR. | → `deprecated` when superseded |
| `deprecated` | Superseded. Kept for reference. | Removed when no longer referenced |

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- Cleanup plan: `docs/research/pivot/cleanup-plan.md`
- SDD process: `docs/sdd_process.md`
- Reference implementation: `/workspace/@alkdev/alknet-main/`
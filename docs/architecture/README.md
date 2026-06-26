---
status: draft
last_updated: 2026-06-26
---

# Alknet Architecture

## Current State

**Pre-implementation.** The project has completed a pivot from a three-layer model to an ALPN-as-service model. The greenfield workspace contains only `alknet-vault` (stable — implementation complete and verified, local-only by construction per ADR-025, HD-derivation key model per ADR-026) and research/reference material. Foundational ADRs (001–028) are in place. ADR-024 resolves the registry mutability question and the `OperationContext.env` type identity crisis by layering the registry by trust boundary. ADR-025 drops irpc from the vault, making it local-only by construction. ADR-026 records the HD-derivation key model as a foundational decision. Review #003 (type/API surface completeness) resolved: `DerivedKey` derive contradiction, `encrypt` prose, return-type divergence, RwLock contradiction, drift table gaps, ADR-022 stale sketches, `Capabilities`/`SessionOverlaySource`/`CallConnection`/`CachedKey` definitions, `CompositeOperationEnv` dispatch contract, `with_local` signature, payload schemas, timeout propagation, and request ID generation. The alknet-core and alknet-call crate specs are in draft; the alknet-vault crate specs are stable.

The alknet-call crate is **implemented and reviewed** — both the server-side core and the client/adapter surface. The server-side core (`CallAdapter`, `CallConnection` dispatch loop, wire framing, pending map, abort cascade, operation registry, service discovery) and the client/adapter surface (`CallClient`, `from_call`, `from_jsonschema`, `OperationAdapter` trait, shared `Dispatcher`) are implemented and tested (207 lib + 2 integration tests passing). The call-completion gap analysis (`docs/research/alknet-call-completion/gap-analysis.md`) identified the missing client/adapter surface specced in ADR-017 plus four decisions (DC-1..4); all are resolved — DC-1 by ADR-028 (peer-scoped default-deny filtering), DC-2/3/4 as two-way-door defaults in `client-and-adapters.md` (OQ-25..28). A post-implementation review (`tasks/call/review-completion.md`) confirmed spec conformance; the one spec-sketch omission (`connect()`'s `ClientError` return type) was the intended illustrative-sketch gap, now filled in. A TLS client-auth gap surfaced during implementation is tracked as OQ-29.

**Next step**: The alknet-call crate is ready for downstream consumers (alknet-http's `OperationAdapter` implementations, the container-service/runner pattern, alknet-agent, alknet-napi). The remaining open questions (OQ-25..29) are all two-way-door shape/defaults, not blockers. The next crate phase is alknet-http (Phase 0 findings in `docs/research/alknet-http/`).

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
| [028](decisions/028-callclient-peer-scoped-registry-filtering.md) | Peer-Scoped Registry Filtering for CallClient Inbound Dispatch | Accepted |

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

**Open (two-way-door remainders from alknet-call completion):**
- **OQ-25**: Remote-safe marking shape — existence of default-deny `CallClient` filtering locked by ADR-028; shape (`remote_safe: bool` v1 vs per-peer allowlist) open
- **OQ-26**: `OperationAdapter` error type — `import()` returns `Result<_, AdapterError>`; variants decided in implementation
- **OQ-27**: `from_call` re-import trigger — v1 default auto-on-reconnect; explicit `refresh()` additive
- **OQ-28**: `from_call` namespace collision — v1 default error-on-collision (no prefix by default)
- **OQ-29**: `CallClient` TLS client-auth + remote-identity verification — v1 connects with `with_no_client_auth()` and `AcceptAnyServerCertVerifier`; wiring RawKey client-auth is additive (the no-env-vars invariant is unaffected — `auth_token` flows through the call-protocol payload, not TLS)

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
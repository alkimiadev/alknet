---
status: draft
last_updated: 2026-06-17
---

# Open Questions

Questions are organized by theme. Each question has a stable OQ-ID for cross-referencing from spec documents.

Door type classifications follow ADR-009:
- **One-way door**: Reversal requires rewriting significant code or permanently closes a capability. Requires ADR before implementation.
- **Two-way door**: Reversal is cheap or additive. Can be decided during implementation.

## Theme: Core Types

### OQ-01: BiStream Type Definition

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: BiStream is a trait (`AsyncRead + AsyncWrite + Send + Unpin`). Handlers receive a `Connection` (not a single BiStream). This preserves the WASM door — browser clients can implement BiStream over WebTransport streams. See ADR-007.
- **Cross-references**: ADR-002, ADR-007, ADR-009

### OQ-02: AuthContext Resolution Timing

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: Hybrid model (Option C) — endpoint resolves what it can (e.g., TLS client certificate), handler resolves what it must (e.g., AuthToken in first frame). AuthContext may be partial when `handle()` is called. See ADR-004.
- **Cross-references**: ADR-002, ADR-004

## Theme: ALPN and Routing

### OQ-03: ALPN String Naming Convention

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: Custom ALPNs use `alknet/<name>` prefix (no version), standard ALPNs use IANA strings. No version negotiation initially. See ADR-006.
- **Cross-references**: ADR-001, ADR-006

### OQ-04: Dynamic Handler Registration at Runtime vs Static at Startup

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Static registration at startup. `HandlerRegistry` is immutable after construction. ALPN strings in the TLS `ServerConfig` are derived from the registry at startup — adding a handler at runtime requires rebuilding the TLS config. The `ArcSwap<HandlerRegistry>` pattern can be applied later if needed (two-way door). See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [endpoint.md](crates/core/endpoint.md)

## Theme: Transport and Endpoint

### OQ-05: Multi-Connectivity Endpoint

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: `AlknetEndpoint` supports both `quinn::Endpoint` (public QUIC+TLS) and `iroh::Endpoint` (P2P relay-assisted) simultaneously, both optional and feature-gated. Both produce QUIC connections that dispatch through the same `HandlerRegistry` by ALPN string. These are not interchangeable transports — they serve fundamentally different deployment contexts (public IP vs NAT traversal). TCP is not an endpoint concern — bare TCP SSH is handled by the SSH handler directly. See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [endpoint.md](crates/core/endpoint.md)

### OQ-06: Server-Side ALPN vs Client-Side ALPN

- **Origin**: ADR-001
- **Status**: resolved
- **Door type**: One-way
- **Priority**: low
- **Resolution**: One ALPN per connection. Clients open one QUIC connection per ALPN. QUIC connections are cheap (multiplexed over the same UDP flow). See ADR-006.
- **Cross-references**: ADR-001, ADR-006

## Theme: Call Protocol

### OQ-07: Call Protocol Scope Within a Connection

- **Origin**: ADR-005
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: The call protocol uses bidirectional QUIC streams with EventEnvelope framing and ID-based correlation via PendingRequestMap. The protocol is stream-agnostic — the client can open one stream per operation, multiplex on one stream, or any mix. Correlation is by request ID, not by stream. Both sides can initiate calls. One `alknet/call` connection gives access to the full operation registry (call, subscribe, batch, schema). No multiplexing layer is needed inside the connection. See ADR-012.
- **Cross-references**: ADR-005, ADR-012

## Theme: Security

### OQ-08: Vault Integration Point

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: CLI-embedded with call protocol exposure. The CLI binary instantiates `VaultServiceHandle` locally and registers vault operations in the call protocol's operation registry. alknet-vault has no ALPN and no alknet-core dependency. Key derivation is local-only; only public key material crosses the network via `alknet/call`. The vault is a capability source — derived keys and decrypted credentials are injected into operation contexts at the assembly layer, not passed as vault references to handlers. See ADR-008.
- **Cross-references**: ADR-003, ADR-005, ADR-008

## Deferred Questions

These questions are acknowledged but not active. They will be promoted to open when their crate is being specified.

### OQ-09: WASM Target Boundaries

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: One-way (when applicable)
- **Priority**: low
- **Resolution**: Not an active question — WASM compatibility is a design constraint (see ADR-009, overview.md design principles), not a deliverable. Specific WASM targeting decisions will be made when individual crates are implemented. The BiStream trait decision (ADR-007) has already preserved the most important WASM door.
- **Cross-references**: ADR-007, ADR-009

### OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Origin**: [overview.md](overview.md)
- **Status**: deferred
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Deferred per the cleanup plan. Start with git smart protocol over QUIC streams. ERC721 integration and full server capabilities are additive. Resolve when speccing alknet-git.
- **Cross-references**: ADR-001

## Theme: alknet-core

### OQ-11: Handler-Level Auth Resolution Observability

- **Origin**: [auth.md](crates/core/auth.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: When a handler resolves identity inside `handle()`, should the resolved `Identity` be stored somewhere for observability (e.g., connection logging), or is the handler's local variable sufficient? Options: (A) handlers return the resolved identity from `handle()`, (B) handlers call a method on Connection to set identity, (C) handlers log locally and the resolved identity stays local. Two-way door — can be decided during implementation.
- **Cross-references**: ADR-004, ADR-011

### OQ-12: TLS Identity Provisioning in AlknetEndpoint

- **Origin**: [endpoint.md](crates/core/endpoint.md), [config.md](crates/core/config.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: TLS identity in alknet has two distinct use cases, not one:

  **Use case 1 — P2P / key-based identity (default for most alknet nodes):** RFC 7250 raw Ed25519 public keys. No domain, no CA, no cert renewal. The Ed25519 public key IS the node's identity. This is the same model iroh uses with its `NodeId`. It works natively with SSH auth (same key type) and git (SSH key-based auth). `TlsIdentity::RawKey` in `StaticConfig` covers this. This is the primary identity mode for alknet-native clients — most nodes will use this.

  **Use case 2 — Domain-hosted services (relays, public-facing nodes):** X.509 certificates with domain names. Required for browser/WebTransport clients, which don't support RFC 7250. This has two sub-cases:
  - **Manual**: Provide cert/key file paths via `TlsIdentity::X509`. Already specified in `StaticConfig`.
  - **ACME auto-provisioning**: Let's Encrypt via rustls-acme. The reverse-proxy project (`/workspace/@alkdev/reverse-proxy`) demonstrates the complete pattern: per-listener ACME state machine, `ResolvesServerCertAcme` rustls integration, TLS-ALPN-01 challenge handling, automatic renewal. This is a proven, solved implementation pattern — not speculative future work. It will be adapted to alknet's `AlknetEndpoint` context when domain-hosted nodes need it.

  **Browser constraint**: Browsers require X.509 and don't support RFC 7250. For browser/WebTransport clients, domain-hosted nodes with X.509 certs are mandatory. All other clients (SSH, git, alknet-native) work with raw keys by default.

  The `TlsIdentity` enum in `StaticConfig` already captures all three modes (`X509`, `RawKey`, `SelfSigned`). ACME auto-provisioning is additive — it produces an X.509 cert at runtime rather than from files, and fits naturally as an additional `TlsIdentity` variant or as a `rustls::ResolvesServerCert` implementation behind the existing `X509` path.
- **Cross-references**: ADR-010, [config.md](crates/core/config.md), [endpoint.md](crates/core/endpoint.md)

### OQ-13: Operation Path Format and Routing Scope

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: alknet-call uses `/{service}/{op}` (e.g., `/vault/derive`, `/services/list`). This is the correct format for the alknet-call crate — it is not a "Phase 1 simplification" but the right design for this architecture. The `/{node}/{service}/{op}` pattern from the reference implementation served a head/worker routing model that is a separate architectural concern. Remote dispatch (federation / node-level routing) would be a different mechanism at a different layer, not a prefix added to alknet-call's operation paths. If remote dispatch is ever needed, it would be addressed by a separate crate or a routing layer above the operation registry, not by changing alknet-call's path format. Two-way door — the path format can be extended later if needed, but `/{service}/{op}` is the correct design now.
- **Cross-references**: ADR-005, ADR-012

### OQ-14: Batch Operation Semantics

- **Origin**: [call-protocol.md](crates/call/call-protocol.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Batch is a client-side pattern — multiple `call.requested` events with correlated IDs, responses arrive independently. This is the correct protocol design, not a simplification to be "upgraded" later. QUIC's stream multiplexing already provides the concurrency and ordering guarantees that batch would need. Batch-specific event types (e.g., `batch.requested`, `batch.responded`) would add protocol complexity without clear benefit over sending multiple `call.requested` events. If a compelling use case for atomic batch semantics emerges, it can be added as a new event type without breaking existing clients. Two-way door.
- **Cross-references**: ADR-012

## Theme: alknet-call

### OQ-15: Call Protocol Client and Adapter Contract

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md), ADR-013
- **Status**: open
- **Door type**: One-way
- **Priority**: high
- **Resolution**: alknet-call currently specifies only the server side (CallAdapter receives connections and dispatches to the operation registry). A call protocol client is needed for: (1) alknet-napi to expose remote invocation to Node.js, (2) alknet-agent to dispatch tool calls (call, batch, search, schema) to remote nodes, (3) the `from_call` adapter pattern that creates operations whose handlers invoke remote services. The adapter contract (from_openapi, from_mcp, from_call, to_openapi, to_mcp) determines how external specifications and protocols compose with the operation registry. These traits belong in alknet-call because they define how operations are produced and consumed — the same contract that enables an agent to register call/batch/search/schema as tools also enables from_openapi to register HTTP-backed operations. The TypeScript `@alkdev/operations` library demonstrated these patterns; the Rust implementation defines the canonical traits (ADR-013). Two-way door for the specific trait signatures, one-way door for the architectural commitment that the adapter contract lives in alknet-call.
- **Cross-references**: ADR-005, ADR-013, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)
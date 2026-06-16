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

### OQ-05: Multi-Transport Endpoint

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Start with quinn (QUIC over UDP). `AlknetEndpoint` uses `quinn::Endpoint` directly. The endpoint can be made transport-agnostic later by abstracting the connection accept loop behind a trait. iroh connectivity produces QUIC connections that can feed into the same ALPN router. `SendStream`/`RecvStream` are concrete wrappers over quinn types — can become enum dispatch if multi-transport is needed. See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [core-types.md](crates/core/core-types.md)

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
- **Status**: open
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: (deferred to implementation) Start with one stream per operation/request in the call protocol. Multiplexing within a stream can be added later if needed without breaking existing clients. Resolve when speccing alknet-call.
- **Cross-references**: ADR-005

## Theme: Security

### OQ-08: Vault Integration Point

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: medium
- **Resolution**: CLI-embedded with call protocol exposure. The CLI binary instantiates `VaultServiceHandle` locally and registers vault operations in the call protocol's operation registry. alknet-vault has no ALPN and no alknet-core dependency. Key derivation is local-only; only public key material crosses the network via `alknet/call`. The vault is a capability source — derived keys and decrypted credentials are injected into operation contexts at the assembly layer, not passed as vault references to handlers. See ADR-008.
- **Cross-references**: ADR-003, ADR-005, ADR-008

## Deferred Questions

These questions are acknowledged but not active. They will be promoted to open when their crate becomes a Phase 1 implementation target.

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

### OQ-12: TLS Certificate Provisioning in AlknetEndpoint

- **Origin**: [endpoint.md](crates/core/endpoint.md), [config.md](crates/core/config.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: Start with file paths in StaticConfig (option A). The CLI binary provides `tls_cert` and `tls_key` paths at startup. ACME auto-provisioning (option B) and external cert managers (option C) are additive — they can be added as features without changing the core StaticConfig or endpoint lifecycle. `StaticConfig` does NOT include `acme_domain` in v1; ACME will be a separate feature when implemented.
- **Cross-references**: ADR-010, [config.md](crates/core/config.md)
---
status: draft
last_updated: 2026-06-15
---

# Open Questions

Questions are organized by theme. Each question has a stable OQ-ID for cross-referencing from spec documents.

## Theme: Core Types

### OQ-01: BiStream Type Definition

- **Origin**: [overview.md](overview.md), future [crates/alknet-core/spec.md](crates/alknet-core/spec.md)
- **Status**: open
- **Priority**: high
- **Resolution**: (pending)
- **Cross-references**: ADR-002

What exactly does BiStream expose? The pivot proposal defines it as a joined `(SendStream, RecvStream)` implementing `AsyncRead + AsyncWrite`. Should it be:
- A concrete type wrapping quinn's `SendStream` + `RecvStream`?
- A trait `BiStream: AsyncRead + AsyncWrite + Send + Unpin` with a QuinnBiStream implementation?
- A type alias or newtype?

The choice affects WASM compatibility (quinn doesn't compile to WASM) and testing (mock BiStream needs to be constructible without a QUIC connection). If BiStream is a trait, WASM targets can implement it over WebTransport streams.

### OQ-02: AuthContext Resolution Timing

- **Origin**: [overview.md](overview.md), future [crates/alknet-core/spec.md](crates/alknet-core/spec.md)
- **Status**: resolved
- **Priority**: high
- **Resolution**: Hybrid model (Option C) — endpoint resolves what it can (e.g., TLS client certificate), handler resolves what it must (e.g., AuthToken in first frame). AuthContext may be partial when `handle()` is called. See ADR-002 and ADR-004.
- **Cross-references**: ADR-002, ADR-004

~~Should AuthContext be fully resolved before `handle()` is called, or should the handler participate in credential extraction?~~

Resolved: The hybrid approach. The endpoint resolves TLS-level credentials (client certificate fingerprints). Handlers that use protocol-level credentials (AuthToken, Bearer headers) extract them inside `handle()` and call `IdentityProvider` to resolve. The `AuthContext` passed to `handle()` may contain only transport-level information.

## Theme: ALPN and Routing

### OQ-03: ALPN String Naming Convention

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Priority**: medium
- **Resolution**: Custom ALPNs use `alknet/<name>` prefix (no version), standard ALPNs use IANA strings, no version negotiation initially. See ADR-006.
- **Cross-references**: ADR-001, ADR-006

~~The pivot proposal uses `alknet/ssh`, `alknet/call`, `alknet/git`, etc. Questions:~~

Resolved: See ADR-006 for the full convention. Custom ALPNs use `alknet/` prefix without version numbers. Standard ALPNs (`h2`, `http/1.1`, `h3`) use their IANA strings and route to HttpAdapter. If a protocol needs a breaking change, a new ALPN string is registered.

### OQ-04: Dynamic Handler Registration at Runtime vs Static at Startup

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: ADR-001

Can handlers be registered and deregistered while the endpoint is running, or only at startup?

The previous implementation used `ArcSwap<DynamicConfig>` for hot-reloading routing rules. The same pattern could apply to handler registration. However:
- Adding a handler at runtime requires updating the TLS ALPN advertisement (may require endpoint restart)
- Removing a handler at runtime affects in-flight connections
- The CLI assembles handlers at startup — is dynamic registration even needed?

## Theme: Transport and Endpoint

### OQ-05: Multi-Transport Endpoint

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: ADR-001

The previous implementation supported TCP, TLS, and iroh (QUIC P2P) transports. In the new model, does the endpoint:
- Accept connections on a single QUIC listener (quinn) and the ALPN handles the rest?
- Also support raw TCP listeners (for clients that don't speak QUIC)?
- Support iroh as an additional transport that produces QUIC connections?

iroh's Router pattern accepts a quinn::Endpoint and dispatches by ALPN. The alknet endpoint likely wraps a quinn::Endpoint with TLS configuration that advertises all registered ALPNs. Iroh connectivity could be an additional transport that produces the same BiStream type.

### OQ-06: Server-Side ALPN vs Client-Side ALPN

- **Origin**: ADR-001
- **Status**: resolved
- **Priority**: low
- **Resolution**: One ALPN per connection. Clients open one QUIC connection per ALPN. See ADR-006.
- **Cross-references**: ADR-001, ADR-006

~~The ADR focuses on server-side dispatch (client connects, server selects ALPN). What about the client side? When a client opens a stream to a specific ALPN on an existing connection, does it: - Open a new QUIC connection with the target ALPN? - Open a bidirectional stream on an existing connection with ALPN metadata?~~

Resolved: ALPN is negotiated per-connection, not per-stream. A client that wants multiple ALPNs opens one QUIC connection per ALPN. QUIC connections are cheap (multiplexed over the same UDP flow). See ADR-006.

## Theme: Call Protocol

### OQ-07: Call Protocol Scope Within a Connection

- **Origin**: ADR-005
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: ADR-005

If a client opens a connection with ALPN `alknet/call`, can it make multiple operations over that connection? Is each operation a separate QUIC stream, or can operations be multiplexed within a single stream?

irpc supports both request/response and streaming. The mapping to QUIC streams needs definition:
- One stream per request/response pair?
- One stream per streaming subscription?
- Or a single long-lived stream with multiplexed operations?

## Theme: Security

### OQ-08: Secret Service Integration Point

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: ADR-004

alknet-secret is standalone (no alknet-core dependency). How does the rest of the system access it?
- As an irpc service that other services call? (current implementation)
- As an ALPN handler on `alknet/secret` that only internal services use?
- As a library that alknet-core calls directly (breaking the independence)?
- As a library that the CLI embeds, exposing it via the call protocol?

The answer affects whether alknet-secret needs to know about BiStream and ProtocolHandler, or if it remains purely an irpc service that the CLI bootstraps and other services call over local irpc.

## Theme: WASM and Browser

### OQ-09: WASM Target Boundaries

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: OQ-01

Which crates need to compile to WASM?
- alknet-call client (for browser/NAPI usage) — yes
- alknet-sftp protocol core — yes (browser SFTP clients)
- alknet-git pkt-line parser — potentially
- alknet-core (BiStream, AuthContext) — only the types, not the endpoint/router

This affects the BiStream type definition (OQ-01) and dependency choices. If alknet-call's client needs to work in WASM, it can't depend on tokio or quinn directly — it needs abstracted I/O.

## Theme: Git and Distributed

### OQ-10: Git Adapter Scope — Smart Protocol Only or Full Server?

- **Origin**: [overview.md](overview.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (pending)
- **Cross-references**: ADR-001

The pivot proposal mentions Git over QUIC streams (no HTTP layer). Does the GitAdapter implement:
- Just the git smart protocol (ls-refs, fetch, receive-pack) over QUIC streams?
- A full git server with ref management, pack generation, etc.?
- The ERC721 integration for on-chain repository management?

This is deferred per the cleanup plan but worth noting as an open question.
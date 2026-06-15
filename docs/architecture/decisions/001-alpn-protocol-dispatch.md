# ADR-001: ALPN-Based Protocol Dispatch

## Status

Accepted

## Context

The previous architecture used a three-layer model: transports produced byte streams, interfaces defined how to interpret those streams (StreamInterface, MessageInterface), and OperationEnv dispatched operations through local, irpc, or remote paths. This required a ListenerConfig enum with three variants (Stream, Http, Dns), a server accept loop handling three different listener types, and a complex dispatch model that mixed concerns across layers.

Protocol detection was done by byte-peeking — the server read the first bytes of an incoming connection and guessed which protocol the client was speaking. This is fragile, limits protocol extensibility, and cannot work with encrypted transports where the payload is opaque.

ALPN (Application-Layer Protocol Negotiation) is a TLS extension where the client advertises supported protocols during the handshake and the server selects one. QUIC builds on this natively — every QUIC connection has an ALPN. This is the same pattern iroh uses: `Router` dispatches incoming QUIC connections to `ProtocolHandler` implementations based on the ALPN string. Hickory DNS registers ALPN protocols (`dot`, `doq`, `h2`, `h3`). The reverse-proxy project at `@alkdev/reverse-proxy` uses the same pattern for TLS.

The core insight: **a service IS an ALPN**. Every protocol handler registers an ALPN string on a shared QUIC+TLS endpoint. The ALPN negotiation during the handshake routes the connection to the correct handler before any application bytes are read.

## Decision

All protocol dispatch in alknet is ALPN-based. A single QUIC+TLS endpoint accepts connections, and the ALPN string selected during the handshake determines which `ProtocolHandler` receives the connection. There is no byte-peeking, no ListenerConfig enum, and no three-layer dispatch model.

The endpoint advertises the union of all registered handlers' ALPN strings. When a client connects, the TLS/QUIC handshake negotiates the ALPN. If the client's offered ALPNs and the server's advertised ALPNs have no intersection, the handshake fails — this is the correct behavior, not an error to work around.

## Consequences

**Positive:**
- Single dispatch mechanism replaces three separate listener types
- Protocol detection happens at the TLS layer, not application layer — no byte-peeking
- Adding a new protocol is registering a new ALPN string — no server code changes
- Each handler owns its entire wire format — no shared framing layer
- QUIC connections are cheap — a client that needs multiple protocols opens one connection per ALPN, all multiplexed over the same UDP flow
- Stealth mode (byte-peek protocol detection on port 443) is unnecessary — ALPN negotiation handles this cleanly
- WASM story is clean: handlers receive byte streams, protocol parsers that operate on bytes compile to WASM

**Negative:**
- ALPN is negotiated per-connection, not per-stream — a client that wants to use multiple ALPNs (e.g., SSH and call protocol) opens separate QUIC connections for each. QUIC connections are cheap (multiplexed over the same UDP flow), so this is acceptable, but it means `alknet/call` cannot serve as a multiplexer for other ALPNs within a single connection unless explicitly designed to do so (see ADR-006).
- All protocols must be registered at endpoint creation time (or use hot-reload via ArcSwap for dynamic addition)
- Custom protocols require reserving ALPN strings — we own the `alknet/` namespace
- Debugging requires knowing which ALPN was negotiated (mitigated by logging at the endpoint level)

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-002: ProtocolHandler trait
- ADR-003: Crate decomposition
- iroh reference: `docs/research/references/iroh/` (ALPN dispatch, ProtocolHandler pattern)
- Replaces the old three-layer model (StreamInterface/MessageInterface/OperationEnv)
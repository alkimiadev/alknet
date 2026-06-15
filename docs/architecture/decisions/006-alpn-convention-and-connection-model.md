# ADR-006: ALPN String Convention and Connection Model

## Status

Accepted

## Context

ADR-001 establishes ALPN-based protocol dispatch. Two questions arise:

1. **ALPN string naming**: What format do custom ALPN strings follow? Should they include version numbers? How do standard ALPNs (`h2`, `http/1.1`, `h3`) coexist with custom ones?

2. **Connection model**: ALPN is negotiated per-connection in QUIC/TLS, not per-stream. A client that wants to speak both SSH and call protocol must open two separate QUIC connections, each with its own ALPN. This is different from the claim in earlier drafts that "a single connection can carry multiple protocols via additional streams" — it cannot. However, QUIC connections are cheap (multiplexed over the same UDP flow), so opening multiple connections is acceptable.

The iroh reference project uses the same model: each `ProtocolHandler` claims an ALPN, and each incoming connection is dispatched to exactly one handler based on the negotiated ALPN.

## Decision

### ALPN String Convention

Custom ALPN strings use the `alknet/` prefix:

| ALPN | Handler | Type |
|------|---------|------|
| `alknet/ssh` | SshAdapter | Custom |
| `alknet/call` | CallAdapter | Custom |
| `alknet/git` | GitAdapter | Custom |
| `alknet/sftp` | SftpAdapter | Custom |
| `alknet/msg` | MessageAdapter | Custom |
| `alknet/http` | HttpAdapter | Custom |
| `alknet/dns` | DnsAdapter | Custom |
| `h3` | WebTransport → alknet/http | Standard (IANA) |
| `h2` | HTTP/2 → alknet/http | Standard (IANA) |
| `http/1.1` | HTTP/1.1 → alknet/http | Standard (IANA) |

Rules:
- Custom ALPNs use the format `alknet/<name>` — lowercase, no version number
- Standard ALPNs (`h2`, `http/1.1`, `h3`) use their IANA-registered strings and are handled by the HTTP adapter
- No version numbers in ALPN strings initially. If protocol compatibility breaks, a new ALPN string is registered (e.g., `alknet/call/v2`). This is simpler than version negotiation and follows the QUIC convention that ALPN mismatch means connection failure
- ALPN strings are compile-time constants in each handler's `alpn()` method — no runtime registration of new ALPN strings

### Connection Model

**One ALPN per connection.** A client that wants to use multiple ALPNs opens one QUIC connection per ALPN. All connections from the same client are multiplexed over the same UDP flow (QUIC's natural connection multiplexing), so the overhead is minimal.

This means:
- `alknet/call` is a distinct ALPN with its own connection — not a multiplexer for other ALPNs
- A client interacting with both SSH and call protocol has two QUIC connections
- Within an `alknet/call` connection, multiple QUIC streams can carry independent operations (see ADR-005)
- The endpoint logs the negotiated ALPN for each connection for observability

## Consequences

**Positive:**
- Simple model: one connection, one protocol — no multiplexing layer needed inside a connection
- ALPN strings are predictable and discoverable — `alknet/<name>` is a clear namespace
- No version negotiation complexity — incompatible versions get new ALPN strings
- QUIC connection multiplexing means multiple ALPN connections share the same UDP flow

**Negative:**
- Multiple ALPNs require multiple connections — a full-featured client might have 3-5 QUIC connections open simultaneously
- No version negotiation — an incompatible change requires a new ALPN string, which means old and new clients can coexist only if the server registers both ALPNs
- The `alknet/` namespace is owned by this project — third-party extensions need their own prefix

## References

- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- OQ-03: ALPN string naming convention (resolved by this ADR)
- OQ-06: Server-side ALPN vs client-side ALPN (resolved by this ADR)
- iroh reference: `docs/research/references/iroh/`
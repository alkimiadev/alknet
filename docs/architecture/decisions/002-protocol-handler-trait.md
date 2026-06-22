# ADR-002: ProtocolHandler Trait

## Status

Accepted

## Context

The previous architecture had two separate interface traits: `StreamInterface` (for byte-stream protocols like SSH, raw TCP) and `MessageInterface` (for message-based protocols like DNS, HTTP). This split created complexity — each interface type needed its own listener configuration, its own dispatch path, and its own framing assumptions. The `ListenerConfig` enum had three variants. The server accept loop handled three different listener types.

In practice, the distinction between "stream" and "message" protocols is artificial at the handler level. SSH starts as a byte stream but internally multiplexes channels and messages. DNS over QUIC is message-based but arrives as a stream of frames. HTTP/2 is both — bidirectional streams with message semantics. Every protocol can be modeled as "receive a byte stream, manage your own wire format."

iroh's `ProtocolHandler` trait demonstrates this: it takes a bidirectional QUIC stream and the handler is responsible for its own protocol. One trait, one dispatch point.

## Decision

A single `ProtocolHandler` trait replaces both `StreamInterface` and `MessageInterface`:

> **Note**: The signature below was revised by ADR-007. The `handle()` method
> now receives a `Connection` (not a `BiStream`) — see ADR-007 for the
> current authoritative signature. The original signature is retained here
> for historical context.

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    /// The ALPN string this handler claims (e.g. b"alknet/ssh")
    fn alpn(&self) -> &'static [u8];

    /// Handle an incoming connection (revised by ADR-007 to receive
    /// `Connection` instead of `BiStream`)
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

- `alpn()` returns a static byte string — the handler's ALPN identifier
- `handle()` receives a `Connection` (revised by ADR-007 from the original
  `BiStream`) and an `AuthContext` carrying the authenticated identity, and
  returns `HandlerError` on failure
- Every handler manages its own wire format — no shared framing, no StreamInterface/MessageInterface split
- The `ListenerConfig` enum is eliminated — ALPN advertisement configuration replaces it

**AuthContext resolution is hybrid** (see ADR-004, OQ-02 resolution): the endpoint resolves what it can before calling `handle()` (e.g., TLS client certificate fingerprint), and the handler resolves what it must inside `handle()` (e.g., AuthToken in the first frame of a call stream). The `AuthContext` passed to `handle()` may contain partial identity information — the handler is responsible for completing authentication if the endpoint didn't have enough information.

## Consequences

**Positive:**
- One trait, one dispatch point — eliminates the StreamInterface/MessageInterface split and ListenerConfig enum
- Each handler owns its wire format — no shared framing assumptions that constrain protocol design
- Adding a new protocol is implementing one trait with two methods
- Testable in isolation — give a handler a mock BiStream and AuthContext
- WASM-compatible in principle — handlers that don't need tokio runtime features compile to WASM

**Negative:**
- Every handler must implement its own framing — no shared "read a length-prefixed message" utility (mitigated: common utilities can live in alknet-core without mandating their use)
- Handlers that want message semantics must build them (mitigated: alknet-call provides this as a handler, not a mandatory layer)
- AuthContext resolution is hybrid — the endpoint resolves what it can (TLS-level auth), but handlers that need protocol-level credential extraction must do so inside handle(). This means AuthContext may be partial when handle() is called. Handlers must not assume AuthContext is fully resolved.

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-001: ALPN-based protocol dispatch
- ADR-004: Auth as shared core (IdentityProvider)
- ADR-007: BiStream type definition (revised this ADR's signature from BiStream to Connection)
- iroh ProtocolHandler pattern: `docs/research/references/iroh/`
- Replaces StreamInterface, MessageInterface, and ListenerConfig
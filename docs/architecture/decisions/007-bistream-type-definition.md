# ADR-007: BiStream Type Definition

## Status

Accepted

## Context

OQ-01 asked whether BiStream should be a concrete type wrapping quinn's `SendStream` + `RecvStream`, a trait `BiStream: AsyncRead + AsyncWrite + Send + Unpin`, or a type alias/newtype. This is a one-way door decision: if BiStream is a concrete type bound to a specific QUIC library, WASM targets and alternative transports cannot implement it. If it's a trait, the door stays open.

### iroh's pattern

iroh's `ProtocolHandler::accept` receives a `Connection`, not a stream. The handler calls `connection.accept_bi()` to get `(SendStream, RecvStream)` pairs. This means iroh's handlers own the entire connection lifecycle and can open/accept multiple streams.

### Alknet's pattern differs

Alknet's handlers are different from iroh's for two reasons:

1. **One ALPN per connection** (ADR-006). An incoming connection is already dispatched to exactly one handler by ALPN. The handler receives the connection and can manage streams however it wants.

2. **Some handlers need connection-level ownership**. SSH multiplexes channels over multiple streams within a single connection. The call protocol opens a new stream per operation. These handlers need the connection, not just a single stream.

### WASM constraint

If alknet-core defines BiStream as `quinn::SendStream + quinn::RecvStream` joined via `tokio::io::join`, then:
- WASM targets cannot implement it (quinn doesn't compile to WASM)
- WebTransport clients in browsers cannot participate as full peers
- The cost of making BiStream a trait later would require changing every handler's signature

If BiStream is a trait, WASM targets implement it over WebTransport streams. Native targets implement it over quinn streams. The cost is minimal — a trait vs a concrete type adds a small amount of indirection and trait object overhead that is negligible compared to I/O latency.

### Testing constraint

A BiStream trait allows test implementations (in-memory channels, mock streams) without requiring a running QUIC connection. A concrete quinn type requires mocking at a higher level (connection mocking) which is more complex.

## Decision

### BiStream is a trait

```rust
pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}
```

Handlers receive a `Connection` (not a single BiStream) in their `handle` method. This differs from the original ADR-002 signature and aligns with iroh's proven pattern.

### Revised ProtocolHandler signature

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    fn alpn(&self) -> &'static [u8];
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

Where `Connection` wraps a QUIC connection (or, in test contexts, a mock) and provides:

```rust
pub struct Connection {
    // Private: wraps the underlying QUIC connection or test mock
}

impl Connection {
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub fn remote_alpn(&self) -> &[u8];
    // Additional methods as needed: close, remote_addr, etc.
}
```

`SendStream` and `RecvStream` are concrete types that implement `AsyncWrite` and `AsyncRead` respectively. They wrap the underlying QUIC stream types.

### Why Connection, not BiStream, as the handler parameter

The original ADR-002 specified `handle(&self, stream: BiStream, auth: &AuthContext)`. This was modeled on the idea that a handler receives a single bidirectional stream. But:

- **SSH** needs to open/accept multiple streams (channels) on one connection
- **Call protocol** opens a new stream per operation
- **HTTP** maps requests to streams within an HTTP/2 or HTTP/3 connection
- **iroh** already uses this pattern successfully

Passing a single BiStream would force handlers that need multiple streams to somehow obtain the Connection through other means, which is awkward. Passing the Connection directly is simpler and more flexible.

Handlers that only need a single stream (simple protocols) call `connection.accept_bi().await` once and work with that stream. Handlers that need multiple streams (SSH, call) use the Connection to open/accept as needed.

### Why BiStream is still defined as a trait

Even though handlers receive a `Connection` rather than a single `BiStream`, the BiStream trait is still useful:

1. **Client-side**: A client connecting to an alknet endpoint needs a way to represent "I have a bidirectional stream to speak my protocol on." That stream should be implementable over WebTransport in WASM.
2. **Testing**: Mock BiStream implementations for unit tests.
3. **Portability**: If alknet later supports transports other than QUIC (raw TCP, iroh P2P), those transports need to produce BiStream-compatible streams.

The BiStream trait is a thin convenience — `AsyncRead + AsyncWrite + Send + Unpin` — that can be implemented by any byte transport. It does not mandate tokio or quinn.

## Consequences

**Positive:**
- WASM door stays open: browser clients can implement BiStream over WebTransport streams
- Testing is straightforward: mock BiStream implementations without QUIC infrastructure
- Handlers that need multiple streams (SSH, call) have direct access to the Connection
- Handlers that need a single stream call `accept_bi()` once — simple case stays simple
- Aligns with iroh's proven ProtocolHandler pattern
- Alternative transports (TCP, iroh P2P) can implement Connection and BiStream traits

**Negative:**
- Slight runtime overhead from trait dispatch vs concrete types (negligible compared to I/O)
- Two concepts (Connection and BiStream) instead of one (BiStream alone) — more types in alknet-core
- ADR-002's `handle` signature changes from `(BiStream, AuthContext)` to `(Connection, AuthContext)` — this is a revision to the original trait signature
- Handlers must call `accept_bi()` explicitly even for simple protocols — one additional line of code per handler

## References

- ADR-002: ProtocolHandler trait (signature revised by this ADR)
- ADR-003: Crate decomposition
- ADR-006: ALPN string convention and connection model
- OQ-01: BiStream type definition (resolved by this ADR)
- iroh ProtocolHandler pattern: `docs/research/references/iroh/iroh/`
- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
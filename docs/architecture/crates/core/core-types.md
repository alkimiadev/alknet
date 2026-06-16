---
status: draft
last_updated: 2026-06-16
---

# Core Types

ProtocolHandler, HandlerError, Connection, BiStream, SendStream, RecvStream, StreamError.

## ProtocolHandler

The central abstraction. Every handler implements one trait:

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    fn alpn(&self) -> &'static [u8];
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

- `alpn()` returns the handler's ALPN identifier as a static byte string (e.g., `b"alknet/ssh"`, `b"alknet/call"`).
- `handle()` receives a `Connection` (not a single BiStream) and an `AuthContext`. Returns `HandlerError` on failure.
- Handlers that need a single stream call `connection.accept_bi()` once. Handlers that multiplex (SSH, call) open/accept streams as needed.

See [ADR-002](../../decisions/002-protocol-handler-trait.md) and [ADR-007](../../decisions/007-bistream-type-definition.md) for rationale.

## HandlerError

Non-fatal errors within a handler's `handle()` method. The endpoint catches these, logs them, and closes the connection. Other connections are unaffected.

```rust
pub enum HandlerError {
    ConnectionClosed,
    StreamError(io::Error),
    AuthRequired,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
```

- `ConnectionClosed`: The peer closed the connection. Clean exit.
- `StreamError`: An I/O error on a stream within the connection.
- `AuthRequired`: The handler requires authentication and couldn't resolve the peer's identity. The endpoint closes the connection with an appropriate error. Handlers that support multi-step auth (like SSH) should handle auth challenges within their protocol, not return `AuthRequired` until all attempts are exhausted.
- `Internal`: Handler-specific errors (protocol violations, upstream failures, etc.).

Handler panics are caught by tokio's task isolation. The connection is dropped, other connections continue.

## Connection

An opaque type wrapping a QUIC connection. Handlers receive a `Connection` in `handle()`.

```rust
pub struct Connection {
    // Private: wraps the underlying QUIC connection or test mock
}

impl Connection {
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub fn remote_alpn(&self) -> &[u8];
    pub fn remote_addr(&self) -> Option<SocketAddr>;
    pub fn close(&self, code: u32, reason: &str);
}
```

- `accept_bi()`: Wait for the peer to open a bidirectional stream. Returns `(SendStream, RecvStream)`.
- `open_bi()`: Open a bidirectional stream to the peer. Returns `(SendStream, RecvStream)`.
- `remote_alpn()`: The ALPN negotiated for this connection. Always present.
- `remote_addr()`: The peer's address, if available. Informational (NAT/proxy).
- `close()`: Close the connection with an error code and reason.

The `Connection` type does not expose quinn types in its public API. It wraps `quinn::Connection` internally, but the wrapper allows test implementations.

See [ADR-007](../../decisions/007-bistream-type-definition.md) for why handlers receive Connection instead of BiStream.

## BiStream

A trait for bidirectional byte streams. Used primarily for client-side and test scenarios.

```rust
pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}
```

Handlers that only need a single stream can obtain one via `connection.accept_bi()` and treat the `(SendStream, RecvStream)` pair as a BiStream. The `BiStream` trait is a convenience for:
- Client-side code that has a single bidirectional stream
- Test mocks that need to simulate a stream
- Future transport abstractions (WebTransport, raw TCP) that produce bidirectional byte streams

See [ADR-007](../../decisions/007-bistream-type-definition.md) for why BiStream is a trait.

## SendStream and RecvStream

Concrete types wrapping QUIC stream halves.

```rust
pub struct SendStream { /* wraps quinn::SendStream or test mock */ }
pub struct RecvStream { /* wraps quinn::RecvStream or test mock */ }

impl AsyncWrite for SendStream { ... }
impl AsyncRead for RecvStream { ... }
```

- `SendStream` implements `AsyncWrite`. Write bytes to the peer.
- `RecvStream` implements `AsyncRead`. Read bytes from the peer.
- These are not trait objects — they are concrete wrapper types that delegate to `quinn::SendStream` / `quinn::RecvStream` in production and to test mocks in tests.

This is a two-way door decision. If future transports need different stream types, `SendStream` and `RecvStream` can become wrappers with enum dispatch. For v1, concrete wrappers over quinn types are simpler and zero-cost.

## StreamError

```rust
pub enum StreamError {
    ConnectionClosed,
    StreamClosed,
    Timeout,
    Internal(io::Error),
}
```

Returned by `accept_bi()`, `open_bi()`, and stream read/write operations. Maps from `quinn::ConnectionError` and `quinn::StreamError`.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| ProtocolHandler receives Connection, not BiStream | [ADR-007](../../decisions/007-bistream-type-definition.md) | Handlers that need multiple streams (SSH, call) have direct access to the Connection |
| BiStream is a trait | [ADR-007](../../decisions/007-bistream-type-definition.md) | WASM door preserved, test mocks possible |
| HandlerError is non-fatal | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Handler errors close the connection, not the endpoint |
| SendStream/RecvStream are concrete wrappers | Two-way door | Can become enum dispatch later if multi-transport is needed |

## Open Questions

- **OQ-05**: See [open-questions.md](../../open-questions.md) — multi-transport. If quinn is the only transport in v1, SendStream/RecvStream can be concrete wrappers.
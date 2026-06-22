---
status: draft
last_updated: 2026-06-22-16
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
    // Private: handler-resolved identity for observability (OQ-11)
    identity: OnceLock<Identity>,
}

impl Connection {
    /// Construct from a quinn connection (feature-gated on quinn).
    #[cfg(feature = "quinn")]
    pub fn from_quinn(conn: quinn::Connection) -> Self;

    /// Construct from an iroh connection (feature-gated on iroh).
    #[cfg(feature = "iroh")]
    pub fn from_iroh(conn: iroh::Connection) -> Self;

    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub fn remote_alpn(&self) -> &[u8];
    pub fn remote_addr(&self) -> Option<SocketAddr>;
    pub fn close(&self, code: u32, reason: &str);
    pub fn set_identity(&self, identity: Identity) -> Result<(), IdentityAlreadySet>;
    pub fn identity(&self) -> Option<&Identity>;
}
```

- `accept_bi()`: Wait for the peer to open a bidirectional stream. Returns `(SendStream, RecvStream)`.
- `open_bi()`: Open a bidirectional stream to the peer. Returns `(SendStream, RecvStream)`.
- `remote_alpn()`: The ALPN negotiated for this connection. Always present.
- `remote_addr()`: The peer's address, if available. Informational (NAT/proxy).
- `close()`: Close the connection with an error code and reason.
- `set_identity()`: Store the handler-resolved identity for observability (OQ-11). Write-once-read-many — a second call returns an error. Handlers that resolve identity inside `handle()` call this; the endpoint and observability layers read it via `identity()`.
- `identity()`: Read the handler-resolved identity, if set. Returns `None` until `set_identity()` is called.

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

Concrete types wrapping QUIC stream halves. Both quinn and iroh produce QUIC connections — `SendStream` and `RecvStream` need to wrap either source.

```rust
pub struct SendStream { /* wraps quinn::SendStream or iroh::SendStream or test mock */ }
pub struct RecvStream { /* wraps quinn::RecvStream or iroh::RecvStream or test mock */ }

impl AsyncWrite for SendStream { ... }
impl AsyncRead for RecvStream { ... }
```

- `SendStream` implements `AsyncWrite`. Write bytes to the peer.
- `RecvStream` implements `AsyncRead`. Read bytes from the peer.
- These are concrete wrapper types that use internal enum dispatch to delegate to the appropriate QUIC stream type (quinn or iroh) in production, and to test mocks in tests.

Since the endpoint supports both quinn and iroh connection sources (ADR-010), streams may come from either. `Connection::from_quinn()` / `Connection::from_iroh()` wrap the appropriate stream source based on where the connection came from.

## StreamError

```rust
pub enum StreamError {
    ConnectionClosed,
    StreamClosed,
    Timeout,
    Internal(io::Error),
}
```

Returned by `accept_bi()`, `open_bi()`, and stream read/write operations. Maps from `quinn::ConnectionError` / `quinn::StreamError` and their iroh equivalents.

### Mapping `StreamError` to `HandlerError`

When a handler encounters a `StreamError` and needs to return from `handle()`, it maps to `HandlerError`:

| `StreamError` | `HandlerError` | Reason |
|---------------|----------------|--------|
| `ConnectionClosed` | `ConnectionClosed` | Peer closed the connection — clean exit |
| `StreamClosed` | `StreamError(io::Error)` | One stream closed mid-operation; the connection may still be usable for other streams |
| `Timeout` | `StreamError(io::Error)` (with `TimedOut` kind) | I/O-level timeout on a stream operation |
| `Internal(e)` | `StreamError(e)` | Underlying I/O error passes through |

Handlers that manage multiple streams (SSH, call) may catch `StreamError::StreamClosed` per-stream and continue serving other streams on the same connection — only `ConnectionClosed` forces `handle()` to return.

The mapping is provided as a `From` impl so handlers can use the `?` operator:

```rust
impl From<StreamError> for HandlerError {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::ConnectionClosed => HandlerError::ConnectionClosed,
            StreamError::StreamClosed => {
                HandlerError::StreamError(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "stream closed",
                ))
            }
            StreamError::Timeout => {
                HandlerError::StreamError(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "stream timed out",
                ))
            }
            StreamError::Internal(e) => HandlerError::StreamError(e),
        }
    }
}
```

This `From` impl is the canonical conversion — handler examples that use
`.await?` on `accept_bi()` / `open_bi()` rely on it. The `StreamError` →
`HandlerError::StreamError(io::Error)` mapping is lossy by design: the
distinction between stream-level and connection-level errors is preserved
in `StreamError`, but once a handler propagates via `HandlerError`, the
endpoint treats all variants as "close the connection" (one-ALPN-per-
connection, ADR-006).

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| ProtocolHandler receives Connection, not BiStream | [ADR-007](../../decisions/007-bistream-type-definition.md) | Handlers that need multiple streams (SSH, call) have direct access to the Connection |
| BiStream is a trait | [ADR-007](../../decisions/007-bistream-type-definition.md) | WASM door preserved, test mocks possible |
| HandlerError is non-fatal | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Handler errors close the connection, not the endpoint |
| SendStream/RecvStream wrap quinn + iroh | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Internal enum dispatch for both QUIC sources |
| Connection stores handler-resolved identity | OQ-11 (resolved) | `set_identity` via `OnceLock` — write-once-read-many for observability |

## Open Questions

None active for this document.
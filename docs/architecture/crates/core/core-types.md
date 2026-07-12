---
status: draft
last_updated: 2026-07-12
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

An opaque type wrapping a transport connection. Handlers receive a
`Connection` in `handle()`. The connection may be QUIC (quinn or iroh), a
generic single stream (TCP+TLS, SSH channel, WebTransport stream, wasm
stream — ADR-065), or any other connection shape a downstream crate
implements via `BidiStreamSource` (ADR-070).

```rust
pub struct Connection {
    source: Box<dyn BidiStreamSource>,
    alpn: Vec<u8>,
    // Private: handler-resolved identity for observability (OQ-11)
    identity: OnceLock<Identity>,
}

impl Connection {
    /// Construct from a quinn connection (feature-gated on quinn).
    #[cfg(feature = "quinn")]
    pub fn from_quinn(conn: quinn::Connection) -> Self;

    /// Construct from a quinn connection with an explicit ALPN (feature-gated
    /// on quinn). Used by the client path (`CallClient::connect`) and the
    /// endpoint's quinn accept loop.
    #[cfg(feature = "quinn")]
    pub fn from_quinn_with_alpn(conn: quinn::Connection, alpn: Vec<u8>) -> Self;

    /// Construct from an iroh connection (feature-gated on iroh).
    #[cfg(feature = "iroh")]
    pub fn from_iroh(conn: iroh::Connection) -> Self;

    /// Construct from any pre-split read/write pair. `accept_bi()` yields
    /// this pair once, then returns `ConnectionClosed`. `open_bi()` returns
    /// `StreamClosed`. No feature gate — generic, no transport deps.
    pub fn from_stream(
        send: impl AsyncWrite + Send + Unpin + 'static,
        recv: impl AsyncRead + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self;

    /// Convenience for a single bidirectional stream (e.g.
    /// `TlsStream<TcpStream>`). Splits internally via `tokio::io::split`.
    pub fn from_bidi(
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self;

    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    pub fn remote_alpn(&self) -> &[u8];
    pub fn remote_addr(&self) -> Option<SocketAddr>;
    pub fn close(&self, code: u32, reason: &str);
    pub fn set_identity(&self, identity: Identity) -> Result<(), IdentityAlreadySet>;
    pub fn identity(&self) -> Option<&Identity>;
}
```

- `accept_bi()`: Yield the next bidirectional stream this connection
  provides. **Transport semantics (ADR-065, extended by ADR-070):** QUIC
  (quinn/iroh) returns a new bidi stream on each call, `ConnectionClosed`
  when the underlying connection closes; a single-stream connection
  (TCP+TLS, SSH channel, WebTransport stream, wasm stream) yields the
  underlying stream on the first call, then `ConnectionClosed` on all
  subsequent calls; a `BidiStreamSource` implementation (e.g. a channels
  connection — see ADR-070) yields one stream per channel, then
  `ConnectionClosed` when the source closes. Handlers that loop `accept_bi`
  (TtyAdapter) get one session per single-stream connection; handlers that
  call once (HttpAdapter) get the stream directly. Both correct, no
  branching on transport.
- `open_bi()`: Open a bidirectional stream to the peer. Returns
  `(SendStream, RecvStream)`. On a single-stream connection, returns
  `StreamClosed` — a single stream cannot open new application streams.
- `remote_alpn()`: The ALPN negotiated for this connection. Always present.
- `remote_addr()`: The peer's address, if available. Informational (NAT/proxy).
- `close()`: Close the connection with an error code and reason. The
  `code`/`reason` args are QUIC application-level close codes; non-QUIC
  sources ignore them (the drop is the close — ADR-065). The signature is
  preserved on the trait so the public `Connection::close` API is
  unchanged across transports; see ADR-070 §"REQ-CORE-02" for why the
  QUIC-shaped signature stays on the trait rather than being split.
- `set_identity()`: Store the handler-resolved identity for observability (OQ-11). Write-once-read-many — a second call returns an error. Handlers that resolve identity inside `handle()` call this; the identity is read by handler-side logging (the handler logs which identity it resolved) and is available on the `Connection` for any code that holds a reference to it. The endpoint does **not** read `identity()` after `handle()` returns — the `Connection` is moved into the spawned handler task (endpoint.md), so the endpoint no longer has a reference. Connection-level observability (remote addr, ALPN, connection ID) is logged by the endpoint before the move; identity-level observability is logged by the handler. See OQ-11 for the full resolution.

The `Connection` type does not expose quinn/iroh types in its public API.
It holds a `Box<dyn BidiStreamSource>` (ADR-070); the QUIC and
single-stream implementations are crate-private (`QuinnBidiStreamSource`,
`IrohBidiStreamSource`, `StreamBidiStreamSource`), with the QUIC variants
feature-gated and the `Stream` variant always available (no transport
deps). Downstream crates implement `BidiStreamSource` directly to add new
connection shapes (e.g. the channels crate's `ChannelBidiStreamSource`)
without editing `alknet-core`. See
[ADR-070](../../decisions/070-bidistreamsource-trait.md) for the trait and
the extension model, and
[ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md)
for the `from_stream` generalization and the yield-once `accept_bi`
contract that the `StreamBidiStreamSource` impl preserves.

See [ADR-007](../../decisions/007-bistream-type-definition.md) for why handlers receive Connection instead of BiStream.

## BidiStreamSource

The trait `Connection` holds. Downstream crates implement it to add new
connection shapes (channels, a future transport, a test double beyond the
`from_stream` case) without editing `alknet-core`. See
[ADR-070](../../decisions/070-bidistreamsource-trait.md) for the full
rationale.

```rust
#[async_trait]
pub trait BidiStreamSource: Send + Sync + 'static {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    fn remote_addr(&self) -> Option<SocketAddr>;
    fn close(&self, code: u32, reason: &str);
}
```

- `accept_bi()` / `open_bi()`: the stream-yield operations. Each
  implementation defines its yield semantics (QUIC: many; single-stream:
  yield-once then `ConnectionClosed`; channels: one per channel). The
  contract is that handlers don't branch on transport — see
  `Connection::accept_bi` above and ADR-065's yield-once contract.
- `remote_addr()`: the peer's address, if available. Same semantic as
  `Connection::remote_addr` (which delegates here).
- `close(code, reason)`: QUIC application-level close codes. Non-QUIC
  sources ignore the args (the drop is the close). The signature matches
  the public `Connection::close` verbatim — see ADR-070 §"REQ-CORE-02".

`Connection`-level operations (`remote_alpn`, `set_identity`, `identity`)
do **not** appear on `BidiStreamSource` — they live on `Connection` itself
(the `alpn` field and the `identity` `OnceLock`), so they carry no new
indirection and are not the transport's concern.

### Built-in implementations (crate-private)

| Impl | Constructor | Yield semantics |
|------|-------------|-----------------|
| `QuinnBidiStreamSource` | `Connection::from_quinn` / `from_quinn_with_alpn` (feature `quinn`) | many streams |
| `IrohBidiStreamSource` | `Connection::from_iroh` (feature `iroh`) | many streams |
| `StreamBidiStreamSource` | `Connection::from_stream` / `from_bidi` (no feature gate) | yield-once, then `ConnectionClosed`; `open_bi` returns `StreamClosed` |

Downstream crates do not wrap `from_stream` to implement
`BidiStreamSource` — they implement the trait directly. `from_stream` is
the compatibility path for callers that want a `Connection` over a single
pre-split stream (the ADR-065 use case).

## BiStream

A trait for bidirectional byte streams. Used primarily for client-side and test scenarios.

```rust
pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}
```

Handlers that only need a single stream can obtain one via `connection.accept_bi()` and treat the `(SendStream, RecvStream)` pair as a BiStream. The `BiStream` trait is a convenience for:
- Client-side code that has a single bidirectional stream
- Test scenarios that need to simulate a stream
- Transports that produce a single bidirectional byte stream (TCP+TLS via `from_bidi`, SSH channels, WebTransport streams, wasm streams) — all dispatchable through the same `HandlerRegistry` as QUIC connections via `Connection::from_stream` (ADR-065)

See [ADR-007](../../decisions/007-bistream-type-definition.md) for why BiStream is a trait.

## SendStream and RecvStream

Concrete types wrapping transport stream halves. Both quinn and iroh
produce QUIC connections; `from_stream` adds a generic single-stream source.
`SendStream` and `RecvStream` wrap any of the three via internal enum
dispatch.

```rust
pub struct SendStream { /* wraps quinn::SendStream, iroh::SendStream, or a generic Box<dyn AsyncWrite> */ }
pub struct RecvStream { /* wraps quinn::RecvStream, iroh::RecvStream, or a generic Box<dyn AsyncRead> */ }

impl AsyncWrite for SendStream { ... }
impl AsyncRead for RecvStream { ... }
```

- `SendStream` implements `AsyncWrite`. Write bytes to the peer.
- `RecvStream` implements `AsyncRead`. Read bytes from the peer.
- These are concrete wrapper types that use internal enum dispatch to
  delegate to the appropriate stream source: quinn or iroh (QUIC,
  feature-gated) in production, or `Stream` (a generic
  `Box<dyn AsyncRead/Write + Send + Unpin>`, no feature gate) for
  single-stream connections constructed via `from_stream` / `from_bidi`.

Since the endpoint supports both quinn and iroh connection sources
(ADR-010), and `from_stream` adds the generic single-stream source
(ADR-065), streams may come from any of the three. `Connection::from_quinn()`
/ `from_iroh()` wrap the appropriate QUIC stream source based on where the
connection came from; `Connection::from_stream()` / `from_bidi()` wrap a
generic `AsyncRead + AsyncWrite` pair as the `Stream` variant.

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

**Note on single-stream connections (ADR-065, ADR-070):** `StreamClosed` from `open_bi` on a single-stream `BidiStreamSource` (the `StreamBidiStreamSource` backed by `from_stream`/`from_bidi`) is terminal for that connection — no other streams exist to continue with. The "connection may still be usable" framing above applies to the QUIC case (a per-stream closure where the connection lives); the single-stream case is a transport property (one stream is all there is), not a mid-operation stream closure. `accept_bi` on a single-stream source returns `ConnectionClosed` after the first yield (not `StreamClosed`), so handlers that loop `accept_bi` exit cleanly.

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

## Capabilities

Outbound credentials injected by the assembly layer at registration time.
A handler uses `Capabilities` to make authenticated outbound calls (LLM
provider API keys, HTTP service tokens, signing keys). See ADR-014 for the
secret-material flow and ADR-022 for the registration-bundle wiring.

```rust
/// Outbound credentials for a handler. Non-serializable, zeroized,
/// immutable after construction. `Clone` is required by the composition
/// model (`parent.capabilities.clone()` in `OperationEnv::invoke()`).
///
/// The concrete internal shape (a typed map, a struct with named fields)
/// is a two-way door, but the public API is fixed: `new()`, `with_api_key()`,
/// `with_http_token()`, and `get()`. Fields are private — callers cannot
/// mutate the credentials after construction. This makes the clone-semantics
/// two-way door genuinely two-way: Arc-based clone (shared immutable state)
/// and deep-copy clone (isolated state) are behaviorally identical when
/// neither supports mutation. See ADR-014, ADR-022, review #002 W2.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Capabilities {
    // Private — no interior mutability. The builder API (new, with_*) is
    // the only construction path. Immutability after construction is the
    // security guard that makes clone semantics safe.
    entries: HashMap<String, Secret<String>>,
}

impl Capabilities {
    /// Empty capabilities — for handlers that make no outbound calls.
    pub fn new() -> Self;

    /// Add an API key (e.g., "google", "openai") to the capabilities.
    pub fn with_api_key(mut self, service: &str, key: String) -> Self;

    /// Add an HTTP bearer token (e.g., "vastai", "github") to the capabilities.
    pub fn with_http_token(mut self, service: &str, token: String) -> Self;

    /// Retrieve a credential by service name, if present.
    pub fn get(&self, service: &str) -> Option<&Secret<String>>;
}
```

- **Non-serializable**: `Capabilities` does **not** derive `Serialize`. It
  cannot appear in `EventEnvelope` payloads even by accident. This is a
  type-level enforcement of ADR-014's "call protocol carries no secret material."
- **Zeroized**: derives `Zeroize` and `ZeroizeOnDrop`. Secret material does
  not linger in freed heap memory.
- **`Clone` + `Send + Sync`**: required by the composition model —
  `OperationEnv::invoke()` clones the parent's capabilities for each child.
  `Send + Sync` is required because the context is held across async task
  boundaries.
- **Immutable after construction**: no `set`, no `insert`, no `mut` accessors.
  This is the guard from review #002 W2 — it makes the Arc-vs-deep-copy clone
  semantics genuinely two-way (shared immutable state is safe).
- **Module location**: `Capabilities` lives in alknet-core (it's a shared type
  — see overview.md's Shared Types table). alknet-call imports it.

See [operation-registry.md → Capability Injection](../call/operation-registry.md#capability-injection)
for how the dispatch path populates `OperationContext.capabilities` from the
registration bundle.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| ProtocolHandler receives Connection, not BiStream | [ADR-007](../../decisions/007-bistream-type-definition.md) | Handlers that need multiple streams (SSH, call) have direct access to the Connection |
| BiStream is a trait | [ADR-007](../../decisions/007-bistream-type-definition.md) | WASM door preserved, test mocks possible |
| `Connection::from_stream` — generic single-stream connections | [ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md) | `from_stream`/`from_bidi` accept any `AsyncRead + AsyncWrite`; yield-once `accept_bi` contract; unblocks TCP+TLS, SSH channels, WebTransport, wasm; QUIC variants feature-gated, `Stream` variant always available; `MockConnection`/`ConnectionKind::Mock` removed (tests use `from_stream` with `sink`/`empty`) |
| `BidiStreamSource` — open `Connection` for extension | [ADR-070](../../decisions/070-bidistreamsource-trait.md) | `Connection` holds `Box<dyn BidiStreamSource>`; QUIC/iroh/stream wrap crate-private impls; downstream crates implement the trait to add connection shapes (channels, future transports) without editing core; public `Connection` API preserved; `close(code, reason)` kept on the trait (non-QUIC impls ignore the args — fixes the ADR-065 leftover clippy warning under `--no-default-features`) |
| HandlerError is non-fatal | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md) | Handler errors close the connection, not the endpoint |
| SendStream/RecvStream wrap quinn + iroh + generic streams | [ADR-010](../../decisions/010-alpn-router-and-endpoint.md), [ADR-065](../../decisions/065-connection-from-stream-generic-single-stream.md) | Internal enum dispatch for QUIC sources and the generic `Stream` variant |
| Connection stores handler-resolved identity | OQ-11 (resolved) | `set_identity` via `OnceLock` — write-once-read-many; read by handler-side logging, not by the endpoint (C13 resolved) |
| Capabilities type | [ADR-014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Non-serializable, zeroized, immutable after construction; `Clone` for composition propagation |

## Open Questions

None active for this document.
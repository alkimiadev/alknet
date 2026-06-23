---
id: core/core-types
name: "Implement core types: ProtocolHandler, Connection, BiStream, SendStream, RecvStream, StreamError, HandlerError, Capabilities"
status: completed
depends_on: [core/crate-init]
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Implement the core types in `src/types.rs`. These are the foundational
abstractions that every handler crate depends on. This is the most
cross-crate-boundary task in core — `Capabilities` in particular is used
heavily by alknet-call's operation registry and composition model.

### ProtocolHandler trait

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    fn alpn(&self) -> &'static [u8];
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

- `alpn()` returns the handler's ALPN identifier as a static byte string
- `handle()` receives a `Connection` (not a single BiStream) and an `AuthContext`
- Handlers that need a single stream call `connection.accept_bi()` once
- Handlers that multiplex (SSH, call) open/accept streams as needed

See ADR-002, ADR-007.

### HandlerError

```rust
pub enum HandlerError {
    ConnectionClosed,
    StreamError(io::Error),
    AuthRequired,
    Internal(Box<dyn std::error::Error + Send + Sync>),
}
```

Non-fatal errors within `handle()`. The endpoint catches these, logs them,
closes the connection. Other connections are unaffected. Handler panics are
caught by tokio's task isolation.

### Connection

```rust
pub struct Connection {
    // Private: wraps the underlying QUIC connection or test mock
    identity: OnceLock<Identity>,
}

impl Connection {
    #[cfg(feature = "quinn")]
    pub fn from_quinn(conn: quinn::Connection) -> Self;
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

- Opaque type wrapping a QUIC connection (quinn or iroh, feature-gated)
- `set_identity` is write-once-read-many via `OnceLock` (OQ-11) — handlers
  store resolved identity for observability; the endpoint does NOT read it
  after `handle()` returns (the Connection is moved into the spawned task)
- Internal enum dispatch for quinn vs iroh vs test mock
- `Connection` does not expose quinn types in its public API

### BiStream trait

```rust
pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}
```

A convenience trait for client-side code, test mocks, and future transport
abstractions (WebTransport, raw TCP). Handlers that need a single stream
obtain one via `connection.accept_bi()` and treat the pair as a BiStream.

### SendStream and RecvStream

```rust
pub struct SendStream { /* wraps quinn::SendStream or iroh::SendStream or test mock */ }
pub struct RecvStream { /* wraps quinn::RecvStream or iroh::RecvStream or test mock */ }

impl AsyncWrite for SendStream { ... }
impl AsyncRead for RecvStream { ... }
```

Concrete wrapper types using internal enum dispatch to delegate to the
appropriate QUIC stream type (quinn or iroh) in production, and to test mocks
in tests.

### StreamError

```rust
pub enum StreamError {
    ConnectionClosed,
    StreamClosed,
    Timeout,
    Internal(io::Error),
}
```

Returned by `accept_bi()`, `open_bi()`, and stream read/write operations.
Maps from `quinn::ConnectionError` / `quinn::StreamError` and iroh equivalents.

### From<StreamError> for HandlerError

```rust
impl From<StreamError> for HandlerError {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::ConnectionClosed => HandlerError::ConnectionClosed,
            StreamError::StreamClosed => HandlerError::StreamError(
                io::Error::new(io::ErrorKind::ConnectionReset, "stream closed")),
            StreamError::Timeout => HandlerError::StreamError(
                io::Error::new(io::ErrorKind::TimedOut, "stream timed out")),
            StreamError::Internal(e) => HandlerError::StreamError(e),
        }
    }
}
```

This `From` impl is the canonical conversion — handlers use `?` on
`accept_bi()` / `open_bi()`.

### Capabilities

```rust
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Capabilities {
    entries: HashMap<String, Secret<String>>,
}

impl Capabilities {
    pub fn new() -> Self;
    pub fn with_api_key(mut self, service: &str, key: String) -> Self;
    pub fn with_http_token(mut self, service: &str, token: String) -> Self;
    pub fn get(&self, service: &str) -> Option<&Secret<String>>;
}
```

Critical constraints (ADR-014, ADR-022, review #002 W2):
- **Non-serializable**: does NOT derive `Serialize`. Cannot appear in
  `EventEnvelope` payloads even by accident.
- **Zeroized**: derives `Zeroize` and `ZeroizeOnDrop`. Secret material does
  not linger in freed heap memory.
- **Clone + Send + Sync**: required by the composition model —
  `OperationEnv::invoke()` clones the parent's capabilities for each child.
- **Immutable after construction**: no `set`, no `insert`, no `mut` accessors.
  This is the guard from review #002 W2 — makes clone semantics genuinely
  two-way (Arc-based vs deep-copy are behaviorally identical when neither
  supports mutation).
- **Private fields**: the builder API (`new`, `with_*`) is the only
  construction path.

Use `secrecy::Secret<String>` (from the `secrecy` crate) or a similar wrapper
for the secret values. Add `secrecy` to dependencies if needed, or implement
a simple `Secret` wrapper that zeroizes on drop and redacts in Debug.

### IdentityAlreadySet error

```rust
#[derive(Debug, thiserror::Error)]
pub enum IdentityAlreadySet {
    #[error("connection identity already set")]
    AlreadySet,
}
```

Returned by `Connection::set_identity()` if called a second time.

## Acceptance Criteria

- [ ] `ProtocolHandler` trait defined with `alpn()` and `handle()` (async)
- [ ] `HandlerError` enum with all 4 variants
- [ ] `Connection` struct with all methods (from_quinn/from_iroh feature-gated)
- [ ] `Connection::set_identity` write-once via `OnceLock`, returns `IdentityAlreadySet` on second call
- [ ] `BiStream` trait defined (AsyncRead + AsyncWrite + Send + Unpin)
- [ ] `SendStream` implements `AsyncWrite`
- [ ] `RecvStream` implements `AsyncRead`
- [ ] `StreamError` enum with all 4 variants
- [ ] `From<StreamError> for HandlerError` impl
- [ ] `Capabilities` struct with `new()`, `with_api_key()`, `with_http_token()`, `get()`
- [ ] `Capabilities` derives `Clone`, `Zeroize`, `ZeroizeOnDrop` — NOT `Serialize`
- [ ] `Capabilities` fields are private (builder API only, no mut accessors)
- [ ] `IdentityAlreadySet` error type
- [ ] Unit tests for Capabilities (build, get, clone, zeroize)
- [ ] Unit test: `Connection::set_identity` once succeeds, twice returns error
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/core-types.md — all type definitions
- docs/architecture/decisions/002-protocol-handler-trait.md — ADR-002
- docs/architecture/decisions/007-bistream-type-definition.md — ADR-007
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014 (Capabilities)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022

## Notes

> This is the most cross-crate-boundary task in core. `Capabilities` is used
> heavily by alknet-call's operation registry and composition model — it must
> be right the first time. The immutability guard (no mut accessors) is the
> security control from review #002 W2 that makes clone semantics safe. The
> `Connection` type uses internal enum dispatch for quinn/iroh/test — do not
> expose quinn types in the public API.

## Summary

Implemented all core types in `types.rs`: `ProtocolHandler` trait (`alpn` +
`handle`), `HandlerError` (4 variants), `Connection` (quinn/iroh feature-gated
enum dispatch, `OnceLock` write-once identity, `accept_bi`/`open_bi`/`close`/
`remote_alpn`/`remote_addr`), `BiStream` trait, `SendStream`/`RecvStream`
`AsyncWrite`/`AsyncRead` wrappers, `StreamError`, `From<StreamError>` for
`HandlerError`, `Capabilities` (`Zeroize`+`ZeroizeOnDrop`, immutable builder API,
`Secret<String>` wrapper, non-serializable), `IdentityAlreadySet`. Added minimal
`Identity`/`AuthContext` in `auth.rs`. 13 unit tests pass; clippy clean across
feature combos. Merged to develop.

Notable: `quinn::Connection` has no `alpn()` accessor so ALPN is stored separately
(`from_quinn_with_alpn`); iroh 0.35 types via `iroh::endpoint::*`; iroh Connection
has no `remote_address` (returns None per spec).
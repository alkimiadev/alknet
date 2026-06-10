# irpc: Serialization and Utility Modules

## Varint Utilities

The `varint-util` module (available with `rpc` or `varint-util` feature) provides LEB128 varint encoding/decoding compatible with postcard's format.

### Async Reading

```rust
pub async fn read_varint_u64<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<u64>>
```

Reads a LEB128-encoded `u64` from an async reader. Returns `Ok(None)` on `UnexpectedEof` at the first byte position (clean stream end).

**Format:** Each byte uses 7 bits for the value, MSB as continuation bit. Values stored little-endian (least significant group first).

### Sync Writing

```rust
pub fn write_varint_u64_sync<W: io::Write>(writer: &mut W, value: u64) -> io::Result<usize>
```

Writes a `u64` as LEB128 to a synchronous writer.

### Length-Prefixed Encoding

```rust
// Sync:
pub fn write_length_prefixed<T: Serialize>(write: impl io::Write, value: T) -> io::Result<()>
pub trait WriteVarintExt: io::Write {
    fn write_varint_u64(&mut self, value: u64) -> io::Result<usize>;
    fn write_length_prefixed<T: Serialize>(&mut self, value: T) -> io::Result<()>;
}

// Async:
pub trait AsyncReadVarintExt: AsyncRead + Unpin {
    fn read_varint_u64(&mut self) -> impl Future<Output = io::Result<Option<u64>>>;
    fn read_length_prefixed<T: DeserializeOwned>(&mut self, max_size: usize) -> impl Future<Output = io::Result<T>>;
}

pub trait AsyncWriteVarintExt: AsyncWrite + Unpin {
    fn write_varint_u64(&mut self, value: u64) -> impl Future<Output = io::Result<usize>>;
    fn write_length_prefixed<T: Serialize>(&mut self, value: V) -> impl Future<Output = io::Result<usize>>;
}
```

The length-prefix format is:
```
[varint-encoded-length][postcard-serialized-data]
```

Used internally by irpc for framing all messages on QUIC streams. The `max_size` parameter in `read_length_prefixed` prevents memory exhaustion from malicious length values.

## noq Endpoint Setup

The `noq_endpoint_setup` feature provides helpers for creating noq endpoints with TLS configuration:

```rust
pub fn configure_client(server_certs: &[&[u8]]) -> Result<ClientConfig>
pub fn configure_server() -> Result<(ServerConfig, Vec<u8>)>
pub fn configure_client_insecure() -> Result<ClientConfig>

// Non-WASM only:
pub fn make_client_endpoint(bind_addr: SocketAddr, server_certs: &[&[u8]]) -> Result<Endpoint>
pub fn make_insecure_client_endpoint(bind_addr: SocketAddr) -> Result<Endpoint>
pub fn make_server_endpoint(bind_addr: SocketAddr) -> Result<(Endpoint, Vec<u8>)>
```

- `configure_server()`: Creates a self-signed certificate with rcgen and configures the server with TLS 1.3. Returns the DER-encoded certificate for clients to trust.
- `configure_client()`: Configures a client to trust specific DER certificates.
- `configure_client_insecure()`: Skips certificate verification (for testing only).
- Server endpoints set `max_concurrent_uni_streams(0)` to disable unidirectional streams (only bidirectional streams are used).
- Keep-alive interval is set to 1 second on client configs.

## FusedOneshotReceiver

```rust
pub(crate) struct FusedOneshotReceiver<T>(pub tokio::sync::oneshot::Receiver<T>);
```

A wrapper that prevents panics when polling an already-completed oneshot receiver. After the inner receiver resolves, subsequent polls return `Poll::Pending` indefinitely instead of panicking.

This is important because irpc's `oneshot::Receiver` can be wrapped in `Receiver::Boxed` (a `BoxFuture`), and the inner future might be polled multiple times in certain select patterns.

## now_or_never

```rust
pub(crate) fn now_or_never<F: Future>(future: F) -> Option<F::Output>
```

Attempts to complete a future immediately without blocking. If the future would block, returns `None`. Used internally by `NoqSenderInner::try_send()` to attempt an immediate write to the QUIC stream without yielding.

Implementation uses a no-op waker to poll the future once.

## Spans Feature

When the `spans` feature is enabled (default), `WithChannels` includes a `span: tracing::Span` field:

```rust
pub struct WithChannels<I: Channels<S>, S: Service> {
    pub inner: I,
    pub tx: <I as Channels<S>>::Tx,
    pub rx: <I as Channels<S>>::Rx,
    #[cfg(feature = "spans")]
    pub span: tracing::Span,
}
```

The span is captured from `tracing::Span::current()` at the time of `WithChannels` construction (via `From` implementations). This preserves tracing context across async message-passing boundaries.

The `rpc_requests` macro generates a `parent_span()` method on the message enum when `no_spans` is not set:

```rust
impl ComputeMessage {
    pub fn parent_span(&self) -> tracing::Span {
        let span = match self {
            ComputeMessage::Multiply(inner) => inner.parent_span_opt(),
            ComputeMessage::Sum(inner) => inner.parent_span_opt(),
        };
        span.cloned().unwrap_or_else(|| tracing::Span::current())
    }
}
```

This allows server-side handlers to enter the client's tracing span:

```rust
async fn handle(msg: ComputeMessage) {
    let _entered = msg.parent_span().enter();
    // ... processing happens in the client's tracing context
}
```

When `no_spans` is set in the macro, no span-related code is generated, making it compatible with builds that don't have the `spans` feature enabled.
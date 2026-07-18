# ADR-092: `BiStream` as the Handler Leaf — Unify the Split-Pair `accept_bi`

## Status

Proposed (amends ADR-070's `BidiStreamSource::accept_bi` return type;
amends ADR-065's `from_stream` / `from_bidi` constructors; amends
ADR-074's `ChannelBidiStreamSource::accept_bi` return type;
resurrects ADR-007's `BiStream` trait as the handler-facing leaf type;
supersedes the "two Phase 6 issues" framing in
`docs/research/alknet-crate-extraction/findings.md`)

## Context

The crate-extraction findings doc
(`docs/research/alknet-crate-extraction/findings.md` Phase 6) deferred
the `alknet-http` rework on the grounds that the `QuicStream` wrapper
(44 lines, `crates/alknet-http/src/server/adapter.rs:271-314`) is a
*necessary* adapter — `accept_bi()` returns a split
`(SendStream, RecvStream)` pair, `SendStream` implements only
`AsyncWrite`, `RecvStream` implements only `AsyncRead`, and
`HttpAdapter::serve_io` needs a single `AsyncRead + AsyncWrite`. The
finding was correct about the symptom and wrong about the cause. This
ADR untangles the cause.

### The tangle: five abstractions for "a bidirectional byte stream"

Today the codebase has five abstractions for the same concept, and every
handler picks a joining strategy per-handler:

| # | Abstraction | Where | Notes |
|---|-------------|-------|-------|
| 1 | `BiStream` trait (`AsyncRead + AsyncWrite + Send + Unpin`) | `crates/alknet-core/src/types.rs:226` | **Vestigial in code.** Declared per ADR-007, named in ADR-070 as "a client-side / test convenience trait," but grep across the workspace finds **zero** consumers — no `impl BiStream`, no `dyn BiStream`, no `Box<dyn BiStream>`. The trait is the ecosystem convention (`tokio::net::TcpStream`, `TlsStream<TcpStream>`, `russh::Channel::into_stream()` all satisfy it natively) but it was never wired in. |
| 2 | `Connection` (yields `(SendStream, RecvStream)` via `accept_bi`) | `crates/alknet-core/src/types.rs:507` | The handler-facing abstraction. Leaf is split. |
| 3 | `SendStream` (AsyncWrite-only) + `RecvStream` (AsyncRead-only) | `crates/alknet-core/src/types.rs:228-294` | The actual leaves handlers receive. Each carries a quinn/iroh/generic enum (`SendStreamKind` / `RecvStreamKind`) and dispatches per-call. |
| 4 | `WsStream` trait (recv/send `axum::ws::Message`) | `crates/alknet-http/src/websocket/upgrade.rs:44` | Bypasses `Connection` entirely. The WS session runs its own dispatch loop directly over `axum::extract::ws::WebSocket`; `CallConnection::new_overlay_only` is used instead of `Connection::from_bidi`. ADR-044/048 already say "a WS message stream is another `BiStream`-satisfying transport" — the code does not. |
| 5 | `MpscSendStream` / `MpscRecvStream` (channels POC) | `/workspace/alknet-channels-poc/src/mpsc_stream.rs` | Split mpsc-backed halves fed to `Connection::from_stream`. The channels POC's `TunnelHandler` consumes them directly as two `tokio::io::copy` pumps — the split shape is right for the tunnel, wrong for HTTP. |

The two Phase 6 issues are symptoms of one root: **the leaf type is
split, so every consumer either re-joins it (HTTP's `QuicStream`,
`QuicStreamDuplex` test helper) or bypasses `Connection` entirely
(WS's `WsStream` + bespoke dispatch loop).**

### What ADR-070 left half-finished

ADR-070 extracted `BidiStreamSource` as the connection-level extension
point and kept `accept_bi` returning `(SendStream, RecvStream)`:

```rust
async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
```

This preserved the existing `Connection` API verbatim (the right call
for ADR-070's scope — the trait extraction was the one-way door; the
return shape was a known leftover). But it left the join *per-handler*:
every handler that wants a single duplex stream re-implements the same
`AsyncRead + AsyncWrite` wrapper. The wrapper is small (44 lines) and
correct, but it is duplicated per-handler, and the duplication is what
forces the WS path into a bespoke `WsStream` trait instead of running
through `Connection::from_bidi` like every other transport.

### What ADR-007 already specified

ADR-007 defined `BiStream: AsyncRead + AsyncWrite + Send + Unpin` as
the leaf, and the ADR's "Why BiStream is still defined as a trait"
section (lines 86-94) lists three uses: WASM door, testing,
portability. The trait was placed in `alknet-core` and then not used
as the handler leaf — ADR-002's `handle` signature takes `Connection`
(correctly, for multi-stream handlers like TTY that loop `accept_bi`),
and `Connection::accept_bi` returns the split pair. `BiStream` became
"the trait that would have been the leaf if handlers received a single
stream." This ADR makes it the actual leaf — not by changing the
handler signature (still `Connection`), but by changing what
`accept_bi` yields.

### The ecosystem convention

`AsyncRead + AsyncWrite + Send + Unpin` (or close variants) is the
Rust ecosystem's standard "bidirectional byte stream" shape:

- `tokio::net::TcpStream`, `tokio::net::UdpSocket`
- `tokio_rustls::server::TlsStream<TcpStream>`
- `russh::Channel::into_stream()` — "Consume the Channel to produce a
  bidirectional stream, sending and receiving `ChannelMsg::Data` as
  `AsyncRead + AsyncWrite`"
- `tokio::io::DuplexStream`
- A WS-message adapter (the one place real adapter work is required)

All satisfy `BiStream` natively. Making `BiStream` the handler leaf
aligns alknet with the convention: `Connection::from_bidi(stream)`
accepts any of these directly, no per-handler wrapper.

### The two-pump shape is unaffected

The tunnel handler (ADR-078) and the SSH `direct-tcpip` handler
(future) use the split shape — two `tokio::io::copy` pumps, one per
direction. With `BiStream` as the leaf, these handlers call
`tokio::io::split(bidi)` to get `(ReadHalf, WriteHalf)` — the same
stdlib idiom `tokio::io::split` already provides for `TcpStream` and
`TlsStream<TcpStream>`. The split is a stdlib call at the handler
boundary, not a per-handler trait wrapper. ADR-078's
shutdown-on-completion contract applies to the `ReadHalf`/`WriteHalf`
unchanged.

### The TTY named-sub-streams case (ADR-074, ADR-077)

ADR-074 specifies `into_sub_streams()` returning
`Vec<(u8, SubStreamHandle)>` where `SubStreamHandle` is
`Send(SendStream) | Recv(RecvStream)`. ADR-077's TTY-inside-channels
mode destructures into five named handles (`stdin`, `stdout`,
`stderr`, `ctrl_in`, `ctrl_out`). **Every stream_type is
unidirectional** (ADR-071) — the typed-sub-stream leaves are
unidirectional by design, and the join is wrong for them.

This means `SendStream` and `RecvStream` cannot fully go away. They
remain as the typed-sub-stream leaves for the channels-inside-TTY
case (and any future handler that destructures a `ChannelSubStreams`).
What goes away is the *quinn-welding* in them: today `SendStreamKind`
/ `RecvStreamKind` are enums with `Quinn` / `Iroh` / `Stream` variants
that dispatch per-call. Once `accept_bi` returns a joined `BiStream`,
the quinn/iroh `accept_bi` impls do the join *once* (via
`tokio::io::join`) and yield a `BiStream`. The `SendStream` /
`RecvStream` types collapse to thin newtypes over
`Box<dyn AsyncWrite + Send + Unpin>` / `Box<dyn AsyncRead + Send +
Unpin>` — used only by `into_sub_streams()` and the channels reassembly
path, never by a top-level handler's `accept_bi` call.

## Decision

### `accept_bi` returns `BiStream`

`BidiStreamSource::accept_bi` returns a single `BiStream`, not a split
pair:

```rust
#[async_trait]
pub trait BidiStreamSource: Send + Sync + 'static {
    async fn accept_bi(&self) -> Result<BidiStream, StreamError>;
    async fn open_bi(&self) -> Result<BidiStream, StreamError>;
    fn remote_addr(&self) -> Option<SocketAddr>;
    fn close(&self, code: u32, reason: &str);
}
```

`Connection::accept_bi` / `open_bi` delegate verbatim. The public
`Connection` API is preserved except for the return type — which is a
type change every caller sees, addressed below.

### `BiStream` is a concrete newtype, not a bare trait

A bare `dyn BiStream` won't work: `AsyncRead` / `AsyncWrite` methods
take `Pin<&mut Self>`, and trait objects need `Pin<Box<dyn ...>>` or a
newtype that owns the inner stream and re-projects. The clean shape is
a concrete struct that boxes the inner joined stream:

```rust
pub struct BiStream {
    inner: Box<dyn AsyncReadWrite + Send + Unpin>,
}

// Internal helper trait — the union of AsyncRead + AsyncWrite + Send +
// Unpin. Not public; exists only to give BiStream a single boxed field.
trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncReadWrite for T {}

impl AsyncRead for BiStream { /* delegate to self.inner */ }
impl AsyncWrite for BiStream { /* delegate to self.inner */ }
```

`BiStream: AsyncRead + AsyncWrite + Send + Unpin` by construction. The
old `pub trait BiStream: AsyncRead + AsyncWrite + Send + Unpin {}`
(ADR-007, `types.rs:226`) is removed — the trait was never consumed,
and the concrete struct carries the same trait bounds forward as
implied bounds, not a marker trait. This is the ADR-007 resurrection:
the name and the bounds survive, the shape becomes a concrete leaf.

### The join moves into core's quinn/iroh impls (once)

```rust
#[cfg(feature = "quinn")]
async fn accept_bi(&self) -> Result<BidiStream, StreamError> {
    let (send, recv) = self.conn.accept_bi().await
        .map_err(map_quinn_connection_error)?;
    Ok(BiStream::from_joined(send, recv))  // tokio::io::join internally
}
```

The `QuicStream` wrapper (`adapter.rs:271-314`, 44 lines) becomes
`BiStream::from_joined(send, recv)` — one line, in core, invisible to
handlers. The same applies to iroh. The join is no longer per-handler.

### `Connection::from_bidi` is the only public stream constructor;
`from_stream` is removed

Today `from_bidi` is a convenience wrapper that calls
`tokio::io::split(stream)` then `from_stream(send, recv)`, and
`from_stream` bakes the split into the constructor API — the same
split-leaf shape pushed one step earlier. With `BiStream` as the leaf,
`from_bidi` is the only public constructor that takes a joined stream.
`Connection::from_stream(send, recv, ...)` is **removed**.

The rule this normalizes: **the split never crosses a crate boundary
as part of a constructor.** A crate that produces split halves
naturally (the channels reassembly path, which produces
`MpscSendStream` / `MpscRecvStream` as distinct async types) joins
them *itself* via `tokio::io::join(send, recv)` (one line) and calls
`from_bidi`. A crate that has a joined stream (`TcpStream`,
`TlsStream<TcpStream>`, `russh::Channel::into_stream()`,
`WsBidiStream`, even a test `DuplexStream`) calls `from_bidi` directly.
`Connection` only ever holds a `BiStream`. The split is a crate-internal
concern of wherever it naturally arises.

The existing `from_stream` call sites update mechanically:

- `crates/alknet-client/src/dial/tcp_tls.rs` already uses `from_bidi`
  (no change).
- `crates/alknet-endpoint/src/accept/tcp_tls.rs` already uses
  `from_bidi` (no change).
- The call crate's test stubs
  (`call_client.rs:91`, `protocol/connection.rs:465`,
  `protocol/dispatch.rs:465`, `protocol/adapter.rs:294`,
  `client/from_call.rs:428`) today do
  `tokio::io::split(x)` then `from_stream(send, recv, ...)` — they
  become `from_bidi(x, ...)` directly, one call, no split.
- The channels reassembly path (per ADR-074, the future
  `ChannelBidiStreamSource::accept_bi` impl) joins its
  `MpscSendStream` / `MpscRecvStream` via `tokio::io::join` and calls
  `from_bidi` — the join is in the channels crate (where the split
  exists), not in the core constructor API.
- The core test at `types.rs:768` and the `from_source_tests` helper
  become `from_bidi` calls (or construct `BiStream` directly via
  `BiStream::from_joined`).

`SendStream::from_stream` / `RecvStream::from_stream` (the per-half
constructors, `types.rs:267` / `types.rs:289`) are **retained** — they
are the per-half boxing for `into_sub_streams()` (ADR-074) and the
channels reassembly path's `SubStreamHandle` leaves, not constructors
that feed `Connection`. The split lives where it is natural (channels
reassembly → `SubStreamHandle`), doesn't leak into `Connection`'s API.

### `SendStream` / `RecvStream` collapse to thin newtypes

```rust
pub struct SendStream { inner: Box<dyn AsyncWrite + Send + Unpin> }
pub struct RecvStream { inner: Box<dyn AsyncRead + Send + Unpin> }
```

Used by `into_sub_streams()` (ADR-074) and the channels reassembly
path. No `SendStreamKind` / `RecvStreamKind` enum — the quinn/iroh
dispatch is gone, the join happens once in the `BidiStreamSource` impl.
`SendStream::from_quinn` / `from_iroh` (crate-private) become the
thin-boxing constructors used only by the channels reassembly path
when it needs to expose unidirectional sub-streams. The
`from_stream(impl AsyncWrite + Send + Unpin)` / `from_stream(impl
AsyncRead + Send + Unpin)` public constructors are retained.

### `HttpAdapter` drops `QuicStream`

```rust
async fn handle(&self, connection: Connection, auth: &AuthContext)
    -> Result<(), HandlerError>
{
    if let Some(identity) = auth.identity.clone() {
        let _ = connection.set_identity(identity);
    }
    let stream = connection.accept_bi().await
        .map_err(stream_error_to_handler)?;
    self.serve_io(stream).await   // BiStream: AsyncRead + AsyncWrite + Unpin
}
```

`QuicStream` (44 lines) and `QuicStreamDuplex` (test helper, 38 lines)
are removed. `serve_io<I: AsyncRead + AsyncWrite + Send + Unpin>` is
unchanged — `BiStream` satisfies the bounds by construction.

### WebSocket runs through `Connection::from_bidi`

`WsBidiStream` (new, ~50-80 lines) implements `AsyncRead` / `AsyncWrite`
over `axum::extract::ws::WebSocket` binary messages: `AsyncRead`
consumes `Message::Binary` payloads (text messages close with a
protocol error, matching the current `drive_ws_session` behavior);
`AsyncWrite` frames each write as a `Message::Binary`; `poll_shutdown`
emits `Message::Close`. The WS session then runs through
`Connection::from_bidi(WsBidiStream::new(socket), alpn, addr)` +
`CallAdapter::handle` (or whatever the call-protocol's
`ProtocolHandler` is at the assembly layer) — the same path as any
other transport.

The `WsStream` trait (`upgrade.rs:44-49`), the bespoke `drive_ws_session`
loop, the `handle_inbound_envelope` / `dispatch_envelope_to_pending`
helpers, and the `run_ws_session` glue are removed. The session's
wire-level invariants (binary-only, protocol-level close on text,
`fail_all` pending on disconnect, ADR-048's `EventEnvelope` framing)
move into `WsBidiStream`'s `AsyncRead` / `AsyncWrite` / `poll_shutdown`
impls and the standard call-protocol dispatch path.

`CallConnection::new_overlay_only` stays — it's the
connection-local-overlay construction for non-peer clients
(ADR-034 §4, ADR-044 §5), orthogonal to the transport seam. What
changes is that the WS session feeds it through a `Connection` rather
than a parallel `WsStream` trait.

### Channels spec updates

ADR-074's `ChannelBidiStreamSource::accept_bi` returns `BiStream`:

```rust
async fn accept_bi(&self) -> Result<BiStream, StreamError> {
    // Yields the joined (stream_type 0, stream_type 1) pair on first
    // call, ConnectionClosed on subsequent calls.
}
```

`into_sub_streams()` is unchanged in shape — it still returns
`Vec<(u8, SubStreamHandle)>` with `SubStreamHandle::Send(SendStream) |
Recv(RecvStream)`, because TTY's named sub-streams are unidirectional
(ADR-071, ADR-077). The two paths (`accept_bi` for handlers that want
the joined pair, `into_sub_streams` for handlers that want the typed
unidirectional sub-streams) are preserved per ADR-074.

The channels POC's `MpscSendStream` / `MpscRecvStream` feed
`BiStream::from_joined(send, recv)` (or `from_stream` if the channels
crate prefers to construct the joined leaf directly from the mux
handle) — the `ChannelBidiStreamSource::accept_bi` impl does the join
once, and the per-channel `Connection::accept_bi` yields a `BiStream`.
The POC's `TunnelHandler` calls `tokio::io::split(bidi)` to get its two
pump halves, the same idiom it would use over `TcpStream`.

### `BiStream` over WebSocket enables "VPN-like without being a VPN" in v1

The `webtransport.md` spec describes the "VPN-like without being a VPN"
path: a browser opens a WebTransport session to `/alknet/ssh`, the h3
handler hands each bidi stream to `SshAdapter::handle` as a
`Connection`, the browser's WASM SSH parser speaks SSH over the
stream. WebTransport is deferred per ADR-044.

With `BiStream` as the leaf, the same path exists over WebSocket in
v1: a browser opens a WS connection, `WsBidiStream` presents it as a
`BiStream`, `Connection::from_bidi` wraps it, `ChannelsAdapter::handle`
runs the channels demux over it, each channel's `accept_bi` yields a
`BiStream` that `SshAdapter::handle` receives. The WASM SSH parser
runs over a `BiStream`-over-WS-message adapter on the browser side
(the same `WsBidiStream` shape, browser-implemented). ADR-044/048's
"WS message stream is another `BiStream`-satisfying transport" becomes
literal — the code does what the spec said.

The channels POC's sync core already compiles under
`wasm32-unknown-unknown`; a `BiStream`-over-WS adapter would too. The
WASM-clean property is preserved by the unification, not blocked by
it.

### WebTransport is a channels concern, not an alknet-http concern

The `h3` handler as specified in `webtransport.md` does exactly the
channels shape: one connection, N bidi streams inside, each routed to
an ALPN by the CONNECT path. That's `ChannelsAdapter::handle` with a
different wire format (HTTP/3 extended CONNECT vs the 9-byte chunk
header). When WebTransport revives, the h3 multi-stream demux leaves
`alknet-http` and becomes a channels-variant ALPN — the `alknet-http`
h3 path becomes "register an ALPN handler that gets one `BiStream` and
serves it as HTTP/3," same as `h2`/`http/1.1`. The
ALPN-stream-proxy (ADR-040) is the channels-over-WebTransport shape,
not an `alknet-http` shape.

This is out of scope for this ADR (WebTransport is deferred per
ADR-044). It is recorded here because the unification is what makes
the future extraction clean: once `accept_bi` returns `BiStream`, the
`h3` handler's "accept a WebTransport session, yield each stream as a
`BiStream` to the ALPN handler" shape is the same code as
`ChannelsAdapter::handle`, and the extraction is a move, not a
redesign.

## What does NOT change

- **`ProtocolHandler` trait shape** — `handle(&self, connection:
  Connection, auth: &AuthContext)` stays. This is an internal
  `Connection` refactor; the handler trait is the ADR-009 one-way door.
- **`HandlerRegistry`** — unchanged.
- **`Connection::remote_alpn` / `set_identity` / `identity` / `close`**
  — unchanged. These are `Connection`-level, not transport-level.
- **`BidiStreamSource` trait** (ADR-070) — preserved. Three signatures
  change return type (`accept_bi`, `open_bi`, and the implied
  `Connection::accept_bi` / `open_bi`); the trait shape and the
  extension-point model are preserved.
- **`from_source` constructor** (ADR-070) — preserved. Downstream
  crates implement `BidiStreamSource` and construct via `from_source`;
  their `accept_bi` impls return `BiStream`.
- **`into_sub_streams()`** (ADR-074) — preserved. TTY's named
  unidirectional sub-streams are the case that justifies keeping
  `SendStream` / `RecvStream` (as thin newtypes, not quinn-welded
  enums).
- **The two-pump pattern** (ADR-078) — preserved. Tunnel/SSH handlers
  call `tokio::io::split(bidi)` for their two pump halves; the
  shutdown-on-completion contract applies to the `ReadHalf` /
  `WriteHalf` unchanged.
- **Yield-once contract** (ADR-065) — preserved.
  `StreamBidiStreamSource::accept_bi` yields the `BiStream` once then
  returns `ConnectionClosed`. The contract is about *how many times*
  `accept_bi` yields, not *what shape* it yields.
- **`Connection::from_quinn` / `from_iroh`** — preserved as
  convenience wrappers; internally wrap the `QuinnBidiStreamSource` /
  `IrohBidiStreamSource` whose `accept_bi` does the join.
- **`Connection::from_bidi`** — promoted to the only public stream
  constructor. `Connection::from_stream` is removed (the split no
  longer crosses a crate boundary as part of a constructor).
- **`SendStream::from_stream` / `RecvStream::from_stream`** (per-half
  constructors) — retained, but only as the boxing for
  `into_sub_streams()` and the channels reassembly path's
  `SubStreamHandle` leaves. Not constructors that feed `Connection`.
- **The endpoint's accept loops** (quinn/iroh) — unchanged.

## Consequences

**Positive:**

- The `QuicStream` wrapper (44 lines) and `QuicStreamDuplex` test
  helper (38 lines) are removed from `alknet-http`. `HttpAdapter::handle`
  becomes 4 lines. `serve_io`'s signature is unchanged.
- The `WsStream` trait, the bespoke `drive_ws_session` loop, and ~150
  lines of WS-specific dispatch glue are removed from
  `alknet-http/websocket/upgrade.rs`. The WS session runs through
  `Connection::from_bidi` + the call-protocol handler like any other
  transport. ADR-044/048's "WS message stream is `BiStream`-satisfying"
  becomes literal.
- One abstraction (`BiStream`) replaces five. The leaf type matches the
  ecosystem convention (`russh::Channel::into_stream()`, `TcpStream`,
  `TlsStream<TcpStream>`, `DuplexStream`).
- "VPN-like without being a VPN" over WS in v1 becomes real: the same
  path `webtransport.md` specified, over WS, now. The browser's WASM
  parser implements `BiStream` over a WS-message adapter; the server
  wraps it via `Connection::from_bidi`; `ChannelsAdapter::handle` runs
  the demux; each channel's `BiStream` reaches `SshAdapter::handle`
  unchanged.
- The quinn-welding in `SendStream` / `RecvStream` (the
  `SendStreamKind` / `RecvStreamKind` enums and their per-call
  dispatch) is gone. `SendStream` / `RecvStream` become thin newtypes
  used only by the channels reassembly path and `into_sub_streams()`.
- The future WebTransport extraction is a move (h3 demux → a
  channels-variant ALPN), not a redesign. The unification is what
  makes it clean.
- ADR-007's `BiStream` is resurrected as the actual leaf, matching the
  original intent the code never delivered.

**Negative:**

- Every `accept_bi().await` caller sees a return-type change from
  `(SendStream, RecvStream)` to `BiStream`. Callers that want the
  split pair call `tokio::io::split(bidi)`. The call-site change is
  mechanical (`let (send, recv) = ...` → `let bidi = ...; let (recv,
  send) = tokio::io::split(bidi)`), but it touches every handler. This
  is the one-time cost of the unification; the alternative is
  per-handler wrappers forever.
- Every `Connection::from_stream(send, recv, ...)` call site is
  removed. The call crate's test stubs (5 sites) become `from_bidi`
  calls. The channels reassembly path gains a one-line
  `tokio::io::join` before `from_bidi`. No caller outside core and
  the channels reassembly path was ever doing anything other than
  `tokio::io::split` then `from_stream` — the split was always
  gratuitous at the call site.
- ADR-070's `accept_bi` return shape is amended. ADR-070 explicitly
  preserved the split-pair shape to keep the `Connection` API verbatim;
  this ADR reverses that preservation. The trade is: one type change
  across the codebase now, vs. one wrapper per handler forever.
  ADR-070's trait-extraction (the one-way door) is preserved; the
  return-shape is the amended part.
- ADR-074's `ChannelBidiStreamSource::accept_bi` return shape is
  amended (same change, same rationale). `into_sub_streams()` is
  unchanged.
- `BiStream` becomes a concrete struct (with an internal boxed
  `dyn AsyncReadWrite`), not a bare trait object. This is the
  `Pin<&mut Self>` projection requirement — a bare `dyn BiStream` is
  not ergonomic for `AsyncRead` / `AsyncWrite` impls. The ADR-007
  trait is removed; the bounds survive as implied bounds on the
  concrete struct. The name and the convention are preserved; the
  shape becomes a concrete leaf.
- `WsBidiStream` is real new code (~50-80 lines). The WS-message ↔
  byte-stream adapter is the one place the unification requires
  non-trivial work — WS messages are framed, not a byte stream, so
  the adapter owns the framing. This is the same work the current
  `drive_ws_session` loop does, just relocated from a bespoke loop
  into the `AsyncRead` / `AsyncWrite` impls.
- The `alknet-http` crate gains a dependency on whatever crate
  owns `WsBidiStream` (likely `alknet-http` itself, or a small
  `alknet-ws` crate if WASM-targetability is a goal — the browser side
  needs the same adapter). This is a packaging decision, not a
  design one — recorded as an open question below.

## Door type

**One-way.** The `accept_bi` return shape is the handler-facing API
surface. Once handlers are written against `BiStream`, reversing to
the split-pair shape is a rewrite of every handler's call site. The
trade is one type change across the codebase now vs. one wrapper per
handler forever — this ADR takes the one-time cost.

The `BiStream` concrete-struct shape (internal `Box<dyn
AsyncReadWrite>`, `Pin` projection) is a two-way-door implementation
detail — the internal representation can change without breaking the
public `AsyncRead + AsyncWrite + Send + Unpin` bounds.

## Migration

The migration is mechanical and can be ordered to keep the workspace
compilable:

1. **Core: introduce `BiStream` as the concrete leaf.** Add the
   struct, the `AsyncRead` / `AsyncWrite` impls, the `from_joined`
   constructor. Change `BidiStreamSource::accept_bi` / `open_bi` return
   types to `BiStream`. Update `QuinnBidiStreamSource` /
   `IrohBidiStreamSource` / `StreamBidiStreamSource` impls to do the
   join. `Connection::accept_bi` / `open_bi` delegate verbatim. Remove
   `Connection::from_stream` (the split-pair constructor); promote
   `Connection::from_bidi` to the only public stream constructor.
   This is a single-crate change; every `accept_bi` and `from_stream`
   caller breaks mechanically.
2. **Update every handler's `accept_bi` call sites and every
   `from_stream` call site.** `HttpAdapter::handle` becomes 4 lines
   (drop `QuicStream`). `TtyAdapter::handle` calls
   `tokio::io::split(bidi)` for its pump halves (or uses
   `into_sub_streams()` in channels mode — unchanged). The channels
   POC's `TunnelHandler` and `EchoHandler` get the same
   `tokio::io::split` treatment. `CallAdapter::handle` (wherever it
   consumes `accept_bi`) gets the same. The call crate's test stubs
   (5 `from_stream` sites) become `from_bidi` calls (drop the
   `tokio::io::split` they were doing immediately before). The channels
   reassembly path gains a one-line `tokio::io::join` before `from_bidi`.
3. **Collapse `SendStream` / `RecvStream` to thin newtypes.** Remove
   `SendStreamKind` / `RecvStreamKind` enums; the quinn/iroh
   constructors become thin-boxing. Used only by the channels
   reassembly path and `into_sub_streams()`.
4. **`alknet-http`: rewrite WS through `Connection::from_bidi`.** Add
   `WsBidiStream`; remove `WsStream` trait, `drive_ws_session` loop,
   and the dispatch glue. The WS session runs through
   `Connection::from_bidi` + the call-protocol handler. This is the
   largest single change and can land after (1)-(3) — the WS path is
   independent of the handler call-site updates.
5. **Update ADR-065, ADR-070, ADR-074, ADR-077** to reflect the
   `BiStream` return shape. ADR-065's `from_stream` constructor is
   removed; `from_bidi` is the only public stream constructor (the
   rule: the split never crosses a crate boundary as part of a
   constructor). ADR-070's `accept_bi` return type is amended. ADR-074's
   `ChannelBidiStreamSource::accept_bi` return type is amended;
   `into_sub_streams()` is unchanged. ADR-077's two-mode TTY design is
   unchanged (the modes differ in *how* the adapter gets sub-streams,
   not in the leaf type).
6. **Update `findings.md` Phase 6.** The "deferred" status is
   replaced: the `QuicStream` wrapper is removed (not because
   `accept_bi` returns streams that are already duplex, but because
   `accept_bi` now returns a `BiStream`); the WS path is unified; the
   h3/WebTransport extraction is recorded as a future channels-variant
   move enabled by this ADR.

The Phase 6 deferral in `findings.md` is resolved by this ADR — not by
the original plan (drop the wrapper as redundant) but by the actual
fix (unify the leaf so the wrapper moves into core).

## Open questions

- **Where does `WsBidiStream` live?** If WASM-targetability is a goal
  (the browser side needs the same adapter), it may want to live
  somewhere a WASM client can reach — `alknet-core` (no, HTTP deps
  don't belong in core), a small `alknet-ws` crate, or
  `alknet-http` with the browser-side adapter extracted separately.
  Default: `alknet-http` owns the server-side `WsBidiStream`; the
  browser-side adapter is a separate concern (the WASM SDK, not
  alknet-http). Resolved at implementation time.
- **`SendStream` / `RecvStream` long-term home.** With the quinn
  enums gone, these are thin newtypes over
  `Box<dyn Async* + Send + Unpin>`. They could move out of
  `alknet-core` into `alknet-channels-core` (their only consumer is
  `into_sub_streams()`). Default: stay in `alknet-core` for now (the
  channels crate is not yet extracted); revisit at the channels
  extraction.

## References

- ADR-007: `BiStream` type definition (resurrected by this ADR — the
  trait is removed, the bounds survive as implied bounds on the
  concrete struct)
- ADR-065: `Connection::from_stream` / `from_bidi` (amended —
  `from_stream` is removed; `from_bidi` is the only public stream
  constructor; the split never crosses a crate boundary as part of a
  constructor)
- ADR-070: `BidiStreamSource` trait (amended — `accept_bi` / `open_bi`
  return `BiStream`, not the split pair; the trait shape and the
  `from_source` extension point are preserved)
- ADR-074: `ChannelBidiStreamSource` (amended — `accept_bi` returns
  `BiStream`; `into_sub_streams()` is unchanged)
- ADR-077: TTY inside channels (unchanged — the two-mode design is
  preserved; the modes differ in how the adapter gets sub-streams,
  not in the leaf type)
- ADR-078: two-pump shutdown-on-completion (unchanged — the contract
  applies to `tokio::io::split(bidi)` halves)
- ADR-044, ADR-048: WebSocket is the v1 browser bidirectional path
  (this ADR makes the "WS message stream is `BiStream`-satisfying"
  claim literal)
- `docs/research/alknet-crate-extraction/findings.md` Phase 6 — the
  deferred `alknet-http` rework; this ADR resolves the deferral by
  unifying the leaf rather than by dropping the wrapper as redundant
- `docs/architecture/crates/http/webtransport.md` — the deferred h3
  handler; this ADR records the future extraction as a
  channels-variant move, enabled by the unification
- `crates/alknet-core/src/types.rs:226` — the vestigial `BiStream`
  trait this ADR resurrects as the concrete leaf
- `crates/alknet-http/src/server/adapter.rs:271-314` — the `QuicStream`
  wrapper this ADR removes
- `crates/alknet-http/src/websocket/upgrade.rs:44-49` — the `WsStream`
  trait this ADR removes
- `russh::Channel::into_stream()` — the ecosystem convention this ADR
  aligns with
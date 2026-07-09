# ADR-065: Connection::from_stream — Generic Single-Stream Connections

## Status

Accepted

## Context

ADR-007 defines `Connection` as a concrete type wrapping a QUIC connection
(quinn or iroh). ADR-010 establishes the endpoint as a multi-connectivity
QUIC acceptor — quinn and iroh, both producing QUIC connections dispatched by
ALPN. The `ProtocolHandler` trait (ADR-002) receives a `Connection`, and
handlers call `accept_bi()` / `open_bi()` to get bidirectional streams.

This design is **welded to QUIC**. Both real `ConnectionKind` variants
(`Quinn`, `Iroh`) are QUIC. The `HttpAdapter::handle` method calls
`connection.accept_bi().await` to get a bidi stream and serves HTTP over it
— "HTTP over QUIC," not "HTTP over TCP+TLS." There is no way to serve the
standard HTTP interface that `api.alk.dev` (an external app being built
against the crates) requires without either bypassing the
`HandlerRegistry` (a parallel listener, defeating the ALPN-router design)
or generalizing `Connection` to accept a non-QUIC stream.

The same welding blocks `alknet-ssh` (needs to dispatch SSH channels —
each channel is a read/write pair — through the same `HandlerRegistry` as
QUIC connections) and WebTransport stream dispatch (each WT stream is a
read/write pair). The `TtyAdapter` and `CallAdapter` dispatch loops are
already transport-agnostic in their inner logic — only the
`connection.accept_bi()` call is QUIC-coupled, because `accept_bi` only
works when `Connection` is QUIC-kind.

### The yield-once contract composes

QUIC's `accept_bi` returns a new bidi stream per call (many). A generic
single-stream connection's `accept_bi` returns the underlying stream on the
first call, then `ConnectionClosed` on all subsequent calls. This is the
contract that makes the abstraction compose:

- Handlers that loop `accept_bi` (TtyAdapter) get one session per
  single-stream connection — the loop body runs once, then
  `ConnectionClosed` breaks the loop. Correct.
- Handlers that call `accept_bi` once (HttpAdapter) get the stream
  directly. Correct.

No branching on transport. The handler code is unchanged across QUIC
(many streams) and TCP+TLS / SSH channels / WebTransport streams (one
stream). The `ProtocolHandler` trait shape is not touched — this is an
additive change to `Connection`, not a trait revision.

### The stream-level Mock variants were already generic

`SendStreamKind::Mock(Box<dyn AsyncWrite>)` and `RecvStreamKind::Mock(Box<dyn
AsyncRead>)` were already generic stream holders — the name was wrong
(carried over from a test-only context). The generalization renames them
to `Stream` and makes them load-bearing: `Connection::from_stream` calls
`SendStream::from_stream` / `RecvStream::from_stream` to wrap the halves of
the single stream.

### The connection-level Mock is removed

The findings doc (`docs/research/transport-generalization/findings.md`)
proposed keeping `ConnectionKind::Mock` / `MockConnection` for test-only
full-connection mocks. The implementation went further: `MockConnection`
and `ConnectionKind::Mock` are removed entirely. Test stubs that
previously used `Connection::from_mock(Arc<StubConnection>)` now use
`Connection::from_stream(tokio::io::sink(), tokio::io::empty(), alpn,
addr)` — `tokio::io::empty()` yields immediate EOF on the read side,
causing the handler's `handle_stream` to exit cleanly, and `accept_bi`
returns `ConnectionClosed` after the first take (driving the run loop to
exit). This is simpler (one connection kind, not two) and the test stubs
are shorter. `from_stream` subsumes the test-mock use case because a test
connection is just a single-stream connection with EOF-on-read.

### Why not change the ProtocolHandler trait

An earlier analysis proposed changing `ProtocolHandler::handle` to take a
single `Channel` instead of a `Connection`, moving the multiplexing loop
from the handler to the endpoint. This ADR does **not** do that:

1. **TtyAdapter already establishes the pattern.** The handler loops
   `accept_bi` and dispatches each stream internally. SSH does the same —
   parse channels, dispatch each. The multiplexing loop belongs in the
   handler, not the endpoint.
2. **The trait shape is a one-way door (ADR-009).** Changing
   `handle(Connection)` → `handle(Channel)` would require migrating every
   handler and would lock in a specific multiplexing model. `from_stream`
   is additive — it extends `Connection` without touching the trait. If a
   trait change is ever warranted, it can come later; `from_stream` doesn't
   preclude it.

See `docs/research/transport-generalization/findings.md` §6 for the full
argument against the trait shape change.

## Decision

### Add `ConnectionKind::Stream`

A new variant holding a single read/write pair behind a
`Mutex<Option<(SendStream, RecvStream)>>` — the yield-once semantic. No
feature gate (generic, no transport deps). `StreamConn` is always
available; the quinn/iroh variants remain feature-gated.

### Add `Connection::from_stream` and `Connection::from_bidi`

```rust
/// Construct a Connection from a pre-split read/write pair.
/// `accept_bi()` yields this pair once, then returns `ConnectionClosed`.
/// `open_bi()` returns `StreamClosed` (a single stream can't open new streams).
pub fn from_stream(
    send: impl AsyncWrite + Send + Unpin + 'static,
    recv: impl AsyncRead + Send + Unpin + 'static,
    alpn: Vec<u8>,
    remote_addr: Option<SocketAddr>,
) -> Self;

/// Convenience for a single bidirectional stream (e.g. TlsStream<TcpStream>).
/// Splits internally via tokio::io::split.
pub fn from_bidi(
    stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
    alpn: Vec<u8>,
    remote_addr: Option<SocketAddr>,
) -> Self;
```

### Make `accept_bi`'s yield-once contract explicit

The `accept_bi` doc comment now states the transport semantics: QUIC yields
many streams, single-stream yields once then `ConnectionClosed`. This is
the contract that makes the abstraction compose — handlers don't branch
on transport.

### Rename stream-level `Mock` → `Stream`

`SendStreamKind::Mock` → `SendStreamKind::Stream`,
`RecvStreamKind::Mock` → `RecvStreamKind::Stream`.
`SendStream::from_mock` → `from_stream`, `RecvStream::from_mock` →
`from_stream`. The variants were already generic stream holders; the name
was wrong. Drop the `#[allow(dead_code)]` — `from_stream` is now
load-bearing.

### Remove `MockConnection` / `ConnectionKind::Mock`

The connection-level test mock trait and variant are removed. Test stubs
use `Connection::from_stream` with `tokio::io::sink()` / `tokio::io::empty()`
(immediate EOF on read → handler exits cleanly → `accept_bi` returns
`ConnectionClosed` → run loop exits). One connection kind for both
production and tests, not two.

### `open_bi` on `Stream` returns `StreamClosed`

A single stream cannot open new application streams. `open_bi` on
`ConnectionKind::Stream` returns `StreamError::StreamClosed`. Handlers that
call `open_bi` (the call protocol's server→client direction) work over
QUIC but not over a single-stream connection — this is inherent to the
transport, not a flaw. A handler that needs `open_bi` should not be
dispatched over a single-stream connection (or should multiplex its own
sub-streams within the one stream, as the call protocol does over a single
WebTransport stream).

### What does NOT change

- `ProtocolHandler` trait shape — `handle(&self, connection: Connection,
  auth: &AuthContext)` stays. This is an additive change to `Connection`,
  not a trait revision (ADR-009: the trait is a one-way door).
- `HandlerRegistry` — unchanged.
- All handler code (`HttpAdapter`, `TtyAdapter`, `CallAdapter`) —
  unchanged. `HttpAdapter` is one `accept_bi` call away from
  transport-agnostic (it already is — the call works over `from_stream`).
- `BiStream` trait — unchanged (ADR-007). `from_stream` is a server-side
  connection constructor; `BiStream` is a client-side/test convenience
  trait. They're complementary, not competing.
- The endpoint's accept loops (quinn/iroh) — unchanged. The TCP+TLS accept
  loop that *uses* `from_stream` is a follow-up, not this ADR.

## Consequences

**Positive:**
- Every existing `ProtocolHandler` works over TCP+TLS, SSH channels,
  WebTransport streams, and wasm streams **unchanged** — dispatch through
  the same `HandlerRegistry` by ALPN string, no handler code changes.
- `api.alk.dev`'s HTTP blocker is resolved: a TCP+TLS accept loop can call
  `Connection::from_bidi(tls_stream, alpn, remote_addr)` and dispatch
  through `HandlerRegistry` — `HttpAdapter` works unchanged over the
  single stream. (The accept loop itself is a follow-up commit; the
  primitive it needs is now in place.)
- `alknet-ssh` is unblocked: the SSH handler wraps each russh channel via
  `from_stream` and dispatches by channel-type (treated as the ALPN string)
  through `HandlerRegistry`. One SSH connection carries heterogeneous
  channels (`alknet/tty`, `alknet/call`, `h2`, ...) — a multiplexing power
  QUIC's per-connection ALPN doesn't give natively.
- WebTransport stream dispatch is unblocked: the WT handler wraps each WT
  stream via `from_stream` and dispatches through `HandlerRegistry` (the
  primitive exists; WT itself is parked per ADR-044).
- The server-side WASM door (OQ-09) is no longer closed by `Connection`
  being QUIC-bound — `from_stream` accepts any `AsyncRead + AsyncWrite`,
  including wasm-compatible streams. (The accept-loop runtime remains
  tokio-bound; the *connection* door is now open.)
- One connection kind for production and tests (no `MockConnection`
  trait) — simpler type, shorter test stubs.
- No new deps, no `Cargo.toml` change — `tokio::io::split` is already
  available via the existing tokio dep.

**Negative:**
- `open_bi` on a single-stream connection returns `StreamClosed` —
  handlers that need server→client stream initiation (the call protocol's
  bidirectional call direction) don't work over a single-stream
  connection. This is inherent to the transport, not a design flaw: a
  single TCP+TLS stream is not a multiplexed transport. Handlers that need
  `open_bi` should run over QUIC, or multiplex their own sub-streams within
  the one stream (as the call protocol does over a single WebTransport
  stream — the `EventEnvelope` framing is stream-agnostic, ADR-012).
- The `close()` method's `code`/`reason` args are QUIC-specific
  (application-level close codes). For a raw stream they're ignored — the
  drop is the close. This is the same best-effort semantic `close` already
  had for the removed `Mock` variant.
- A `Mutex` on the `StreamConn` — a single lock per `accept_bi` / `close`
  call. Negligible cost (one `take()`), but it is a lock where the QUIC
  variants have none.

## References

- ADR-002: ProtocolHandler trait (unchanged by this ADR)
- ADR-007: BiStream type definition (amended by this ADR — `Connection` is
  no longer QUIC-only; the server-side WASM door is open)
- ADR-009: One-way door decision framework (why the trait shape is not
  changed — `from_stream` is additive)
- ADR-010: ALPN router and endpoint (amended by this ADR — "TCP is not an
  endpoint concern" is revised; `from_stream` lets TCP+TLS participate in
  ALPN dispatch via a handler-internal accept loop)
- ADR-012: Call protocol stream model (the `EventEnvelope` framing is
  stream-agnostic — composes over `from_stream`)
- OQ-09: WASM target boundaries (resolution amended — the server-side
  dispatch door is no longer closed by `Connection` being QUIC-bound)
- Transport generalization findings:
  [`docs/research/transport-generalization/findings.md`](../../research/transport-generalization/findings.md)
- Implementation commit: `865fef6` (2026-07-09)
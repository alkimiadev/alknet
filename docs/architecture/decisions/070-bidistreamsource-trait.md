# ADR-070: BidiStreamSource Trait — Open Connection for Extension

## Status

Accepted

## Context

ADR-065 generalized `Connection` beyond QUIC by adding
`ConnectionKind::Stream` (a yield-once read/write pair) and the
`Connection::from_stream` / `from_bidi` constructors. That closed the
server-side "QUIC-only" gap: TCP+TLS, SSH channels, WebTransport streams,
and wasm streams now dispatch through the same `HandlerRegistry` as QUIC
connections, unchanged.

What ADR-065 did **not** change is the *shape* of `Connection` itself. It
remains a closed enum:

```rust
enum ConnectionKind {
    #[cfg(feature = "quinn")] Quinn(quinn::Connection),
    #[cfg(feature = "iroh")]  Iroh(iroh::endpoint::Connection),
    Stream(StreamConn),   // yield-once — ADR-065
}
```

Adding a new connection type today requires editing `alknet-core` — adding a
variant to `ConnectionKind`, adding match arms to `accept_bi` / `open_bi` /
`remote_addr` / `close`. Every downstream crate that introduces a new
connection shape (channels, a future transport, a test double beyond the
`from_stream` case) forces a core change. `Connection` is closed for
extension.

### The channels crate is the first crate that needs to extend it

The `alknet-channels` POC (`docs/research/alknet-channels/poc-summary.md`)
validated the channels multiplexer and surfaced the concrete blocker. A
channels connection carries N logical channels over one transport stream;
each channel is a bidirectional byte stream presented to a `ProtocolHandler`
as a `Connection`. With the ADR-065 shape, each channel becomes a fresh
yield-once `Connection::from_stream`, and the channels endpoint holds a *bag*
of these connections (one per channel) rather than one `ChannelConnection`
that yields N streams.

The POC confirmed this is *sufficient* (the yield-once path works — handlers
run unchanged) but *awkward*: the channels layer wants to expose a single
`ChannelConnection` that is a first-class peer of QUIC (many bidi streams),
not a collection of yield-once `Connection`s. The clean shape is for the
channels crate to implement the stream-yield interface itself, in its own
crate, without a core edit.

### The extension point is narrow and already implied by ADR-065

`Connection`'s public surface is four operations: `accept_bi`, `open_bi`,
`remote_addr`, `close`. `remote_alpn` / `set_identity` / `identity` are
`Connection`-level (not transport-level) and stay on `Connection` itself.
The four transport-level operations are the seam. Extracting them into a
trait that downstream crates can implement turns `Connection` from a closed
enum into an open trait object — the same extensibility `ProtocolHandler`
already gives handlers, applied to the connection.

### What the POC de-risked

The channels POC (28 tests passing) validated that:

1. The yield-once `Connection::from_stream` path is sufficient for per-channel
   presentation — an echo `ProtocolHandler` runs through the full
   demux→Connection→handler→mux path with zero channels-layer awareness
   (`poc-summary.md` §"POC Target 2").
2. The `BidiStreamSource` trait is **additive** — existing callers keep working
   via a `from_stream`-backed implementation of the trait, and the trait
   cleanly supports a `ChannelConnection` that yields N streams
   (`poc-summary.md` §"Issues Surfaced" #1).
3. The trait does not touch the `ProtocolHandler` trait shape (ADR-002) —
   handlers continue to receive a `Connection` and call `accept_bi` /
   `open_bi` on it. This is a `Connection` internal refactor, not a handler
   API change (`poc-summary.md` §"POC Target 2").

The remaining unknowns are spec-scope (the channels crate's API), not
feasibility. This ADR makes the core-side extension point available so the
channels spec can build on it.

## Decision

### Extract `BidiStreamSource` trait

```rust
#[async_trait]
pub trait BidiStreamSource: Send + Sync + 'static {
    /// Yield the next bidirectional stream this connection provides.
    ///
    /// Transport semantics (carried from ADR-065):
    /// - QUIC (quinn/iroh): returns a new bidi stream on each call,
    ///   `ConnectionClosed` when the underlying connection closes.
    /// - Single-stream (TCP+TLS, SSH channel, WebTransport stream, wasm):
    ///   yields the underlying stream on the first call, then
    ///   `ConnectionClosed` on all subsequent calls.
    /// - Channels: yields one bidi stream per channel, `ConnectionClosed`
    ///   when the channels connection closes.
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;

    /// Open a bidirectional stream to the peer.
    ///
    /// Single-stream sources return `StreamClosed` (a single stream cannot
    /// open new application streams — ADR-065). QUIC and channels sources
    /// open new streams.
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;

    /// The peer's address, if available. Informational (NAT/proxy).
    fn remote_addr(&self) -> Option<SocketAddr>;

    /// Close the connection. The `code`/`reason` args are QUIC application-
    /// level close codes; non-QUIC sources ignore them (the drop is the
    /// close — ADR-065 §"Negative"). See REQ-CORE-02 below for the
    /// rationale for keeping the QUIC-shaped signature on the trait.
    fn close(&self, code: u32, reason: &str);
}
```

### `Connection` holds `Box<dyn BidiStreamSource>`

```rust
pub struct Connection {
    source: Box<dyn BidiStreamSource>,
    alpn: Vec<u8>,
    identity: OnceLock<Identity>,
}
```

`ConnectionKind` (the private enum) is replaced by the trait object. The
public `Connection` API (`accept_bi`, `open_bi`, `remote_alpn`, `remote_addr`,
`close`, `set_identity`, `identity`) is preserved verbatim — each method
delegates to `self.source`. `remote_alpn` reads `self.alpn` (unchanged).
`set_identity` / `identity` read/write `self.identity` (unchanged).

### Constructors stay; each wraps a `BidiStreamSource` impl

| Constructor | Wraps |
|-------------|-------|
| `from_quinn` / `from_quinn_with_alpn` (feature `quinn`) | `QuinnBidiStreamSource` (crate-private) |
| `from_iroh` (feature `iroh`) | `IrohBidiStreamSource` (crate-private) |
| `from_stream` / `from_bidi` (no feature gate) | `StreamBidiStreamSource` (crate-private, yield-once) |
| `from_source` (no feature gate) | caller-supplied `impl BidiStreamSource` — the extension point for downstream crates |

`from_source(source: impl BidiStreamSource, alpn: Vec<u8>) -> Self` is the
constructor that makes the trait the extension point. A downstream crate
implements `BidiStreamSource` (e.g. the channels crate's
`ChannelBidiStreamSource`) and constructs a `Connection` from it via
`from_source` — no core edit. The built-in impls (`QuinnBidiStreamSource`,
`IrohBidiStreamSource`, `StreamBidiStreamSource`) are crate-private;
`from_source` is the only path a downstream crate uses to wrap its own
impl. The `from_quinn` / `from_iroh` / `from_stream` / `from_bidi`
constructors are convenience wrappers for the three built-in impls.

The `Stream`-backend implementations are crate-private; downstream crates
do not implement `BidiStreamSource` by wrapping `from_stream`. They
implement the trait directly (channels: `ChannelBidiStreamSource`) and
construct the `Connection` via `from_source`.

### `from_stream`-backed default impl is the compatibility path

The yield-once `StreamBidiStreamSource` is the implementation that keeps
existing callers working: `Connection::from_stream(send, recv, alpn, addr)`
constructs a `Connection` backed by a `StreamBidiStreamSource` whose
`accept_bi` yields once then returns `ConnectionClosed`, whose `open_bi`
returns `StreamClosed`, whose `close` drops the stream. Behaviorally identical
to the ADR-065 `ConnectionKind::Stream` variant. No caller change.

### REQ-CORE-02: `close()` keeps the QUIC-shaped signature on the trait

The `close(&self, code: u32, reason: &str)` signature is preserved on the
trait, rather than being split into transport-specific close methods. This
resolves the ADR-065 leftover: the `Stream` backend's `close(code, reason)`
currently takes both args and uses neither, which clippy flags under
`--no-default-features` (the channels POC's build mode) as two unused
variable warnings on `crates/alknet-core/src/types.rs:500`.

Two options were considered:

- **(a) Split `close`**: `trait BidiStreamSource { fn close(&self); }` plus a
  separate `fn close_with_code(&self, code: u32, reason: &str)` default-
  implemented to call `close()`. Non-QUIC impls implement only `close()`;
  QUIC impls override `close_with_code`. This moves the QUIC-shaped args off
  the common method.
- **(b) Keep the QUIC-shaped signature on the trait**: `fn close(&self, code:
  u32, reason: &str)`. Non-QUIC impls prefix the args with `_` and document
  why they're ignored (the drop is the close — ADR-065). The trait method
  matches the existing public `Connection::close` signature verbatim — no
  caller change, no `Connection` API split.

**Decision: (b).** Rationale:

1. **No caller breakage.** `Connection::close(code, reason)` is the existing
   public signature; every caller passes both args. Option (a) would force
   either a `Connection::close` that *always* takes `code`/`reason` and
   dispatches to the right trait method (which means the trait still has the
   QUIC-shaped method, just renamed — no actual improvement), or a
   `Connection::close` that drops the args (which breaks every caller).
2. **The args are not QUIC-only in principle.** WebTransport has
   application-level close codes; a future transport may as well. The
   signature `close(code, reason)` is a reasonable "close with diagnostic"
   shape that multiple transports can use. Only raw-stream backends (the
   ADR-065 `Stream` case) have nothing to do with the args, and they're the
   degenerate case.
3. **The clippy warning is fixed by the trait, not by renaming.** Under the
   trait, the `StreamBidiStreamSource::close` impl prefixes the args with
   `_code`/`_reason` and carries a doc comment stating they're ignored
   because the drop is the close. The warning disappears; the signature
   matches the public API.

The trait method's doc comment carries the "QUIC application-level close
codes; non-QUIC sources ignore them" note from ADR-065, so implementers know
the args are optional for their transport.

### What does NOT change

- **`ProtocolHandler` trait shape** — `handle(&self, connection: Connection,
  auth: &AuthContext)` stays. This is an internal `Connection` refactor, not
  a handler API change (ADR-009: the handler trait is a one-way door).
- **`HandlerRegistry`** — unchanged.
- **All handler code** (`HttpAdapter`, `TtyAdapter`, `CallAdapter`,
  `ChannelsAdapter`) — unchanged. They receive a `Connection` and call
  `accept_bi` / `open_bi` on it. The dispatch through `Box<dyn
  BidiStreamSource>` is transparent to them.
- **`SendStream` / `RecvStream`** — unchanged. They continue to wrap
  quinn/iroh/generic-stream sources via their own internal enum dispatch.
  `BidiStreamSource` implementations construct `SendStream` / `RecvStream`
  via the existing `from_quinn` / `from_iroh` / `from_stream` constructors.
- **`BiStream` trait** — unchanged (ADR-007). `BidiStreamSource` is the
  server-side / connection-level seam; `BiStream` is a client-side / test
  convenience trait. Complementary, not competing.
- **The endpoint's accept loops** (quinn/iroh) — unchanged. They construct
  `Connection::from_quinn` / `from_iroh`, which now internally wrap a
  `QuinnBidiStreamSource` / `IrohBidiStreamSource`. The accept loops
  themselves don't touch the trait.
- **`Connection::remote_alpn` / `set_identity` / `identity`** — unchanged.
  These are `Connection`-level (the `alpn` field and the `identity` OnceLock),
  not transport-level. They stay on `Connection` and do not appear on
  `BidiStreamSource`.

## Consequences

**Positive:**

- `Connection` is open for extension. The channels crate implements
  `ChannelBidiStreamSource` in its own crate and constructs `Connection`
  from it via `from_source` — no core edit. A future transport, test
  double, or relay connection follows the same path. This is the
  structural payoff: the connection type is no longer a closed enum that
  every new connection shape must edit.
- A channels connection is a first-class peer of QUIC: one
  `ChannelConnection` that yields N streams, rather than a bag of yield-
  once `Connection`s. The channels layer's API matches its actual shape.
- The ADR-065 leftover clippy warning (unused `code`/`reason` on the
  `Stream` backend under `--no-default-features`) is resolved — the
  `StreamBidiStreamSource::close` impl documents why the args are ignored,
  and the `_` prefix is intentional, not a missing fix.
- Existing callers, handlers, and tests are unchanged. The public
  `Connection` API is preserved verbatim; the refactor is internal.
- No new deps. `async_trait` is already a core dep (used by
  `ProtocolHandler`).

**Negative:**

- One dyn-dispatch indirection per `accept_bi` / `open_bi` / `close` /
  `remote_addr` call. The previous enum match was also a branch, so the cost
  is roughly one `Box<dyn>` method call per stream operation — negligible
  next to the async I/O those operations perform. The `alpn` / `identity`
  fields stay on `Connection` (not behind the dyn), so `remote_alpn` /
  `set_identity` / `identity` have no new indirection.
- `BidiStreamSource: Send + Sync + 'static` is object-safe. This constrains
  implementations to `Send + Sync + 'static`, matching `ProtocolHandler` —
  consistent with the existing handler model.
- The `Box<dyn BidiStreamSource>` is one allocation per `Connection`. The
  enum was stack-allocated (except the `StreamConn`'s inner `Mutex`).
  Negligible per-connection cost; only matters if connections are
  constructed in a hot loop, which they are not.

## References

- ADR-002: ProtocolHandler trait (unchanged by this ADR)
- ADR-007: BiStream type definition (amended by ADR-065; this ADR does not
  touch `BiStream`)
- ADR-009: One-way door decision framework (why `ProtocolHandler` is not
  changed — this ADR is additive to `Connection`, not a trait revision)
- ADR-010: ALPN router and endpoint (the endpoint constructs `Connection`s
  via `from_quinn` / `from_iroh`; those now wrap a `BidiStreamSource` impl,
  transparently)
- ADR-065: `Connection::from_stream` — generic single-stream (this ADR
  generalizes `Connection` to hold a trait object; the `from_stream` /
  `from_bidi` constructors and the yield-once contract are preserved via
  `StreamBidiStreamSource`)
- Channels POC summary:
  [`docs/research/alknet-channels/poc-summary.md`](../../research/alknet-channels/poc-summary.md)
  §"Issues Surfaced" #1 (OQ-CH-13 confirmed +EV), #2 (REQ-CORE-02)
- Channels Phase 0 findings:
  [`docs/research/alknet-channels/phase-0-findings.md`](../../research/alknet-channels/phase-0-findings.md)
  §POC-Validated Requirements — REQ-CORE-01, REQ-CORE-02
---
id: core/connection-from-source-constructor
name: "Add Connection::from_source constructor (ADR-070 gap — the public extension point)"
status: pending
depends_on: []
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

ADR-070 made `Connection` hold `Box<dyn BidiStreamSource>` so downstream
crates can add connection shapes without editing core. The trait exists,
the three built-in impls exist, but the **public constructor that lets a
downstream crate construct a `Connection` from its own `BidiStreamSource`
impl is missing.** The `source` field is private; there is no way for
the channels crate (or any future crate) to build a `Connection` from a
`ChannelBidiStreamSource` — the trait is currently unusable from outside
core.

This is a gap in the ADR-070 implementation, not a new architectural
decision. ADR-070 says "the channels crate implements
`ChannelBidiStreamSource` in its own crate and constructs `Connection`
from it" (§Consequences), but the constructor for that path was never
added. The alknet-channels POC update surfaced this when it tried to use
the trait directly and found no `from_source`.

### Current state (`crates/alknet-core/src/types.rs`)

```rust
pub struct Connection {
    source: Box<dyn BidiStreamSource>,  // private
    alpn: Vec<u8>,
    identity: OnceLock<Identity>,
}

impl Connection {
    // from_quinn, from_quinn_with_alpn, from_iroh, from_stream, from_bidi
    // — all wrap crate-private impls. No constructor takes a
    // caller-supplied BidiStreamSource.
}
```

### Target state

```rust
impl Connection {
    /// Construct from a caller-supplied `BidiStreamSource` impl. The
    /// extension point for downstream crates — implement the trait and
    /// construct a `Connection` from it without editing core. See ADR-070.
    pub fn from_source(source: impl BidiStreamSource, alpn: Vec<u8>) -> Self {
        Self {
            source: Box::new(source),
            alpn,
            identity: OnceLock::new(),
        }
    }
}
```

The signature takes `impl BidiStreamSource` (not `Box<dyn
BidiStreamSource>`) to match the ergonomic style of `from_stream` /
`from_bidi` (which take `impl AsyncWrite + Send + Unpin + 'static`). The
trait already requires `Send + Sync + 'static`, so no extra bounds are
needed. The `Box::new(source)` is an internal implementation detail.

### Why not just use `from_stream`

`from_stream` wraps a `StreamBidiStreamSource` (yield-once). A downstream
crate that implements `BidiStreamSource` for a multi-stream source (e.g.
channels' `ChannelBidiStreamSource` that yields one stream per channel)
cannot use `from_stream` — it needs `from_source` to wrap its own impl.
Using `from_stream` for a multi-stream source would give yield-once
behavior, which is wrong. This is exactly the gap the channels POC hit.

### Test

Add a test that constructs a `Connection` from a minimal custom
`BidiStreamSource` impl (a mock that yields a fixed `SendStream` /
`RecvStream` pair) and verifies `accept_bi` / `open_bi` / `remote_addr` /
`close` delegate correctly. The test impl does not need to be realistic —
it just needs to prove the public constructor works end-to-end with a
non-built-in `BidiStreamSource`.

```rust
#[cfg(test)]
mod from_source_tests {
    use super::*;

    struct MockSource {
        stream: Option<(SendStream, RecvStream)>,
        addr: Option<SocketAddr>,
    }

    #[async_trait]
    impl BidiStreamSource for MockSource {
        async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
            // yield-once mock
        }
        async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
            Err(StreamError::StreamClosed)
        }
        fn remote_addr(&self) -> Option<SocketAddr> { self.addr }
        fn close(&self, _code: u32, _reason: &str) {}
    }

    #[tokio::test]
    async fn from_source_delegates_to_custom_impl() {
        let conn = Connection::from_source(
            MockSource { stream: None, addr: None },
            b"alknet/test".to_vec(),
        );
        assert_eq!(conn.remote_alpn(), b"alknet/test");
        assert_eq!(conn.remote_addr(), None);
        // accept_bi delegates to MockSource::accept_bi
    }
}
```

(The test shape is illustrative — the implementer should make the mock
yield a real `SendStream`/`RecvStream` pair via `tokio::io::duplex` so
`accept_bi` returns a usable stream, and verify the stream round-trips.)

## Acceptance Criteria

- [ ] `Connection::from_source(source: impl BidiStreamSource, alpn: Vec<u8>) -> Self` added to `types.rs`
- [ ] `from_source` is `pub` and has no feature gate (always available, like `from_stream`)
- [ ] `from_source` boxes the source internally: `source: Box::new(source)`
- [ ] `from_source` initializes `alpn` and `identity: OnceLock::new()` (same as the other constructors)
- [ ] Unit test: construct a `Connection` from a custom `BidiStreamSource` impl via `from_source`, verify `remote_alpn` / `remote_addr` / `accept_bi` / `open_bi` / `close` all delegate to the custom impl
- [ ] Existing tests pass unchanged (`from_quinn` / `from_iroh` / `from_stream` paths are not affected)
- [ ] `cargo test -p alknet-core` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings (all feature combos)
- [ ] `cargo fmt --check -p alknet-core` passes

## References

- docs/architecture/decisions/070-bidistreamsource-trait.md — ADR-070 (§Constructors, §Consequences — the "constructs `Connection` from it" clause that requires this constructor)
- docs/architecture/crates/core/core-types.md — `Connection` impl block (updated to include `from_source`), BidiStreamSource section, built-in implementations table

## Notes

> This is a gap fix, not a new feature. The ADR-070 design always intended
> the trait to be the extension point for downstream crates; the
> constructor is how a downstream crate *uses* that extension point. Without
> it, the trait is unusable from outside core — the channels crate cannot
> build a `Connection` from `ChannelBidiStreamSource`, which is the entire
> reason ADR-070 exists. The fix is one constructor (~6 lines) + one test.
> The risk is low: `from_source` is additive, the existing constructors
> are untouched, and the constructor's body is the same field-init pattern
> the other four constructors already use.
---
id: core/bidistreamsource-trait
name: "Implement BidiStreamSource trait + AuthContext::anonymous (ADR-070, REQ-CORE-01/02/03)"
status: pending
depends_on: []
scope: broad
risk: medium
impact: component
level: implementation
---

## Description

Land the three core-crate changes surfaced by the alknet-channels POC
(`docs/research/alknet-channels/poc-summary.md` §"Issues Surfaced" #1-3).
The architecture specs and ADR-070 are already written; this task is the
implementation.

### Part 1: `BidiStreamSource` trait (REQ-CORE-01, ADR-070)

Extract the stream-yield operations from `Connection` into a trait so
downstream crates (channels, future transports) can add connection shapes
without editing `alknet-core`. `Connection` goes from a closed enum
(`ConnectionKind: Quinn | Iroh | Stream`) to holding
`Box<dyn BidiStreamSource>`. The public `Connection` API is preserved
verbatim — this is an internal refactor, not a handler-facing change.

#### Current state (`crates/alknet-core/src/types.rs`)

```rust
enum ConnectionKind {
    #[cfg(feature = "quinn")] Quinn(quinn::Connection),
    #[cfg(feature = "iroh")]  Iroh(iroh::endpoint::Connection),
    Stream(StreamConn),
}

struct StreamConn {
    stream: Mutex<Option<(SendStream, RecvStream)>>,
    remote_addr: Option<SocketAddr>,
}

pub struct Connection {
    kind: ConnectionKind,
    alpn: Vec<u8>,
    identity: OnceLock<Identity>,
}
```

`accept_bi`, `open_bi`, `remote_addr`, `close` all match on `self.kind`.

#### Target state (ADR-070)

```rust
#[async_trait]
pub trait BidiStreamSource: Send + Sync + 'static {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    fn remote_addr(&self) -> Option<SocketAddr>;
    fn close(&self, code: u32, reason: &str);
}

pub struct Connection {
    source: Box<dyn BidiStreamSource>,
    alpn: Vec<u8>,
    identity: OnceLock<Identity>,
}
```

`Connection::accept_bi` / `open_bi` / `remote_addr` / `close` delegate to
`self.source`. `remote_alpn` reads `self.alpn` (unchanged). `set_identity` /
`identity` read/write `self.identity` (unchanged).

#### Crate-private implementations

Three impls, each wrapping an existing constructor:

| Impl | Constructor(s) | Yield semantics |
|------|----------------|-----------------|
| `QuinnBidiStreamSource` | `from_quinn` / `from_quinn_with_alpn` (feature `quinn`) | many streams |
| `IrohBidiStreamSource` | `from_iroh` (feature `iroh`) | many streams |
| `StreamBidiStreamSource` | `from_stream` / `from_bidi` (no feature gate) | yield-once, then `ConnectionClosed`; `open_bi` returns `StreamClosed` |

The `StreamBidiStreamSource` preserves the ADR-065 yield-once contract
exactly — `accept_bi` takes the stream from the `Mutex<Option<...>>` on
the first call, returns `ConnectionClosed` on subsequent calls. `open_bi`
returns `StreamError::StreamClosed`. `close` drops the stream (ignores
`code`/`reason` — see Part 2). `remote_addr` returns the stored
`SocketAddr`.

The `QuinnBidiStreamSource` / `IrohBidiStreamSource` impls delegate to
the underlying `quinn::Connection` / `iroh::endpoint::Connection` exactly
as the current `ConnectionKind::Quinn` / `Iroh` match arms do — same error
mapping (`map_quinn_connection_error` / `map_iroh_connection_error`), same
`SendStream`/`RecvStream` construction (`from_quinn` / `from_iroh`).

#### What does NOT change

- `ProtocolHandler` trait — unchanged.
- `HandlerRegistry` — unchanged.
- All handler code (`HttpAdapter`, `TtyAdapter`, `CallAdapter`) —
  unchanged. They receive a `Connection` and call `accept_bi` / `open_bi`
  on it. The dispatch through `Box<dyn BidiStreamSource>` is transparent.
- `SendStream` / `RecvStream` — unchanged (their own internal enum dispatch
  stays).
- `BiStream` trait — unchanged.
- The endpoint's accept loops — unchanged (they call `from_quinn` /
  `from_iroh`, which now internally wrap a `BidiStreamSource` impl).
- `Connection::remote_alpn` / `set_identity` / `identity` / `from_bidi` —
  unchanged.

### Part 2: `close()` params fix (REQ-CORE-02, folded into ADR-070)

The `close(&self, code: u32, reason: &str)` signature stays on the trait
(non-QUIC impls ignore the args). This resolves the ADR-065 leftover clippy
warning: under `--no-default-features`, the `Stream` backend's `close(code,
reason)` takes both args and uses neither, which clippy flags as two unused
variable warnings on `types.rs:500`.

Under the trait, the `StreamBidiStreamSource::close` impl prefixes the
args with `_code` / `_reason` and carries a doc comment stating they're
ignored because the drop is the close (ADR-065). The warning disappears;
the signature matches the public `Connection::close` API (no caller
breakage).

See ADR-070 §"REQ-CORE-02" for the full rationale of why the QUIC-shaped
signature stays on the trait rather than being split.

### Part 3: `AuthContext::anonymous` constructor (REQ-CORE-03)

Add a convenience constructor to `AuthContext` in
`crates/alknet-core/src/auth.rs`:

```rust
impl AuthContext {
    /// Construct an `AuthContext` with no identity, no fingerprint, and no
    /// remote address — only the ALPN is set. For POCs, tests, and handlers
    /// that don't require auth.
    pub fn anonymous(alpn: impl Into<Vec<u8>>) -> Self {
        Self {
            identity: None,
            alpn: alpn.into(),
            remote_addr: None,
            tls_client_fingerprint: None,
        }
    }
}
```

Not gated behind a `test-utils` feature — it's a plain `pub fn` useful for
any caller that constructs an `AuthContext` outside the endpoint's
resolution path. The name is honest about the semantics: no identity, no
fingerprint.

This removes the four-`None`-field literal that recurred in every handler
POC and test (`poc-summary.md` §"Issues Surfaced" #3).

### Commit structure

This is one task but may be two commits if the implementer prefers to
separate concerns:
1. `BidiStreamSource` trait + `Connection` refactor + `close()` fix
   (Parts 1+2, `types.rs`)
2. `AuthContext::anonymous` (Part 3, `auth.rs`)

Or one commit covering all three. Either is acceptable — the changes are
in different files but logically related (all from the same POC findings).

## Acceptance Criteria

### Part 1: BidiStreamSource

- [ ] `BidiStreamSource` trait defined with `accept_bi`, `open_bi`, `remote_addr`, `close` (all async or sync per ADR-070)
- [ ] `BidiStreamSource: Send + Sync + 'static` (object-safe)
- [ ] `Connection` struct holds `Box<dyn BidiStreamSource>` + `alpn: Vec<u8>` + `identity: OnceLock<Identity>`
- [ ] `QuinnBidiStreamSource` implements `BidiStreamSource` (feature-gated `quinn`)
- [ ] `IrohBidiStreamSource` implements `BidiStreamSource` (feature-gated `iroh`)
- [ ] `StreamBidiStreamSource` implements `BidiStreamSource` (no feature gate)
- [ ] `StreamBidiStreamSource::accept_bi` yields once, then `ConnectionClosed` (ADR-065 contract preserved)
- [ ] `StreamBidiStreamSource::open_bi` returns `StreamClosed`
- [ ] `Connection::from_quinn` / `from_quinn_with_alpn` / `from_iroh` / `from_stream` / `from_bidi` all preserved (same signatures, same behavior)
- [ ] `Connection::accept_bi` / `open_bi` / `remote_addr` / `close` delegate to `self.source`
- [ ] `Connection::remote_alpn` / `set_identity` / `identity` unchanged (read `self.alpn` / `self.identity`)
- [ ] `ConnectionKind` enum removed (replaced by the trait object)
- [ ] `map_quinn_connection_error` / `map_iroh_connection_error` preserved (used by the QUIC impls)
- [ ] Existing `types.rs` tests pass unchanged (the `test_connection()` helper, `set_identity` tests, `remote_alpn`/`remote_addr` tests, `StreamError` mapping tests)

### Part 2: close() fix

- [ ] `StreamBidiStreamSource::close` prefixes `code`/`reason` with `_` and documents why they're ignored
- [ ] `cargo clippy -p alknet-core --no-default-features` passes with **no warnings** (the ADR-065 leftover is fixed)
- [ ] `cargo clippy -p alknet-core` (default features) passes with no warnings
- [ ] `cargo clippy -p alknet-core --all-features` passes with no warnings (if applicable)

### Part 3: AuthContext::anonymous

- [ ] `AuthContext::anonymous(alpn: impl Into<Vec<u8>>)` constructor added to `auth.rs`
- [ ] Sets `identity: None`, `alpn: alpn.into()`, `remote_addr: None`, `tls_client_fingerprint: None`
- [ ] Unit test: `AuthContext::anonymous(b"alknet/test")` produces the expected fields
- [ ] Existing `auth.rs` tests pass unchanged

### All parts

- [ ] `cargo test -p alknet-core` succeeds (all feature combos)
- [ ] `cargo test -p alknet-core --no-default-features` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings
- [ ] `cargo clippy -p alknet-core --no-default-features` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-core` passes
- [ ] Downstream crates compile unchanged: `cargo check -p alknet-call` succeeds (no handler code change needed)

## References

- docs/architecture/decisions/070-bidistreamsource-trait.md — ADR-070 (the trait + close() rationale)
- docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md — ADR-065 (the yield-once contract the StreamBidiStreamSource preserves)
- docs/architecture/crates/core/core-types.md — Connection + BidiStreamSource spec (updated)
- docs/architecture/crates/core/auth.md — AuthContext::anonymous spec (updated)
- docs/research/alknet-channels/poc-summary.md — §"Issues Surfaced" #1-3 (the POC findings)
- docs/research/alknet-channels/phase-0-findings.md — §POC-Validated Requirements (REQ-CORE-01/02/03)

## Notes

> The `BidiStreamSource` refactor is the load-bearing change. It touches
> `Connection` — the struct every handler depends on — but the public API
> is preserved verbatim, so no handler code changes. The risk is in the
> internal dispatch: `Box<dyn BidiStreamSource>` adds one method-call
> indirection per `accept_bi`/`open_bi`/`close`/`remote_addr`, which is
> negligible next to the async I/O those operations perform. The
> `ConnectionKind` enum is removed entirely; the three variants become
> three trait impls. The clippy warning under `--no-default-features`
> (unused `code`/`reason` on the Stream backend's `close`) is fixed
> structurally by the trait — the `StreamBidiStreamSource::close` impl
> documents why the args are ignored, and the `_` prefix is intentional.
> Verify all four feature combinations pass clippy (default, quinn-only,
> iroh-only, no-default-features) — the warning only surfaces under
> `--no-default-features` today.
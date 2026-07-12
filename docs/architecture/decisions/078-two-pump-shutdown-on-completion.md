# ADR-078: Two-Pump Shutdown-on-Completion Pattern

## Status

Accepted

## Context

The de-risk POC (`docs/research/alknet-channels/poc-summary.md` §Issues
Surfaced #7) surfaced a deadlock in the tunnel handler's two-pump shape.
The naive `tokio::try_join!(c2t, t2c)` deadlocks: each pump waits for the
other's EOF, which only comes once the *opposite* pump completes and shuts
down its sink. The TTY adapter avoids this because its three pumps
coordinate via the `exit_code` future — a third signal. A two-pump handler
(tunnel, SSH `direct-tcpip`) has no such third signal.

The fix the POC found: shut down the peer's sink when one pump completes.
`c2t` (client→target) shuts down `tcp_write` on EOF; `t2c` (target→client)
shuts down `send` on EOF. This is the `pump_session` shape with an explicit
shutdown-on-completion step that the TTY adapter doesn't need (because
TTY's three pumps coordinate via the exit future).

This pattern will recur — any handler with a pump-driven two-direction
shape (tunnel, SSH `direct-tcpip`, future port-forward) needs it. Getting it
wrong hangs channels silently. The POC hung; the spec must pin the pattern.

## Decision

### The two-pump pattern is a documented contract

A two-pump handler (two `tokio::io::copy` pumps, one per direction) MUST
shut down the opposite sink when one pump completes. `tokio::try_join!`
alone deadlocks because each pump waits for the other's EOF, which only
comes after the opposite pump shuts down its sink.

```rust
// Correct two-pump shape:
let (mut send, mut recv) = connection.accept_bi().await?;
let mut tcp = TcpStream::connect(target).await?;
let (mut tcp_read, mut tcp_write) = tcp.into_split();

let c2t = async {
    tokio::io::copy(&mut recv, &mut tcp_write).await?;
    tcp_write.shutdown().await.ok();  // shut down the peer's sink
    Result::<_, std::io::Error>::Ok(())
};
let t2c = async {
    tokio::io::copy(&mut tcp_read, &mut send).await?;
    send.shutdown().await.ok();  // shut down the peer's sink
    Result::<_, std::io::Error>::Ok(())
};
tokio::try_join!(c2t, t2c)?;
```

When `c2t` completes (recv EOF), it shuts down `tcp_write`, which causes
`t2c`'s `tcp_read` to eventually EOF, completing `t2c`. When `t2c`
completes (tcp_read EOF), it shuts down `send`, which causes `c2t`'s `recv`
to eventually EOF. Either pump completing unblocks the other.

### Where the pattern lives

The pattern is a **handler-level contract**, not a channels-layer concern.
The channels layer routes chunks; the handler owns its pump logic. This ADR
documents the pattern so handlers don't reimplement it incorrectly.

The channels spec (`channels-adapter.md`) documents the pattern in the
handler-integration section. The tunnel handler (the first two-pump
consumer) implements it. Future two-pump handlers (SSH `direct-tcpip`)
follow the same shape.

### Consideration: a helper in alknet-core

The POC summary suggested "a helper in `alknet-core` that encapsulates the
'two-pump with shutdown-on-completion' shape so handlers don't reimplement
it." This is an implementation convenience, not an architecture decision.
The contract is the shutdown-on-completion pattern; whether it's a helper
function or inline code in each handler is a two-way-door implementation
detail.

**Decision: do not add a core helper yet.** The pattern is ~10 lines of
inline code. A helper would be called from handler crates (`alknet-tty`,
the future tunnel crate, the future SSH crate), which means the helper's
signature (`fn pump_bidi<R, W>(recv: R, send: W, ...) -> impl Future`) is a
cross-crate API surface. Extracting it prematurely (with one consumer — the
POC's tunnel) would bake in a shape that the second consumer (SSH
`direct-tcpip`) might not fit. The pattern is documented; the helper is
extracted when two real consumers exist and their shapes converge. This is
a genuine deferral (blocked on: a second two-pump handler existing), not a
hedge — the contract is decided (shutdown-on-completion), only the
extraction is deferred.

### The three-pump pattern (TTY) is unaffected

The TTY adapter's `pump_session` (three pumps: stdout, stderr,
client→backend, coordinating via the `exit_code` future) does not have this
deadlock because the `exit_code` future is the third signal that unblocks
the pumps. This ADR applies only to two-pump handlers. The TTY adapter is
unchanged.

## Consequences

**Positive:**
- The two-pump deadlock is documented as a contract, not left as a POC
  finding. Handlers that follow the pattern don't hang.
- The pattern is handler-level — the channels layer stays a re-framing
  proxy, not a pump-logic owner.
- The three-pump pattern (TTY) is unaffected — the ADR scopes itself to
  two-pump handlers.

**Negative:**
- Each two-pump handler implements the shutdown-on-completion inline (~10
  lines). Until a core helper is extracted (deferred, blocked on a second
  consumer), the pattern is copy-paste with documentation. This is the
  correct trade-off: the contract is decided, the extraction is deferred on
  a real blocker (shape convergence across consumers), not hedged.

## Door type

**One-way.** The shutdown-on-completion contract is a correctness invariant
— two-pump handlers MUST shut down the opposite sink on pump completion, or
they deadlock. This is not a preference; it is a correctness requirement.

The core helper extraction is a **deferred decision** (OQ-57,
deferred(scope)), not a door-type attribute. Its door type is two-way (a
helper function is additive), but the extraction is not decided in this
ADR — see OQ-57 for the blocking condition (a second two-pump handler
existing, so shape convergence is observable).

## References

- ADR-074: ChannelBidiStreamSource (the `accept_bi` that yields the stream
  pair the pumps operate on)
- ADR-055: exit-chunk-is-last (the three-pump TTY invariant — the pattern
  this ADR does NOT touch)
- `docs/research/alknet-channels/poc-summary.md` §Issues Surfaced #7 (the
  deadlock the POC found and fixed)
- `crates/alknet-tty/src/adapter.rs` — `pump_session` (the three-pump
  reference shape)
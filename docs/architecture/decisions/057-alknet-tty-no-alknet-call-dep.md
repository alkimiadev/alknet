# ADR-057: alknet-tty Does Not Depend on alknet-call (Self-Contained Negotiation Framing)

## Status

Accepted

## Context

The alknet-tty specs (ADR-052, ADR-053, ADR-054) and ADR-003 Amendment 1
previously stated that alknet-tty depends on alknet-call for the
`FrameFramedReader`/`FrameFramedWriter` "framing utility" — the 4-byte
big-endian length prefix + UTF-8 JSON body framing the negotiation frame
uses. The claim was "reuse the framing utility, not the `EventEnvelope`
type" (ADR-052 §6).

A pre-implementation sanity check surfaced that **the framing utility is
not actually reusable as the spec described.** `FrameFramedReader` (in
`crates/alknet-call/src/protocol/wire.rs`) is hardcoded to deserialize
`EventEnvelope`:

```rust
pub async fn read_frame(&mut self) -> Result<EventEnvelope, FrameError> {
    // ... read 4-byte length prefix, read body ...
    let envelope: EventEnvelope = serde_json::from_slice(&body)?;
    Ok(envelope)
}
```

The framing logic (read 4 bytes → length, read N bytes) and the
`EventEnvelope` deserialization are entangled in the same method. The
"framing utility" the spec claimed to reuse does not exist as a
separable thing — the length-prefix read and the type-specific
deserialize are one call. alknet-tty's negotiation frame is a
`NegotiateRequest`, not an `EventEnvelope`, so `read_frame()` cannot
return what alknet-tty needs.

This left three options (see ADR-003 Amendment 2 for the full
comparison):

1. **Duplicate the ~30 lines of framing logic in alknet-tty.** The
   framing is a trivial length-prefix idiom (4-byte BE length + body);
   the two copies would share an idiom, not a domain abstraction.
2. **Promote a generic length-prefixed framing utility to alknet-core.**
   Makes the spec's claim true (a reusable utility exists), but accretes
   a framing module to the foundation crate for the sake of two
   consumers — a shared utility pays for itself at ≥2 consumers, but
   the framing is trivial enough that the cost of the shared abstraction
   (a new module, a new type, a refactor of alknet-call) exceeds the
   cost of the duplication.
3. **Actually use alknet-call** (e.g., model the tty control channel as
   call-protocol operations). A different architecture, not a
   dependency-edge fix — the current `ControlMessage` tagged enum
   (ADR-052) is the two-way-door seam; replacing it with the call
   protocol is a v2 ALPN decision, not a v1 dependency choice.

## Decision

### 1. alknet-tty does not depend on alknet-call

alknet-tty implements its own length-prefixed framing for the
negotiation frame directly on tokio's `AsyncRead`/`AsyncWrite`. The
crate's dependency edge is:

```
alknet-tty
└── alknet-core   (ProtocolHandler, Connection, AuthContext, Identity,
                   AccessControl, OwnershipProvider — ADR-050)
```

No `alknet-call` dependency. The crate depends on alknet-core (which
every handler crate depends on anyway for the `ProtocolHandler` trait)
and nothing else for the protocol surface. `portable_pty`, `bollard`,
`russh` remain in the backend crates (ADR-054).

### 2. The framing format coincides with alknet-call's by convention, not by code reuse

Both alknet-tty's negotiation frame and alknet-call's `EventEnvelope`
frame use a 4-byte big-endian length prefix + UTF-8 JSON body. This is
a shared *format convention* (length-prefixed JSON is a standard
framing pattern), not a code dependency. The two implementations are
independent: alknet-tty's reader deserializes `NegotiateRequest`;
alknet-call's `FrameFramedReader` deserializes `EventEnvelope`. They
share an idiom (length-prefix framing), not a module.

### 3. The framing logic lives in alknet-tty as a small, self-contained module

alknet-tty implements the negotiation framing as a small module
(~30 lines: read 4-byte BE length, bounds-check, read N bytes; write the
inverse). The module's types (`NegotiateFrameReader`/`NegotiateFrameWriter`
or similar) are private to the crate — they are not a reusable utility
for other crates. If a future crate wants length-prefixed JSON framing,
it implements its own (the idiom is trivial) or a future ADR promotes a
generic utility to alknet-core at that point (deferred — not needed for
the current scope; two consumers is the threshold but the second
consumer does not yet exist).

## Consequences

**Positive:**

- alknet-tty's dependency surface is minimal and correct: alknet-core
  only. No dependency on alknet-call for a "framing utility" that wasn't
  reusable as specced. The "weird" dependency edge (a handler crate
  depending on another handler crate for 30 lines of glue) is gone.
- The spec is honest: it describes what the code does (a self-contained
  framing module) rather than what a previous draft hoped for (a
  reusable utility in alknet-call that doesn't exist in a separable
  form).
- The framing logic is trivial and self-contained; bugs in it are
  local to alknet-tty (no cross-crate coordination if alknet-call's
  framing changes for call-protocol reasons).
- ADR-003's "no handler crate depends on another handler crate" rule is
  preserved without the Amendment 1 exception for alknet-tty. The
  exception remains for alknet-http/agent/napi (which use alknet-call's
  `OperationSpec`/`Handler`/`OperationAdapter` types — actual type
  reuse, not framing glue).

**Negative:**

- ~30 lines of framing logic are duplicated between alknet-tty and
  alknet-call. The duplication is an idiom (length-prefix framing), not
  a domain abstraction; the cost of the shared abstraction (a new
  module in alknet-core + a refactor of alknet-call) exceeds the cost
  of the duplication for two consumers. If a third consumer appears,
  this trade-off should be revisited (promote to alknet-core).
- A bug found in the length-prefix framing edge cases (e.g., a
  partial-read handling bug) would need fixing in two places. The
  framing is mature (a standard `read_exact`-based pattern); the edge
  cases are known and tested in both crates independently.

## Door type

**One-way.** alknet-tty not depending on alknet-call is a dependency-edge
commitment. Adding the dependency back later (if, e.g., the ctty idea
of ADR-003 Amendment 2 is pursued) is a new ADR. The framing logic being
self-contained in alknet-tty is two-way (it could be refactored to a
shared utility in alknet-core later without a wire-format change), but
the dependency edge is one-way.

## Assumptions

1. **The framing logic is trivial enough that duplication is cheaper
   than a shared abstraction.** Length-prefixed framing (4-byte BE
   length + body) is a ~30-line idiom. The cost of a shared utility in
   alknet-core (a new module, a new type, a refactor of alknet-call's
   `wire.rs` to extract the generic layer) is higher than the cost of
   two independent implementations for two consumers. If a third
   consumer appears, revisit (promote to alknet-core).

2. **The format coincidence (both use 4-byte BE length + JSON body) is
   stable.** Both crates use the same length-prefix convention. If
   alknet-call's framing changes (e.g., a different max-frame-size, a
   different prefix width), alknet-tty's is unaffected — they are
   independent implementations that happen to share a format today.

## References

- [ADR-003](003-crate-decomposition.md) Amendment 1 (protocol-foundation
  exception for alknet-http/agent/napi) and Amendment 2 (this ADR's
  effect on the Amendment 1 framing-reuse claim for alknet-tty)
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) §2
  (negotiation frame format), §6 (revised: format coincides by
  convention, not by code reuse)
- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — the crate
  decomposition this ADR's dependency edge affects
- `crates/alknet-call/src/protocol/wire.rs` — the `FrameFramedReader`/
  `FrameFramedWriter` that are NOT reused (the `EventEnvelope`-bound
  methods that motivated this ADR)
- Spec: [crates/tty/overview.md](../crates/tty/overview.md) (dependency
  edge), [crates/tty/tty-wire.md](../crates/tty/tty-wire.md) (negotiation
  framing)
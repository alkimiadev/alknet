# ADR-064: irpc Was Never Integrated — Hand-Rolled EventEnvelope Framing

## Status

Accepted

## Context

ADR-005 accepted "irpc as the call protocol foundation" based on the
previous architecture's use of irpc. When the call protocol was implemented,
it turned out that **no `.rs` file in the workspace ever imported irpc**.
The workspace `Cargo.toml` declared `irpc = "0.16"` / `irpc-derive = "0.16"`
as workspace dependencies, and `crates/alknet-call/Cargo.toml` declared
`irpc = { workspace = true }`, but the import was never written. The wire
protocol (`crates/alknet-call/src/protocol/wire.rs`) is hand-rolled
length-prefixed JSON — 4-byte big-endian length prefix + UTF-8 JSON body —
not an irpc service.

This was discovered when building an external app against the crates: the
`irpc 0.16` workspace dep was a version-gap blocker for `alknet-blobs` (which
pulls `irpc 0.17` transitively via `iroh-blobs 0.103`). A grep for any `irpc`
import in the workspace found zero hits — the dep was dead weight carried
over from the previous architecture without verification.

The framing, operation registry, dispatch, and subscription patterns that
ADR-005 attributed to irpc are all hand-rolled in alknet-call:

- **Framing**: `FrameFramedReader` / `FrameFramedWriter` in `wire.rs` —
  length-prefixed JSON, hand-written against `tokio::io::AsyncRead`/
  `AsyncWrite`. Not an irpc service.
- **Operation registry**: `OperationSpec`, `Handler`, `OperationRegistry`,
  `AccessControl` — hand-rolled in alknet-call, not irpc's `Service` trait.
- **Event types**: `call.requested`, `call.responded`, `call.completed`,
  `call.aborted`, `call.error` — the alknet call protocol's own event
  vocabulary, not irpc's.
- **Subscription/streaming**: `StreamingHandler` / `invoke_streaming()`
  (ADR-049) — hand-rolled, not irpc's streaming patterns.

The `EventEnvelope { type, id, payload }` shape was derived from the
`@alkdev/pubsub` TypeScript `EventEnvelope` (`/workspace/@alkdev/pubsub/src/
types.ts`), not from irpc. ADR-005's claim that "the wire format is irpc's
EventEnvelope framing" was wrong — irpc was never imported, and the envelope
shape has a different origin (the pubsub prior art, ADR-013). The framing
coincidentally resembles irpc's (both are length-prefixed JSON), which is
how the misattribution went unnoticed.

### What ADR-005 got right

Despite the irpc misattribution, ADR-005's *architectural* decisions are
correct and stand unchanged:

- The call protocol uses length-prefixed JSON `EventEnvelope` framing
  (hand-rolled, not irpc-supplied).
- The wire format is cross-language and consumable from TypeScript, Python,
  any language (JSON is inherently cross-language — ADR-005's "mitigated:
  it's length-prefixed JSON" note was the load-bearing point, not the irpc
  attribution).
- Operations use JSON Schema discovery. The `OperationSpec` shape is
  hand-rolled, JSON-Schema-compatible — the same property ADR-005 attributed
  to irpc, achieved without irpc.

### Why a new ADR rather than an amendment

ADR-005's Decision and Consequences are built on the premise "alknet-call
uses irpc as its foundation — irpc IS the call protocol's core." That
premise is false. Amending ADR-005 to say "actually it's hand-rolled" would
leave an ADR whose Context, Decision, and Consequences sections all argue
for a choice that was never made. The correct record is: ADR-005 is
superseded; the call protocol uses hand-rolled framing (this ADR-064); the
architectural properties ADR-005 sought (proven framing, cross-language
JSON, streaming) are preserved, but the mechanism is hand-rolled, not
irpc-sourced.

### The dead dep removal

The `irpc` / `irpc-derive` workspace deps and the `alknet-call` consumer dep
were removed in commit `668d777` (2026-07-09). `irpc` may be re-added as
`0.17` when `alknet-blobs` lands (it pulls `irpc 0.17` transitively via
`iroh-blobs 0.103`), but that would be a *transitive* dependency of
`alknet-blobs`, not a direct dependency of `alknet-call` — alknet-call does
not import irpc and has no plans to. See
[`docs/research/transport-generalization/findings.md`](../../research/transport-generalization/findings.md)
§3.1 for the removal trace.

## Decision

1. **ADR-005 is superseded.** The call protocol does not use irpc. irpc was
   never imported by any `.rs` file in the workspace. The dead `irpc` /
   `irpc-derive` workspace and crate deps are removed.

2. **The call protocol uses hand-rolled `EventEnvelope` framing.** The wire
   format is length-prefixed JSON (4-byte big-endian length + UTF-8 JSON
   body), implemented in `crates/alknet-call/src/protocol/wire.rs`. The
   `EventEnvelope { type, id, payload }` shape was derived from the
   `@alkdev/pubsub` TypeScript prior art (ADR-013), not from irpc. The
   framing, operation registry, dispatch, and streaming patterns are all
   hand-rolled in alknet-call.

3. **The architectural properties ADR-005 sought are preserved by the
   hand-rolled implementation:**
   - Proven framing — length-prefixed JSON is a well-understood,
     battle-tested pattern; the hand-rolled implementation is tested (207
     lib + 2 integration tests passing).
   - Cross-language — JSON is inherently consumable from any language;
     NAPI, WASM, and browser clients speak the same wire format.
   - Streaming — `StreamingHandler` / `invoke_streaming()` (ADR-049) provide
     the subscription/streaming patterns ADR-005 attributed to irpc,
     hand-rolled.

4. **irpc is not a planned dependency for alknet-call.** If `alknet-blobs`
   pulls irpc transitively, it will be a transitive dependency of that
   crate, not a direct dependency of alknet-call. alknet-call's framing,
   registry, and dispatch are hand-rolled and will remain so. The "mitigated:
   irpc is lightweight and we can fork if needed" caveat in ADR-005 is moot
   — there is nothing to fork because nothing was integrated.

5. **The vault's irpc drop (ADR-025) stands.** ADR-025 dropped irpc from
   alknet-vault. With this ADR, irpc is also confirmed absent from
   alknet-call. The vault and call decisions are now consistent: neither
   crate uses irpc. The only difference is that ADR-025 *removed* a real
   (but unused-for-its-primary-path) irpc dependency from the vault, while
   this ADR records that alknet-call's irpc dependency was never integrated
   at all — it was a Cargo.toml entry with no corresponding import.

## Consequences

**Positive:**
- The spec matches the code. ADR-005's irpc claims were a spec/code
  divergence that surfaced only when the `irpc 0.16` version gap blocked
  `alknet-blobs`. This ADR closes the divergence.
- `alknet-blobs` is unblocked — the dead `irpc 0.16` workspace dep is
  gone; `iroh-blobs 0.103` (which pulls `irpc 0.17` transitively) no longer
  conflicts with a workspace-pinned older irpc.
- The call protocol's framing, registry, and dispatch are documented
  accurately as hand-rolled — readers of the spec aren't sent looking for
  an irpc integration that doesn't exist.
- The cross-language story is unchanged: JSON wire format, JSON Schema
  discovery. The mechanism changed (hand-rolled vs irpc), but the property
  ADR-005 sought is preserved.

**Negative:**
- The call protocol does not inherit irpc's testing or production pedigree
  for its framing. Mitigation: length-prefixed JSON is a trivial,
  well-understood pattern; the hand-rolled implementation is tested; and
  the framing is small enough to audit completely (~30 lines in `wire.rs`).
- ADR-005's claim that "the call protocol inherits irpc's streaming and
  subscription patterns" was wrong — those patterns are hand-rolled
  (ADR-049). The streaming implementation is younger and less battle-tested
  than irpc's would have been, but it is also simpler and fully owned.

## References

- ADR-005: irpc as call protocol foundation (superseded by this ADR)
- ADR-025: Vault local-only dispatch (dropped irpc from the vault; this ADR
  records irpc was never integrated into alknet-call either)
- ADR-013: Rust as canonical implementation (the `@alkdev/pubsub` prior art
  that the `EventEnvelope` shape was actually derived from)
- ADR-049: Streaming handler for subscriptions (the hand-rolled streaming
  dispatch path)
- Call protocol wire format: `crates/alknet-call/src/protocol/wire.rs`
- Transport generalization findings:
  [`docs/research/transport-generalization/findings.md`](../../research/transport-generalization/findings.md)
  §3.1 (dead `irpc` dep removal)
- Removal commit: `668d777` (2026-07-09)
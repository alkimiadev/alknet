# ADR-094: Per-Identity Channel Cap as DoS Defense

## Status

Accepted (amends ADR-076's DoS-defense framing — the per-connection
`max_channels = 256` is reframed as a per-connection memory bound, not a
DoS defense)

## Context

ADR-076 set the channels-layer channel limit at 256 **per connection**
and framed that cap as the DoS defense against an authenticated peer
opening many channels and never reading from them ("DoS defense
summary" table, "Per-connection channel count cap → 256 channels"). On
review, the per-connection cap is not a DoS defense at all. A single
peer can open an unbounded number of transport connections, and across
those connections, across substrates, the peer gets 256 × N × (substrate
multiplier) channels:

| Substrate a peer can use | Channels per connection |
|--------------------------|--------------------------|
| In-line (TCP+TLS, WebTransport, SSH `direct-tcpip`) | 256 (one stream, header-demuxed) |
| Native (QUIC substreams) | 256 (per-connection demux; each substream carries one channel) |
| Multi-connection (N transport connections) | 256 × N |

A peer that opens 10 transport connections to the same accepting peer
gets 2,560 channels. A peer that opens 100 gets 25,600. There is no
bound on the number of transport connections a peer can open. The
"per-connection, not per-peer — a peer can open more channels on a
second connection" line in ADR-076 was, in retrospect, the channels
layer confessing a hole and hoping the layer above it would fill it.
That is not a DoS defense; it is a per-connection memory bound
(reassembly-buffer cost per connection) labeled as a DoS defense.

The only coherent unit for a channel DoS defense is the **identity**.
The peer, not the connection, is what an authenticated-DoS defense
must bound. This is the same primitive as any other resource ACL:
`OwnershipProvider` (ADR-050) checks "does identity X own resource
Y?"; the channel cap checks "has identity X exceeded their channel
quota?" Same shape, different resource.

### Why the channels layer cannot hold the cap

`ChannelManager` (ADR-075) is auth-blind by design: "No auth state.
Auth lives in the `OperationContext` that the call protocol passes to
`channel/open`." That decision is load-bearing — it is what makes the
channels layer WASM-compatible, transport-agnostic, and ALPN-blind
(ADR-075, ADR-093). Putting per-identity tracking in the channels
layer would reverse ADR-075.

So the per-identity cap lives **one layer up**, in `channels-call`,
where the identity is already on `OperationContext` (the same place
`AccessControl::check` runs). The `channel/open` and `channel/close`
handlers (ADR-073) are in `channels-call` already; they gain a policy
consultation. The channels layer (`channels-core`) is unchanged —
still auth-blind, still WASM-clean.

### This is not a hub-specific concern

The cap is a **channels-accepting-peer concern**. A worker accepting a
direct channels connection from a peer needs the cap just as much as a
hub does. The call protocol does not need a hub to enforce "does this
peer have access to this resource?" (ADR-073: `AccessControl::check` on
`channel/open`), and neither should channels. Framing the cap as
hub-specific would be the "assembly layer" hedging pattern — putting
the hard question off on a fictional "later" that, when it arrives,
turns out to be exactly the same problem. The cap is a peer concern;
the hub is one peer that happens to aggregate others.

The cap is also **symmetric**, like the call protocol. Peer A accepts
a channels connection from Peer B; A enforces its cap on B's channels;
B enforces its cap on A's channels. Both sides have the cap, both
sides check it, same as `AccessControl::check` on any operation.

## Decision

### 1. A `ChannelLifecyclePolicy` trait in `channels-call`

```rust
/// Per-identity channel lifecycle policy. Consulted by the
/// `channel/open` handler (after `AccessControl::check`, before
/// allocation) and the `channel/close` handler (after deallocation).
/// Both handlers have the identity via `OperationContext`.
///
/// A channel slot is a resource; the cap is a quota check on that
/// resource — parallel to `OwnershipProvider::owns` (ADR-050) for
/// spawned resources. Same primitive, different resource.
pub trait ChannelLifecyclePolicy: Send + Sync + 'static {
    /// Before channel allocation. Deny with `channel:too_many_channels`
    /// (ADR-073) when the identity is over its cap. The identity is
    /// the direct caller (the peer that opened this channels
    /// connection); `forwarded_for` is metadata and is NOT consulted
    /// (ADR-032).
    fn check_open(&self, identity: &Identity) -> Result<(), ChannelError>;

    /// After channel deallocation. Decrement the per-identity count.
    /// Called by the `channel/close` handler after the drain completes
    /// (ADR-076 §channel-id-reuse).
    fn on_close(&self, identity: &Identity);
}
```

### 2. Default: `PerIdentityChannelPolicy::new(256)`

The default constructor enforces 256 per identity out of the box — no
hedging, no "NoOp default + wire it in the assembly layer." A channels
accepting peer that constructs `ChannelOperations::new(manager)` with
no policy argument gets `PerIdentityChannelPolicy::new(256)`. The
default is secure; opt-outs are explicit:

- `PerIdentityChannelPolicy::new(cap)` — shared per-identity state
  (`HashMap<PeerId, usize>` + cap), constructed **once per accepting
  peer** and shared (via `Arc`) across every channels connection that
  peer accepts. For a hub, that's one `Arc<PerIdentityChannelPolicy>`
  on the `Hub`, shared across all worker and browser legs. For a
  worker accepting direct channels, that's one `Arc` on the worker's
  own state, shared across whatever connections it accepts. For tests
  and POCs, the default constructor.
- `PerIdentityChannelPolicy::with_per_identity_caps(mapping)` — a
  per-peer-role variant: `HashMap<PeerId, usize>` overrides the
  default cap for specific peers. Used by a spoke that serves a
  high-fan-out hub (the hub peer's cap is set higher than a worker
  peer's cap — see "Relay consequence" below).
- `NoCap` — no cap (for tests, POCs, and trusted single-peer
  deployments). Explicit opt-out, not the default.

The policy is constructed once and passed to `ChannelOperations` at
registration time:

```rust
let policy = Arc::new(PerIdentityChannelPolicy::new(256));
let channel_ops = ChannelOperations::new(manager, policy);
channel_ops.register_on(&mut call_registry)?;
```

The same `Arc<PerIdentityChannelPolicy>` is shared across every
channels connection that peer accepts — that is what makes the cap
per-identity, not per-connection.

### 3. Enforcement point: between `AccessControl::check` and allocation

The `channel/open` handler (ADR-073) gains the policy check after
ACL and before `next_id.fetch_add`:

1. ACL is already checked by `OperationRegistry::invoke` (the existing
   `AccessControl::check` path — unchanged).
2. **NEW:** `policy.check_open(&op_ctx.identity)?` — deny with
   `channel:too_many_channels` if over cap.
3. Allocate the `channel_id` via `next_id.fetch_add(1, Relaxed)` (DP-1:
   server-assigned — unchanged).
4. Construct the `ChannelBidiStreamSource`, spawn the handler, record
   the `ChannelState` (unchanged).
5. Return the `channel_id`.

The `channel/close` handler (ADR-073) gains the decrement after the
drain completes (the same point ADR-076 marks the `channel_id` as
eligible for reuse):

1. Drain the reassembly buffer for `channel_id` (existing — ADR-076
   §channel-id-reuse).
2. **NEW:** `policy.on_close(&op_ctx.identity)` — decrement the
   per-identity count.
3. Return `{ "closed": true }` (unchanged).

### 4. `ChannelManager.max_channels = 256` stays as a per-connection memory bound

The per-connection cap (ADR-076) stays, but is reframed. It is no
longer the DoS defense — it is a per-connection **memory bound** that
limits one connection's reassembly-buffer cost regardless of policy.
It composes with the per-identity cap but is not the security
boundary. It still returns `channel:too_many_channels` when hit; the
per-identity policy returns the same error when the per-identity cap
is hit. An over-cap peer sees the same error either way; which cap
fired first is an implementation detail.

Keeping the per-connection bound as a backstop covers deployments that
use `NoCap` (tests, trusted single-peer) and bounds the damage if a
custom policy is buggy. Removing it would leave the channels layer
unbounded in the no-policy case. The cost of keeping it is zero (the
cap is already implemented in the POC); the cost of removing it is a
real hole in the `NoCap` path.

### 5. Relay consequence: the spoke caps the hub, not the browser

When the hub relays a browser's channel to a spoke (ADR-079), the
spoke sees the hub as the direct caller. `forwarded_for` carries the
browser's identity as metadata (ADR-032 — `forwarded_for` is not
authority; `AccessControl::check` never reads it). The channel cap
follows the same shape: the spoke's `ChannelLifecyclePolicy` is
consulted with the **hub's** identity, not the browser's. The spoke
asks "does the hub have access to open another channel?" and the
hub's quota on the spoke reflects the aggregate of all relayed
channels. The hub's per-browser caps are the hub's own concern
(enforced on the browser leg by the hub's own policy), not the
spoke's.

This is correct and consistent — the spoke authorizes the hub for
container access the same way it authorizes any peer, and the hub's
browser-relay ACL is the hub's own layer. The channel cap follows the
same pattern as any other resource ACL.

**Deployment consequence:** a spoke that serves a hub relaying for
many browsers must set the hub peer's cap higher than a worker peer's
cap, or the spoke denies legitimate relayed channels when the hub's
aggregate count exceeds a worker-sized cap. This is a per-peer-role
policy, set by the spoke via `with_per_identity_caps`. The
architecture provides the mechanism (`PerIdentityChannelPolicy::with_per_identity_caps`);
the deployment sets the numbers. This is not a flaw — it is the same
shape as any per-peer ACL (a spoke may authorize one peer for 1000
containers and another for 10; the channel cap is the same kind of
per-peer policy).

### 6. Recursive channels do not bypass the cap

A recursive `alknet/channels`-inside-`alknet/channels` channel runs a
new `ChannelsAdapter` with a new `ChannelManager`. If the same
`ChannelLifecyclePolicy` is wired into the inner `ChannelOperations`,
the inner channels are counted against the same identity. If a
different policy is wired, the inner channels are counted against
that policy's identity (which may be a different identity, if the
inner channels connection is authenticated separately). Either way,
the cap applies; recursion is not a bypass. The 13-byte-per-chunk
overhead of recursion is the documented cost (ADR-093); the cap
behavior is unchanged. Recursive channels are an edge case for edge
cases and not specced further.

## Consequences

**Positive:**
- A real per-identity DoS defense. A peer with N transport connections
  to the same accepting peer is bounded by 256 (or the configured
  per-identity cap), not 256 × N × (substrate multiplier). The cap
  composes correctly across substrates because the unit is the
  identity, not the connection.
- The cap is symmetric, like the call protocol. Both sides of a
  channels connection enforce their cap; the cap is a peer concern,
  not a hub-specific concern.
- The cap lives in `channels-call`, where the identity is already on
  `OperationContext`. The channels layer (`channels-core`) is
  unchanged — still auth-blind, still WASM-clean, still
  transport-agnostic. ADR-075's auth-blindness is preserved.
- The default is secure. `PerIdentityChannelPolicy::new(256)` is the
  out-of-the-box behavior; opt-outs (`NoCap`) are explicit. A
  deployment that forgets to wire a policy still gets a per-identity
  cap.
- The cap is the same primitive as any other resource ACL
  (`OwnershipProvider` for spawned resources, `AccessControl::check`
  for operations). The mental model is uniform: a channel slot is a
  resource, the cap is a quota check on that resource.

**Negative:**
- One new trait (`ChannelLifecyclePolicy`) and one new constructor
  argument on `ChannelOperations`. The `channel/open` and
  `channel/close` handlers gain a policy call. Small implementation
  cost; the policy is a single trait method per direction.
- Per-identity state is shared across connections
  (`HashMap<PeerId, usize>` on the policy, guarded by a `Mutex`). The
  state is touched on `channel/open` and `channel/close` only — not
  on every chunk. The contention is per-identity, not per-chunk;
  acceptable for the intended use cases.
- A spoke serving a high-fan-out hub must set the hub peer's cap
  higher than the default, or legitimate relayed channels are denied.
  This is a deployment-time policy decision, surfaced explicitly by
  `with_per_identity_caps`. Not a flaw; the same shape as any
  per-peer ACL.
- The cap is per direct-caller identity (ADR-032), not per
  `forwarded_for` originator. A hub relaying for 100 browsers
  consumes one channel slot per relayed channel against the hub's
  quota on the spoke, not 100 slots against 100 browser quotas. A
  spoke that wants per-browser capping would need to read
  `forwarded_for` for authority, which ADR-032 explicitly forbids.
  This is the correct trade-off: capping against `forwarded_for`
  would reverse ADR-032's "forwarded_for is metadata, not authority"
  and is a much bigger change. The hub enforces per-browser caps on
  the browser leg; the spoke enforces per-hub caps on the spoke leg.

## Door type

**One-way.** The `ChannelLifecyclePolicy` trait surface
(`check_open(&Identity) -> Result<(), ChannelError>` and
`on_close(&Identity)`) is a one-way-door API commitment — the
`channels-call` `channel/open` and `channel/close` handlers depend on
it, and consumers (`Hub`, worker crates) construct implementations.
Removing the trait or changing the signatures after deployments exist
is a breaking change.

The **default cap value (256)** is a two-way-door implementation
detail within the one-way trait surface — changing the default is
additive (a new constructor or a default-override), not a wire-format
change.

The **reframing of ADR-076's per-connection cap** (from DoS defense
to memory bound) is two-way — it's a documentation change, not a
behavior change. The per-connection cap still exists, still returns
`channel:too_many_channels`, and still bounds one connection's
reassembly-buffer cost.

## References

- **ADR-076**: Backpressure, Channel Limits, and ID Reuse (amended by
  this ADR — the per-connection `max_channels = 256` is reframed as a
  per-connection memory bound, not a DoS defense; the "DoS defense
  summary" table is removed; the "per-connection, not per-peer" line
  is corrected)
- **ADR-075**: ChannelsAdapter and ChannelManager (the auth-blindness
  this ADR preserves — the cap lives in `channels-call`, not
  `channels-core`)
- **ADR-073**: Channel Lifecycle Operations (the `channel/open` and
  `channel/close` handlers that gain the policy check; the
  `channel:too_many_channels` error code)
- **ADR-093**: channels Pure Channel Multiplexing (the umbrella
  decision; the channels layer has no `stream_type` concept, and no
  identity concept either — both are above it)
- **ADR-032**: Forwarded-For Identity (Metadata, Not Authority) (why
  the spoke caps the hub, not the browser — `forwarded_for` is
  metadata; the direct caller's identity is the authority for the cap
  just as it is for `AccessControl::check`)
- **ADR-079**: Hub Relay — Translate, Not Transparently Forward (the
  relay path where the spoke sees the hub as the direct caller)
- **ADR-050**: Dynamic Resource Ownership for Runtime-Spawned
  Resources (the parallel — a channel slot is a resource, the cap is
  a quota check, same primitive as `OwnershipProvider::owns`)
- **ADR-030**: PeerEntry and Identity.id Decoupling (`PeerId` =
  `Identity.id` — the stable key the per-identity cap counts against)
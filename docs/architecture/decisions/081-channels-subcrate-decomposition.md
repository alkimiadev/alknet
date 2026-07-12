# ADR-081: alknet-channels Sub-Crate Decomposition

## Status

Accepted

## Context

The initial channels spec (ADR-071-080) framed `alknet-channels` as a
single crate depending on both `alknet-core` and `alknet-call`. The
dependency on `alknet-call` arises because channel lifecycle operations
(`channel/open`, `channel/close`, `channel/control`,
`channel/resources/subscribe`) register on the call protocol's
`OperationRegistry`, and channel 0 is pre-negotiated as `alknet/call`
(ADR-072).

This creates two issues:

1. **The dependency graph conflates the pure multiplexer with the call-
   protocol coupling.** The wire format, demux/mux, and
   `ChannelBidiStreamSource` are ALPN-blind and call-protocol-blind — they
   depend on `alknet-core` only. The channel 0 pre-negotiation and lifecycle
   op registrations are the call-protocol coupling. Baking both into one
   crate means any consumer that wants the multiplexer also pulls in the
   call-protocol coupling, even if they don't need channel 0 to be
   `alknet/call`.

2. **The "no special-casing for downstream crates" principle is
   violated.** The user's constraint: "we don't want to be doing anything
   special for downstream crates unless it's needed/useful to the point of
   they all kind of benefit." The call-protocol coupling is a channels-crate
   concern (channels needs call for orchestration), not a downstream-crate
   concern. Separating them makes the dependency graph honest: the pure
   multiplexer has no opinion about channel 0; the call-protocol coupling
   is isolated where it belongs.

The substrate simplification (ADR-071 revised) strengthened this: the wire
format is the same across all substrates, and the `ChannelsAdapter` is
substrate-agnostic. The pure multiplexer (`channels-core`) is cleanly
separable from the call-protocol orchestration (`channels-call`).

## Decision

### Three crates

```
alknet-channels-core  — the pure multiplexer (wire format, demux/mux,
│                       ChannelBidiStreamSource, ChannelManager). Depends
│                       on alknet-core only. ALPN-blind, call-protocol-blind,
│                       transport-blind.
├── alknet-channels-call — channel 0 pre-negotiation + lifecycle op
│        │                registrations on the call protocol's
│        │                OperationRegistry. Depends on channels-core +
│        │                alknet-call.
│        ├── alknet-channels-hub — the relay (ADR-079). Depends on
│        │                   channels-call. The hub-side role.
│        └── alknet-channels-worker — ChannelClient (ADR-080). Depends on
│                            channels-call. The client/worker-side role.
```

### `alknet-channels-core`

The pure multiplexer. Contains:

- The 9-byte chunk wire format (`parse_header` / `write_header` — ADR-071)
- The demux/mux (`Demux`, `MuxHandle`/`MuxRunner` — ADR-075)
- `ChannelBidiStreamSource` (implements `BidiStreamSource` — ADR-074)
- `ChannelSubStreams` / `SubStreamHandle` (the typed destructure accessor)
- `ChannelManager` (the shared state — channel_id → ChannelState, but
  **without** the `call_ops: Arc<OperationRegistry>` field; the manager is
  ALPN-blind and call-protocol-blind)
- `ChannelsAdapter` (the `ProtocolHandler` on `alknet/channels` — the
  read/demux loop, substrate-agnostic per ADR-071 revised)

Depends on `alknet-core` only. No `alknet-call` dependency. No opinion about
what channel 0 carries — that's the consumer's concern.

The `ChannelsAdapter::handle` in `channels-core` does NOT preinstall channel
0 as `alknet/call`. It runs the demux loop and routes chunks by
`channel_id`. Channel 0 is just another channel; what ALPN it carries is
determined by the consumer (the `channels-call` crate pre-negotiates it as
`alknet/call`; a hypothetical other consumer could pre-negotiate it
differently).

### `alknet-channels-call`

The call-protocol coupling. Contains:

- Channel 0 pre-negotiation as `alknet/call` (ADR-072) — the
  `preinstall_channel_0` logic that constructs channel 0's reassembly
  buffers with stream_types [0, 1] and hands the `Connection` to the
  `CallAdapter`.
- The four lifecycle operations (ADR-073): `channel/open`,
  `channel/close`, `channel/control`, `channel/resources/subscribe` —
  registered on the call protocol's `OperationRegistry` at assembly time.
- `ChannelOperations` (the registration helper that closes over a
  `ChannelManager` clone).

Depends on `channels-core` + `alknet-call`. This is where the
call-protocol coupling lives, isolated from the pure multiplexer.

### `alknet-channels-hub` and `alknet-channels-worker`

The two roles. These may be sub-crates or feature-gated modules within
`channels-call`; the exact packaging is a two-way-door implementation
detail. The contract is:

- **`channels-hub`** (or the hub role): the relay (ADR-079). Translates
  `channel/open` on channel 0, byte-forwards data channels with
  `channel_id` rewrite. Depends on `channels-call` (for the call-protocol
  translation) + the hub's own `CallAdapter` / `from_call` machinery.
- **`channels-worker`** (or the worker/client role): `ChannelClient`
  (ADR-080). Dials a transport, establishes the channels connection, runs
  the demux/mux, exposes `open_channel(alpn, params) -> Channel`. Depends
  on `channels-call` (for channel 0 pre-negotiation) + `alknet-call`'s
  `CallClient`-equivalent.

The naming (hub/worker vs server/client) is a two-way-door detail. The
user noted "server/client" is muddy because a hub/worker can act as both
depending on the use case (bidirectionality — ADR-073 §direction
semantics). The roles are "the side that relays" (hub) and "the side that
dials" (worker/client), but both can open channels in either direction.

### What moves where

| Component | Original (ADR-071-080) | Now |
|-----------|------------------------|-----|
| 9-byte wire format | `alknet-channels` | `channels-core` |
| Demux/Mux | `alknet-channels` | `channels-core` |
| `ChannelBidiStreamSource` | `alknet-channels` | `channels-core` |
| `ChannelSubStreams` | `alknet-channels` | `channels-core` |
| `ChannelManager` (without `call_ops`) | `alknet-channels` | `channels-core` |
| `ChannelsAdapter` (demux loop only) | `alknet-channels` | `channels-core` |
| Channel 0 pre-negotiation | `alknet-channels` (ADR-072) | `channels-call` |
| `channel/open`/`close`/`control`/`resources/subscribe` ops | `alknet-channels` (ADR-073) | `channels-call` |
| `ChannelOperations` registration helper | `alknet-channels` | `channels-call` |
| Hub relay (ADR-079) | `alknet-channels` (spec) / `alknet-hub` (impl) | `channels-hub` (or `alknet-hub` — see below) |
| `ChannelClient` (ADR-080) | `alknet-channels` | `channels-worker` |

### Relationship to `alknet-hub`

The existing `alknet-hub` crate (`docs/architecture/crates/hub/README.md`)
is the hub pattern: peer lifecycle, aggregated env, service discovery. The
channels hub relay (ADR-079) is a channels-specific concern that the hub
crate consumes. The split: `channels-hub` (or the hub role within
`channels-call`) provides the relay logic; `alknet-hub` wires it into the
hub runtime alongside the call-protocol peer management. Whether the relay
lives in a `channels-hub` sub-crate or directly in `alknet-hub` is a
packaging decision — the contract (ADR-079) is the same either way. The
user's preference for decomposing channels into core/hub/worker suggests a
`channels-hub` sub-crate that `alknet-hub` depends on, but this is not
one-way and can be revisited during implementation.

## Consequences

**Positive:**
- The dependency graph is honest: `channels-core` is the pure multiplexer
  with no call dependency; the call-protocol coupling is isolated in
  `channels-call`. A consumer that wants the multiplexer without the
  call-protocol orchestration can depend on `channels-core` only.
- The "no special-casing for downstream crates" principle is preserved:
  `channels-core` doesn't know about `alknet-call`, `alknet-tty`, or any
  handler crate. The call-protocol coupling is a channels-crate concern,
  not a downstream-crate concern.
- The sub-crate split matches the existing pattern (`alknet-tty` +
  `alknet-tty-local`, `alknet-docker` + its `tty` feature) — core in one
  crate, consumer-specific wiring in another.
- The hub and worker roles are separated, matching the user's
  decomposition preference and the bidirectionality of the channels
  protocol (both sides can open channels; the roles are about who relays
  vs who dials, not about request/response direction).

**Negative:**
- Three crates instead of one. The assembly layer must depend on
  `channels-core` + `channels-call` (and optionally `channels-hub` or
  `channels-worker`) instead of one `alknet-channels`. This is the cost of
  the clean separation; the assembly layer already wires multiple crates,
  so this is consistent with the existing pattern.
- The `ChannelsAdapter` in `channels-core` doesn't preinstall channel 0 —
  the consumer does. This means `channels-core`'s `ChannelsAdapter::handle`
  exposes a hook (callback or trait method) for the consumer to install
  channel 0. `channels-call` provides the `preinstall_channel_0`
  implementation; a different consumer could provide a different one. This
  is a slightly more complex adapter shape than "channel 0 is always call,"
  but it's the cost of the clean separation.

## Door type

**One-way (crate structure) + two-way (role packaging).** The three-crate
split (`channels-core` / `channels-call` / roles) is one-way — once
consumers depend on `channels-core` without `channels-call`, re-merging
them is a breaking change. The hub/worker packaging (separate sub-crates
vs feature-gated modules within `channels-call`) is two-way — an
implementation detail that can change without breaking the contract.

## References

- ADR-003: crate decomposition (no-handler-depends-on-another-handler —
  preserved; the channels sub-crates depend on core/call, not on handlers)
- ADR-071: channels wire format (revised — substrate simplification; the
  wire format is in `channels-core`)
- ADR-072: channel 0 pre-negotiated (moves to `channels-call`)
- ADR-073: channel lifecycle operations (move to `channels-call`)
- ADR-074: ChannelBidiStreamSource (in `channels-core`)
- ADR-075: ChannelsAdapter and ChannelManager (split: core demux in
  `channels-core`, call coupling in `channels-call`)
- ADR-079: hub relay (in `channels-hub` or `alknet-hub`)
- ADR-080: ChannelClient (in `channels-worker`)
- `docs/architecture/crates/hub/README.md` — the existing hub crate (the
  relay's consumer)
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

### Two crates, not four

```
alknet-channels-core  — the pure multiplexer (wire format, demux/mux,
│                       ChannelBidiStreamSource, ChannelManager). Depends
│                       on alknet-core only. ALPN-blind, call-protocol-blind,
│                       transport-blind.
└── alknet-channels-call — channel 0 pre-negotiation + lifecycle op
                         registrations on the call protocol's
                         OperationRegistry. Depends on channels-core +
                         alknet-call.
```

There are no `channels-hub` or `channels-worker` sub-crates. The hub and
worker are **consumers** of channels, not sub-crates of it. The existing
`alknet-hub` crate (`docs/architecture/crates/hub/README.md`) IS the hub —
it depends on `channels-call` and uses the channels protocol as its
substrate. A worker is just a worker — it depends on `channels-call` and
uses `ChannelClient` (ADR-080) to dial. The hub relay logic (ADR-079) lives
in `alknet-hub`, not in a channels sub-crate.

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
- `ChannelClient` (ADR-080) — the client-side type that dials a transport,
  establishes the channels connection, and exposes `open_channel(alpn,
  params) -> Channel`. This is the worker/client entry point; it lives here
  because it needs channel 0 pre-negotiation (which is in `channels-call`).

Depends on `channels-core` + `alknet-call`. This is where the
call-protocol coupling lives, isolated from the pure multiplexer.

### Hub and worker are consumers, not sub-crates

The hub and worker are architectural roles, not channels sub-crates:

- **The hub** is the existing `alknet-hub` crate. It depends on
  `channels-call` and uses the channels protocol as its substrate. The hub
  relay logic (ADR-079 — translate `channel/open` on channel 0,
  byte-forward data channels with `channel_id` rewrite) lives in
  `alknet-hub`, alongside its existing peer lifecycle, aggregated env, and
  service discovery responsibilities. There is no `channels-hub` sub-crate;
  `alknet-hub` IS the channels hub.

- **A worker** is any crate that uses `ChannelClient` (ADR-080, in
  `channels-call`) to dial a hub. There is no `channels-worker` sub-crate;
  a worker depends on `channels-call` and uses `ChannelClient` directly.
  The worker may be a CLI binary, a docker-side connector, an SSH-side
  connector, or any other role that dials into a hub's channels connection.

This means the channels crate provides the substrate (`channels-core` +
`channels-call`); the hub and worker crates are consumers that build on it.
The dependency direction is: `alknet-hub` → `channels-call` →
`channels-core` → `alknet-core`; a worker → `channels-call` →
`channels-core` → `alknet-core`. The channels crate has no dependency on
`alknet-hub` or any worker crate.

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
| `ChannelClient` (ADR-080) | `alknet-channels` | `channels-call` |
| Hub relay (ADR-079) | `alknet-channels` (spec) | `alknet-hub` (the existing hub crate, consuming `channels-call`) |

### Relationship to `alknet-hub`

The existing `alknet-hub` crate (`docs/architecture/crates/hub/README.md`)
is the hub pattern: peer lifecycle, aggregated env, service discovery. With
channels as the substrate, `alknet-hub` gains a dependency on
`channels-call` and incorporates the relay logic (ADR-079). The hub spec
(`crates/hub/README.md`) will be updated to reflect that the hub uses
channels as its transport substrate — one channels connection per leg
(browser↔hub, hub↔spoke), with the relay translating `channel/open` and
byte-forwarding data channels. The hub's existing responsibilities (peer
lifecycle, aggregated env, service discovery, worker supervision) are
unchanged; channels is the substrate they run on.

This makes "channels hub" and "hub" the same thing — the hub IS built on
channels. There is no separate channels-hub concept.

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
- Hub and worker are consumers, not sub-crates. The existing `alknet-hub`
  crate IS the channels hub — it depends on `channels-call` and uses
  channels as its substrate. A worker depends on `channels-call` and uses
  `ChannelClient`. The channels crate has no dependency on `alknet-hub` or
  any worker crate. This is the cleanest dependency direction: channels
  provides the substrate; hub and worker consume it.
- The WASM and cross-platform story gets easier: `channels-core` is
  WASM-compatible by construction (pure byte manipulation, no platform
  deps); `channels-call` inherits the call protocol's WASM constraints; the
  hub and worker crates are platform-specific as needed.

**Negative:**
- Two channels crates instead of one. The assembly layer must depend on
  `channels-core` + `channels-call` instead of one `alknet-channels`. This
  is the cost of the clean separation; the assembly layer already wires
  multiple crates, so this is consistent with the existing pattern.
- The `ChannelsAdapter` in `channels-core` doesn't preinstall channel 0 —
  the consumer does. This means `channels-core`'s `ChannelsAdapter::handle`
  exposes a hook (callback or trait method) for the consumer to install
  channel 0. `channels-call` provides the `preinstall_channel_0`
  implementation; a different consumer could provide a different one. This
  is a slightly more complex adapter shape than "channel 0 is always call,"
  but it's the cost of the clean separation.

## Door type

**One-way (crate structure).** The two-crate split (`channels-core` /
`channels-call`) is one-way — once consumers depend on `channels-core`
without `channels-call`, re-merging them is a breaking change. The hub and
worker being consumers (not sub-crates) is also one-way — it establishes
the dependency direction (hub/worker → channels, not channels → hub/worker).

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
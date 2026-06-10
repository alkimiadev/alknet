# iroh-gossip: HyParView Membership Protocol

## Overview

The HyParView protocol provides **swarm membership management** — it maintains which peers are currently part of the swarm for a given topic and ensures the overlay network remains connected even as nodes join, leave, or fail.

It is implemented in `src/proto/hyparview.rs`.

## Core Concept: Two Views

Each peer maintains two sets of peers:

| View | Description | Default Size | Connection? |
|------|-------------|--------------|-------------|
| **Active View** | Peers we maintain active bidirectional connections to | 5 | Yes — TCP/QUIC connection is kept open |
| **Passive View** | An address book of peers we know about but are not connected to | 30 | No — just contact information |

Key invariants:
- **Active connections are always bidirectional**: If peer A has peer B in its active view, peer B also has peer A in its active view.
- The passive view serves as a **failover pool**: When an active peer disconnects, a random peer from the passive view is promoted to fill the slot.

## Configuration (`hyparview::Config`)

```rust
pub struct Config {
    pub active_view_capacity: usize,          // Default: 5
    pub passive_view_capacity: usize,          // Default: 30
    pub active_random_walk_length: Ttl,        // Default: Ttl(6)
    pub passive_random_walk_length: Ttl,       // Default: Ttl(3)
    pub shuffle_random_walk_length: Ttl,       // Default: Ttl(6)
    pub shuffle_active_view_count: usize,      // Default: 3
    pub shuffle_passive_view_count: usize,     // Default: 4
    pub shuffle_interval: Duration,            // Default: 60s
    pub neighbor_request_timeout: Duration,    // Default: 500ms
}
```

These defaults come directly from the HyParView paper (p9), except for `shuffle_interval` and `neighbor_request_timeout` which are "wild guesses" in the code.

## State Structure

```rust
pub struct State<PI, RG = ThreadRng> {
    me: PI,                                    // Our peer identity
    me_data: Option<PeerData>,                 // Opaque data we share with peers
    pub active_view: IndexSet<PI>,             // Connected peers
    pub passive_view: IndexSet<PI>,            // Known but disconnected peers
    config: Config,
    shuffle_scheduled: bool,                   // Whether shuffle timer is active
    rng: RG,                                   // Random number generator
    stats: Stats,
    pending_neighbor_requests: HashSet<PI>,     // Peers we've sent Neighbor to but no reply yet
    peer_data: HashMap<PI, PeerData>,          // Opaque data received from other peers
    alive_disconnect_peers: HashSet<PI>,       // Peers disconnecting but to keep in passive view
}
```

## Messages (`hyparview::Message`)

| Message | Direction | Purpose |
|---------|-----------|---------|
| `Join(Option<PeerData>)` | New node → Contact | Sent to a known peer to join the swarm |
| `ForwardJoin(ForwardJoin)` | Propagated | Forwarded to active view to introduce a new member |
| `Neighbor(Neighbor)` | Bidirectional | Request to add sender to active view (with priority) |
| `Disconnect(Disconnect)` | Bidirectional | Notification that a peer is leaving or being demoted |
| `Shuffle(Shuffle)` | Initiated periodically | Sent to random peer to exchange passive view contacts |
| `ShuffleReply(ShuffleReply)` | Reply to Shuffle | Returns a random subset of our views to the origin |

### Message Details

```rust
pub struct ForwardJoin<PI> {
    peer: PeerInfo<PI>,   // The new peer's identity + optional data
    ttl: Ttl,             // Time-to-live, decremented per hop
}

pub struct Shuffle<PI> {
    origin: PI,           // Who initiated the shuffle
    nodes: Vec<PeerInfo<PI>>,  // Random subset of our views
    ttl: Ttl,             // Time-to-live for the random walk
}

pub struct Neighbor {
    priority: Priority,   // High (cannot be denied) or Low (can be denied)
    data: Option<PeerData>,
}

pub struct Disconnect {
    alive: bool,          // If true, peer is still alive (just demoting)
    _respond: bool,       // Obsolete, kept for wire compat
}
```

## Join Procedure (Step by Step)

1. A new node sends `Join(me_data)` to a known contact peer.
2. The contact peer adds the new node to its active view (even evicting a random peer if necessary).
3. The contact peer forwards `ForwardJoin` to all other peers in its active view with `TTL = active_random_walk_length`.
4. Each peer receiving `ForwardJoin`:
   - If `TTL == 0` or active view has ≤1 peer: sends `Neighbor(High)` to the new node (which adds it to active view).
   - If `TTL == passive_random_walk_length`: adds the new node to passive view.
   - Decrements TTL and forwards to a random active peer (different from sender).

5. The `Neighbor` message establishes the bidirectional active connection. A `Priority::High` neighbor request **must** be accepted (potentially evicting a random active peer). A `Priority::Low` request is only accepted if there is room.

## Shuffle Mechanism

Periodically (every `shuffle_interval`), each node:
1. Picks a random active peer.
2. Sends `Shuffle` containing a random subset of active + passive views plus the origin's info, with a TTL.
3. The shuffle message does a random walk (each hop decrements TTL).
4. When TTL reaches 0 or the active view is ≤1, the peer accepts the shuffle and replies with `ShuffleReply` containing its own random peers.
5. The origin receives `ShuffleReply` and adds new peers to its passive view.

This ensures the passive view remains fresh and provides good connectivity even in dynamic networks.

## Failure Recovery

When a peer in the active view disconnects (detected via `PeerDisconnected`):
1. The peer is removed from the active view.
2. A `NeighborDown` event is emitted.
3. A random peer from the passive view is selected and sent a `Neighbor(Low)` request.
4. If that peer doesn't respond within `neighbor_request_timeout`, it's removed from the passive view and another peer is tried.
5. This continues until a connection is established or the passive view is exhausted.

If a `Disconnect(alive=true)` message is received:
- The peer is moved to the passive view (not just dropped), because it's still alive.
- The `alive_disconnect_peers` set tracks which disconnected peers should be retained in passive view when their connection eventually closes.

## PeerData

`PeerData` is an opaque `Bytes` type that peers exchange when joining. In the `net` module, it is used to serialize and transmit addressing information (`AddrInfo`):

```rust
struct AddrInfo {
    relay_url: Option<RelayUrl>,
    direct_addresses: BTreeSet<SocketAddr>,
}
```

This allows the gossip protocol itself to help propagate connectivity information, enabling the `GossipAddressLookup` service to feed addresses back into iroh's endpoint discovery system.

## Events (`hyparview::Event`)

| Event | Meaning |
|-------|---------|
| `NeighborUp(PI)` | A peer was added to our active view |
| `NeighborDown(PI)` | A peer was removed from our active view |

These events are forwarded up to the PlumTree layer and to the application.

## Timers

| Timer | Purpose |
|-------|---------|
| `DoShuffle` | Periodically trigger a shuffle operation |
| `PendingNeighborRequest(PI)` | Timeout for a pending neighbor request |

## IO Trait Pattern

The HyParView state machine is generic over an `IO` trait:

```rust
pub trait IO<PI: Clone> {
    fn push(&mut self, event: impl Into<OutEvent<PI>>);
}
```

This allows the protocol to emit output events without knowing about the networking layer. The upper layers supply a `VecDeque<OutEvent>` or similar container.
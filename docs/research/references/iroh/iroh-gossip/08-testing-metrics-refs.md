# iroh-gossip: Testing & Simulation

## Test Infrastructure

The crate includes two layers of testing:

### 1. Unit Tests (in source files)

Unit tests are embedded in each module file behind `#[cfg(test)]`:

| Module | Tests |
|--------|-------|
| `proto/hyparview.rs` | Not shown (would be in the file) |
| `proto/plumtree.rs` | `optimize_tree`, `spoofed_messages_are_ignored`, `cache_is_evicted` |
| `proto.rs` | `hyparview_smoke`, `plumtree_smoke`, `quit` |
| `net.rs` | `gossip_net_smoke`, `subscription_cleanup` |
| `api.rs` | `test_rpc`, `ensure_gossip_topic_is_sync` |
| `proto/util.rs` | `indexset`, `timer_map`, `hex`, `time_bound_cache` |

### 2. Protocol Simulator (`proto::sim`)

The `sim` module (behind `test-utils` feature) provides a deterministic network simulator:

```rust
// Available when feature = "test-utils"
pub mod sim;
```

This allows testing the protocol logic without any real networking, using seeded RNG for reproducibility.

The simulator creates a `Network` of virtual nodes, each running their own `proto::State`. Events are processed in discrete "trips" (round-trips), allowing controlled testing of protocol behavior.

### 3. Simulation Binary (`sim` feature)

The crate includes a CLI simulator (behind `simulator` feature) that can run large-scale simulations:

```
cargo run --bin sim --features simulator
```

This uses `rayon` for parallel execution and `comfy-table` for result output.

### 4. Integration Tests (`tests/sim.rs`)

Behind the `test-utils` feature, provides end-to-end protocol testing.

## Key Test Patterns

### Protocol-Level Smoke Test

From `proto.rs`:

```rust
#[test]
fn hyparview_smoke() {
    let rng = ChaCha12Rng::seed_from_u64(0);
    let mut config = Config::default();
    config.membership.active_view_capacity = 2;
    let mut network = Network::new(config.into(), rng);
    for i in 0..4 { network.insert(i); }
    let t: TopicId = [0u8; 32].into();
    
    // Join nodes
    network.command(0, t, Command::Join(vec![1, 2]));
    network.command(1, t, Command::Join(vec![2]));
    network.command(2, t, Command::Join(vec![]));
    network.run_trips(3);
    
    // Verify events and connections
    assert_eq!(network.events_sorted(), expected);
    assert_eq!(network.conns(), vec![(0, 1), (0, 2), (1, 2)]);
    assert!(network.check_synchronicity());
}
```

### PlumTree Optimization Test

From `plumtree.rs`:

```rust
#[test]
fn optimize_tree() {
    // When an IHave message arrives with fewer hops than the Gossip message,
    // and the difference exceeds optimization_threshold, the tree is restructured:
    // - The IHave sender is promoted to eager (Graft)
    // - The Gossip sender is demoted to lazy (Prune)
}
```

### Spoofed Message Test

```rust
#[test]
fn spoofed_messages_are_ignored() {
    // Messages where MessageId != blake3(content) are silently discarded
    let message = Message::Gossip(Gossip {
        content: content.clone(),
        id: MessageId::from_content(b"wrong_content"),  // Spoofed!
        scope: DeliveryScope::Swarm(Round(1)),
    });
    state.handle(InEvent::RecvMessage(2, message), now, &mut io);
    // No events are emitted
}
```

### Networking Smoke Test

From `net.rs`:

```rust
#[tokio::test]
async fn gossip_net_smoke() {
    // Creates 3 endpoints with a relay server
    // Subscribes and joins a topic
    // Broadcasts messages and verifies reception
    // Uses real QUIC connections via iroh
}
```

## Metrics

The `Metrics` struct (in `src/metrics.rs`) uses `iroh_metrics::MetricsGroup`:

```rust
#[derive(Debug, Default, MetricsGroup)]
#[metrics(name = "gossip")]
pub struct Metrics {
    pub msgs_ctrl_sent: Counter,
    pub msgs_ctrl_recv: Counter,
    pub msgs_data_sent: Counter,
    pub msgs_data_recv: Counter,
    pub msgs_data_sent_size: Counter,
    pub msgs_data_recv_size: Counter,
    pub msgs_ctrl_sent_size: Counter,
    pub msgs_ctrl_recv_size: Counter,
    pub neighbor_up: Counter,
    pub neighbor_down: Counter,
    pub actor_tick_main: Counter,
    pub actor_tick_rx: Counter,
    pub actor_tick_endpoint: Counter,
    pub actor_tick_dialer: Counter,
    pub actor_tick_dialer_success: Counter,
    pub actor_tick_dialer_failure: Counter,
    pub actor_tick_in_event_rx: Counter,
    pub actor_tick_timers: Counter,
}
```

These are tracked both in the protocol state machine (for message counts) and in the actor event loop (for tick-level diagnostics). When the `metrics` feature is enabled, they are exported via Prometheus-compatible endpoints.

## References

### Academic Papers

- **HyParView**: Leitao, J., Pereira, J., & Rodrigues, L. (2007). "HyParView: A Membership Protocol for Reliable Gossip Multicast with Dense Coverage." [PDF](https://asc.di.fct.unl.pt/~jleitao/pdf/dsn07-leitao.pdf)
- **PlumTree**: Leitao, J., Pereira, J., & Rodrigues, L. (2007). "Epidemic Broadcast Trees." [PDF](https://asc.di.fct.unl.pt/~jleitao/pdf/srds07-leitao.pdf)

### Implementation Reference

- Bartosz Sypytkowski's example implementation: [gist](https://gist.github.com/Horusiath/84fac596101b197da0546d1697580d99)

### Related Projects

- [iroh](https://docs.rs/iroh) — The networking library that iroh-gossip integrates with
- [Earthstar](https://github.com/earthstar-project/earthstar) — Another PlumTree implementation referenced in code comments

### Crate Repository

- [github.com/n0-computer/iroh-gossip](https://github.com/n0-computer/iroh-gossip)
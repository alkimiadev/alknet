# iroh-live: Network Signals and Adaptive Bitrate

## NetworkSignals

Produced by polling iroh QUIC connection stats. Consumed by `VideoTrack::enable_adaptation()` to decide when to switch video renditions.

```rust
pub struct NetworkSignals {
    pub rtt: Duration,              // Round-trip time to remote peer
    pub loss_rate: f64,             // Recent packet loss rate (0.0..=1.0), 200ms delta window
    pub available_bps: u64,         // Estimated available bandwidth (cwnd * 8 / rtt)
    pub congestion_events: u64,    // Monotonically increasing congestion counter
}
```

### Production

`spawn_signal_producer()` in `iroh-live/src/util.rs` polls every 200ms:

1. Gets connection paths via `conn.paths().get()`
2. Finds the selected path (`is_selected()`)
3. Reads path stats (`lost_packets`, `udp_tx.datagrams`, `cwnd`) and RTT
4. Computes delta-based loss rate: `delta_lost / (delta_sent + delta_lost)`
5. Estimates bandwidth: `cwnd * 8 * 1e9 / rtt_ns`
6. Writes to `watch::Sender<NetworkSignals>`

Also: `spawn_stats_recorder()` records into `NetStats` for the debug overlay (RTT, loss%, bandwidth in/out, path type).

## Adaptive Rendition Algorithm

Located in `moq-media/src/adaptive.rs`. The algorithm evaluates `NetworkSignals` against configured thresholds and produces `Decision` values.

### Configuration (`AdaptiveConfig`)

| Parameter | Default | Description |
|-----------|---------|-------------|
| `upgrade_hold` | 4s | Sustained good conditions before upgrade probe |
| `downgrade_hold` | 500ms | Sustained bad conditions before downgrade |
| `probe_duration` | 3s | How long a probe runs before committing |
| `probe_cooldown` | 8s | Cooldown after a failed probe |
| `post_downgrade_cooldown` | 4s | Cooldown after any downgrade |
| `loss_downgrade` | 10% | Loss rate threshold for downgrade |
| `loss_emergency` | 20% | Loss rate for immediate drop to lowest |
| `loss_good` | 2% | Loss rate considered "good" |
| `loss_probe_abort` | 5% | Loss rate that aborts an active probe |
| `bw_downgrade_ratio` | 85% | Bandwidth utilization ceiling for downgrade |
| `bw_probe_headroom` | 120% | Required excess bandwidth for probe |
| `check_interval` | 200ms | How often adaptation task checks signals |

### Decision Logic

```
1. Emergency: loss >= 20% AND not already lowest → Drop to lowest immediately

2. Downgrade check: 
   - bandwidth_stressed (available < current_bitrate * 85%) OR loss >= 10%
   - sustained for downgrade_hold (500ms) → Downgrade(next_lower)

3. Upgrade check:
   - Already at highest → Hold
   - Within post_downgrade_cooldown (4s) → Hold
   - Within probe_cooldown (8s) → Hold  
   - bandwidth_headroom (available >= next_higher_bitrate * 120%) AND loss <= 2%
   - sustained for upgrade_hold (4s) → StartProbe(next_higher)

4. Otherwise: Hold
```

### Probe Lifecycle

When `StartProbe(idx)` is decided:
1. Create a new decoder pipeline for the higher rendition
2. Write frames to the same `FrameSender` (seamless switch for the consumer)
3. Monitor signals during the probe period
4. If `should_abort_probe()` (loss ≥ 5% or new congestion events) → abort, drop probe pipeline, cooldown 8s
5. If probe duration (3s) passes without abort → commit, replace current pipeline

### Rendition Ranking

```rust
pub fn rank_renditions(renditions: &BTreeMap<String, VideoConfig>) -> Vec<RankedRendition>
```

Sorts by pixel count descending (highest quality = index 0). Each `RankedRendition` carries name, pixels, bitrate_bps, width, height.

### RenditionMode

```rust
pub enum RenditionMode {
    Auto,           // Algorithm-driven switching
    Fixed(String),  // Pin to a specific rendition
}
```

Controlled via `VideoTrack::set_rendition_mode()`. In Fixed mode, the algorithm switches directly to the named rendition without probing.
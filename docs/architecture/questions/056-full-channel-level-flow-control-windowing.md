# OQ-56: Full Channel-Level Flow-Control Windowing

- **Origin**: `docs/research/alknet-channels/phase-0-findings.md` §DP-5,
  §OQ-CH-03; `docs/architecture/decisions/076-backpressure-channel-limits-id-reuse.md`
- **Status**: deferred(scope)
- **Door type**: two-way (additive — per-channel window tracking does not
  change the wire format)
- **Priority**: low
- **Blocked on**: a real deployment observes head-of-line blocking on a
  saturated channel where the bounded-buffer's stop-reading mitigation is
  insufficient. The trigger is specific: a channel whose consumer is
  persistently slower than its producer, causing the demux to stall that
  channel's reads frequently enough that other channels' throughput is
  measurably affected. The intended use cases (TTY, SSH, tunnels) are not
  high-throughput in the HOL-blocking sense; the trigger requires a
  high-throughput use case (e.g., file transfer over a tunnel) that
  saturates a channel.
- **Resolution**: Not yet decidable. The bounded-buffer backpressure
  (ADR-076, default 1 MiB per `(channel_id, stream_type)`) is the decided
  v1 mechanism — validated by the POC's 1 MiB `tunnel_large_payload` test
  with no deadlock and no cross-channel blocking. Full channel-level
  windowing (SSH-style sliding-window per channel) is an additive extension
  that does not change the wire format; it adds per-channel window tracking
  to the demux/mux. The decision to add it depends on whether the
  bounded-buffer mitigation is sufficient in practice, which can only be
  determined by a deployment that hits the limitation.
- **What does NOT block on this**: the bounded-buffer mechanism is decided
  and is the v1 implementation. Full windowing is an extension, not a
  prerequisite. The channels crate ships with bounded-buffer backpressure;
  full windowing is added if and only if the trigger condition is observed.
- **Cross-references**: ADR-076 (bounded-buffer decision), ADR-071 (wire
  format — unchanged by windowing extension),
  `docs/research/alknet-channels/poc-summary.md` §POC Target 1
  (backpressure validation)
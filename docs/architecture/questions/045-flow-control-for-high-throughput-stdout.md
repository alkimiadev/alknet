# OQ-45: Flow Control for High-Throughput stdout

- **Origin**: `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-03;
  [crates/tty/tty-wire.md](crates/tty/tty-wire.md) (no windowing in the
  chunk format).
- **Status**: open (low risk)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: The chunk format has no windowing — QUIC's per-stream
  flow control handles backpressure. This is expected to suffice for
  high-throughput stdout (e.g., `cargo build` output): the docker POC's
  logs subscription handled multi-line output without issue, and QUIC's
  bidi-stream flow control is the established backpressure mechanism. A
  POC with real high-volume output (a deliberately large `cargo build`
  or `find /` over a high-bandwidth connection) would confirm. If a
  flow-control problem surfaces, the reversal is a per-stream
  flow-control window in the chunk format (a two-way-door extension to
  the wire format, additive — a new control message type for window
  updates, not a header change). Not blocking; the default assumption is
  QUIC flow control suffices.
- **Cross-references**: ADR-052, [tty-wire.md](crates/tty/tty-wire.md),
  [tty-adapter.md](crates/tty/tty-adapter.md)

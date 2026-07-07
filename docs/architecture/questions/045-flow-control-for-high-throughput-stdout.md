# OQ-45: Flow Control for High-Throughput stdout

- **Origin**: `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-03;
  [crates/tty/tty-wire.md](crates/tty/tty-wire.md) (no windowing in the
  chunk format).
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: No application-level windowing. QUIC's per-stream flow
  control is the backpressure mechanism — the chunk format carries no
  window. This is decided, not a default assumption.

  The backpressure chain is complete by construction — every link awaits
  its producer, so slowness at the client's read end propagates all the
  way back to the child process's stdout write without any unbounded
  buffering link:

  1. **Client reads slowly → QUIC flow control.** The server's
     `write_chunk` writes into a quinn `SendStream`, which respects
     per-stream flow control. When the client's receive window fills,
     the send awaits. (Standard, mature quinn behavior — not something
     alknet-tty inventes.)
  2. **Send awaits → drainer channel backpressures.** The drainer task
     writes chunks to the client one at a time. When `write_chunk`
     awaits, the drainer stops pulling from the bounded `writer_rx`
     channel (depth 64 in the POC's `session.rs`).
  3. **Drainer channel fills → stdout pump backpressures.** The
     stdout→client pump does `writer_tx.send(chunk).await`; when the
     drainer isn't pulling, the send awaits.
  4. **Stdout pump awaits → backend stdout channel backpressures.** The
     pump stops receiving from the backend's stdout channel (the local
     backend's `mpsc::Receiver<Bytes>`, also bounded at 64).
  5. **Stdout channel fills → reader thread backpressures.** The
     backend's reader thread (`portable_pty` master reader, or a piped
     `Child::stdout`) does `blocking_send`; when the channel is full,
     the std thread blocks and stops reading from the kernel.
  6. **Reader thread stops → OS pipe/PTY buffer fills → process blocks.**
     The kernel PTY buffer (or `Stdio::piped()` buffer, typically 64 KiB)
     fills, and the child process's `write()` to stdout blocks. The
     process is throttled.

  Every link in the chain awaits its producer. The chain is the standard
  composition of QUIC flow control + bounded tokio channels + OS pipe
  buffers — the same pattern the docker POC's logs subscription and the
  tty POC's PTY pump already use. A high-throughput stdout workload
  (e.g., `cargo build` output) throttles at the process when the client
  can't keep up; no unbounded buffer breaks the chain.

  The two-way-door reversal — a per-stream window-update control message
  on stream_type 3 — is an additive extension to the control channel
  (a new `ControlMessage` variant), not a wire-format header change. It
  is not the expected path; it is noted in ADR-052's consequences as the
  cheap reversal if a flow-control problem ever surfaces that QUIC's
  defaults cannot handle (e.g., a pathological stream that needs
  sub-QUIC-window backpressure signaling). Tuning concerns (read buffer
  size, channel depth) are implementation-level, not architectural, and
  don't warrant an ADR.

- **Cross-references**: ADR-052, [tty-wire.md](crates/tty/tty-wire.md),
  [tty-adapter.md](crates/tty/tty-adapter.md),
  `/workspace/alknet-tty-poc/src/session.rs` (the three-pump driver —
  the bounded channels at every link),
  `/workspace/alknet-tty-poc/src/local_pty.rs` (the blocking→async
  bridge — the reader thread that backpressures into the kernel).
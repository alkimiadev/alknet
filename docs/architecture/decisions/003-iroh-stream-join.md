# ADR-003: iroh Stream via tokio::io::join

## Status
Accepted

## Context
iroh's QUIC implementation provides separate `RecvStream` (implements `AsyncRead`) and `SendStream` (implements `AsyncWrite`) for each bidirectional channel opened via `open_bi()` / `accept_bi()`. russh's `connect_stream()` and `run_stream()` require a single type implementing both `AsyncRead` and `AsyncWrite`.

Options considered:
1. `tokio::io::join(recv, send)` — Combines the two halves into `Join<RecvStream, SendStream>` which implements both traits.
2. Custom `IrohStream` wrapper — A struct with `recv` and `send` fields that delegates `AsyncRead` to `recv` and `AsyncWrite` to `send`.
3. Using iroh's `Connection` directly — Opening a new `open_bi()` for each SSH channel instead of running SSH over a single stream.

## Decision
Use `tokio::io::join(recv_stream, send_stream)` (Option 1).

One line of code, correct trait implementations, no custom types needed. The `Join<A, B>` type implements `AsyncRead` using `A` and `AsyncWrite` using `B`, which maps directly to iroh's split stream model.

If profiling later shows overhead (unlikely — it's just method dispatch), we can switch to a custom wrapper. But YAGNI until demonstrated.

Option 3 was rejected because it would require modifying russh to understand iroh connections. The whole point of the transport trait is that SSH doesn't know about iroh.

## Consequences
- **Positive**: Minimal code. One line to bridge iroh and russh.
- **Positive**: No custom types to maintain.
- **Positive**: Correct `AsyncRead` + `AsyncWrite` behavior — `Poll::Pending` on one half doesn't affect the other.
- **Negative**: None identified. The `Join` type is a standard tokio combinator with well-tested semantics.

## References
- [transport.md](../transport.md)
- [Feasibility assessment §11](../../research/feasibility/ssh-tunnel-vpn-alternative-feasibility.md)
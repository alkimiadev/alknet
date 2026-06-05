# ADR-001: Pluggable Transport via AsyncRead+AsyncWrite Trait

## Status
Accepted

## Context
Alknet needs to support multiple transport modes (TCP, TLS, iroh) for SSH sessions. Each mode has different connection establishment logic but produces the same result: a bidirectional byte stream. Without an abstraction, each transport would need its own SSH connection code path.

russh's `client::connect_stream()` and `server::run_stream()` both accept `AsyncRead + AsyncWrite + Unpin + Send`, meaning SSH is already transport-agnostic at the API level. The design question is whether to enshrine this in alknet's own type system or handle each transport case-by-case.

## Decision
Define a `Transport` trait that produces `AsyncRead + AsyncWrite + Unpin + Send` streams. Each transport (TCP, TLS, iroh) implements this trait. The SSH layer calls `transport.connect()` and passes the result to `russh::client::connect_stream()`.

On the server side, define a `TransportAcceptor` trait that produces incoming streams. Each acceptor (TCP listener, TLS listener, iroh endpoint) implements this trait. The server calls `acceptor.accept()` and passes the result to `russh::server::run_stream()`.

This makes adding a new transport (e.g., WebSocket, QUIC directly) a matter of implementing the trait, not modifying SSH code.

## Consequences
- **Positive**: Clean separation between transport and protocol. Adding transports is additive. SSH code is transport-agnostic.
- **Positive**: Testing is simplified — mock transports can produce in-memory streams.
- **Negative**: Slight indirection for the single-transport case (just TCP). The trait boilerplate is minimal though.
- **Negative**: The trait must be object-safe if we want dynamic dispatch. Using `impl Trait` in function signatures avoids this but limits runtime transport selection. CLI-selected transport needs dynamic dispatch: `Box<dyn Transport<Stream = Box<dyn AsyncRead+AsyncWrite+Unpin+Send>>>`.

## References
- [transport.md](../transport.md)
- [Feasibility assessment §3](../../../../conversations/research/ssh-tunnel-vpn-alternative-feasibility.md)
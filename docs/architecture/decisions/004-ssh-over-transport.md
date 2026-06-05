# ADR-004: SSH Runs Over Transport, Not Alongside

## Status
Accepted

## Context
There are two ways to structure the relationship between SSH and the transport layer:

1. **SSH over transport**: The transport produces one duplex stream. The entire SSH session (handshake, key exchange, channel multiplexing) runs over that single stream via `connect_stream()` / `run_stream()`. SSH has no direct network access.

2. **Transport alongside SSH**: SSH manages its own TCP connections via `connect()` / `run()`. The transport layer is an additional feature that wraps outgoing connections. SSH knows about the network.

## Decision
SSH runs over the transport (Option 1). The SSH layer never opens its own sockets or knows what transport it's on.

This is directly enabled by russh's `connect_stream()` and `run_stream()` APIs, which accept any `AsyncRead+AsyncWrite+Unpin+Send`. SSH's entire interaction with the network goes through the single stream produced by the transport.

## Consequences
- **Positive**: Adding a new transport requires implementing the `Transport` trait, not modifying SSH code.
- **Positive**: Testing is straightforward — mock transports produce in-memory streams.
- **Positive**: Security audit is clean — the SSH implementation has no network-facing code.
- **Positive**: The transport can be layered. Iroh connecting through a SOCKS5 proxy (which itself tunnels through alknet) is just a transport that calls out to a SOCKS5 library before establishing the QUIC connection.
- **Negative**: SSH keepalive and reconnection must be handled at the transport level. If the transport stream dies, the SSH session dies. Reconnection means establishing a new transport + new SSH session. There's no "SSH reconnects over the same transport" — you get a new session.
- **Negative**: Multiple SSH sessions over the same iroh connection require the iroh `Endpoint` (not stream) to be shared between sessions. The transport trait produces one stream per `connect()` call. The iroh `Endpoint` must be created externally and shared. (The `IrohTransport` struct holds an `Arc<Endpoint>`.)

## References
- [transport.md](../transport.md)
- [Feasibility assessment §3.4](../../../../conversations/research/ssh-tunnel-vpn-alternative-feasibility.md)
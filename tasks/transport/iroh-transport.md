---
id: transport/iroh-transport
name: Implement IrohTransport and IrohAcceptor (feature-gated iroh)
status: pending
depends_on:
  - transport/trait-and-types
  - transport/tcp-transport
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Implement iroh QUIC P2P transport. Per ADR-003, use `tokio::io::join(recv_stream, send_stream)` to combine iroh's split QUIC streams into a single duplex stream that russh can consume.

Client-side: `IrohTransport` connects to a remote iroh endpoint, opens a bidirectional QUIC stream, and joins the halves.
Server-side: `IrohAcceptor` creates an iroh endpoint, accepts incoming connections, accepts bidirectional streams.

iroh supports proxy configuration natively via `Endpoint::builder()`, which is how `--proxy` works with iroh transport (ADR-010).

Feature-gated behind `iroh` feature flag.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/transport/iroh.rs` (behind `#[cfg(feature = "iroh")]`)
- [ ] `IrohTransport` holds: target endpoint ID (base58-decoded to `NodeId`), relay URL, optional proxy URL
- [ ] `IrohTransport::connect()` calls `endpoint.connect(node_id, alpn)`, then `conn.open_bi()`, then `tokio::io::join(recv, send)`
- [ ] ALPN value is `b"alknet-ssh"`
- [ ] `IrohTransport::describe()` returns e.g. `"iroh://<endpoint-id>"`
- [ ] `IrohAcceptor` holds an `iroh::Endpoint` instance
- [ ] `IrohAcceptor::bind()` creates endpoint with relay URL and optional proxy config
- [ ] `IrohAcceptor::accept()` calls `endpoint.accept()`, then `conn.accept_bi()`, then `tokio::io::join(recv, send)`
- [ ] `IrohAcceptor` exposes `endpoint_id()` returning base58-encoded node ID for CLI display
- [ ] Default relay is n0's `https://relay.iroh.network/` (ADR-009)
- [ ] Proxy URL passed to `Endpoint::builder()` for outbound proxy (ADR-010)
- [ ] `TransportInfo.transport_kind` is `TransportKind::Iroh { endpoint_id }`
- [ ] Module re-exported from `transport/mod.rs` behind `#[cfg(feature = "iroh")]`
- [ ] Unit tests: endpoint creation, stream join produces correct type
- [ ] Integration test: iroh client connects to iroh server, stream is duplex (may need iroh relay, mark `#[ignore]` for CI)

## References

- docs/architecture/transport.md — IrohTransport row, iroh stream join, relay config
- docs/architecture/decisions/003-iroh-stream-join.md — tokio::io::join rationale
- docs/architecture/decisions/009-default-iroh-relay.md — default relay
- docs/architecture/decisions/010-transport-chaining-cli.md — proxy configuration

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
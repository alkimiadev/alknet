---
id: transport/trait-and-types
name: Define Transport trait, TransportAcceptor trait, TransportInfo, and TransportKind types
status: pending
depends_on:
  - setup/project-init
scope: narrow
risk: low
impact: phase
level: implementation
---

## Description

Define the core transport abstraction types that everything else builds on. This is the foundation per ADR-001: a `Transport` trait that produces `AsyncRead + AsyncWrite + Unpin + Send` streams, and a `TransportAcceptor` trait for the server side.

The `TransportInfo` and `TransportKind` types carry metadata about incoming connections (remote address, transport kind) which the server handler needs for logging and auth decisions.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/transport/mod.rs` exports `Transport` trait, `TransportAcceptor` trait, `TransportInfo`, `TransportKind`
- [ ] `Transport` trait: `async fn connect(&self) -> Result<Self::Stream>` where `Self::Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static`
- [ ] `Transport::describe(&self) -> String` for human-readable logging
- [ ] `TransportAcceptor` trait: `async fn accept(&self) -> Result<(Self::Stream, TransportInfo)>` with same stream bounds
- [ ] `TransportInfo { remote_addr: Option<SocketAddr>, transport_kind: TransportKind }`
- [ ] `TransportKind` enum: `Tcp`, `Tls { server_name: Option<String> }`, `Iroh { endpoint_id: String }`
- [ ] Traits are `Send + Sync + 'static`
- [ ] Re-exported from `crates/alknet-core/src/lib.rs`
- [ ] Unit tests verifying trait objects can be constructed (trait is object-safe with `Box<dyn Transport<Stream = ...>>`)
- [ ] Documentation comments on all public types referencing ADR-001, ADR-004

## References

- docs/architecture/transport.md — Transport trait, TransportAcceptor trait, TransportInfo, TransportKind definitions
- docs/architecture/decisions/001-pluggable-transport.md — pluggable transport rationale
- docs/architecture/decisions/004-ssh-over-transport.md — SSH runs over transport

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
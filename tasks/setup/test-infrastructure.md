---
id: setup/test-infrastructure
name: Set up test infrastructure with tokio test helpers and integration test skeleton
status: pending
depends_on:
  - setup/project-init
scope: narrow
risk: trivial
impact: component
level: implementation
---

## Description

Set up test infrastructure so that subsequent tasks can write tests as they implement. Add test helpers for creating in-memory transport streams (mock transport), and skeleton integration test files for each component.

The mock transport is critical — it lets us test SSH client/server flows without actual network I/O, per ADR-001's consequence that "mock transports can produce in-memory streams."

## Acceptance Criteria

- [ ] `crates/alknet-core/tests/` directory with empty integration test skeletons: `transport_tests.rs`, `client_tests.rs`, `server_tests.rs`, `auth_tests.rs`
- [ ] `crates/alknet-core/src/testutil.rs` module (behind `#[cfg(test)]` or a `testutil` feature) exporting `MockTransport` and `MockStream`
- [ ] `MockStream` wraps `tokio::io::DuplexStream` implementing `AsyncRead + AsyncWrite + Unpin + Send`
- [ ] `MockTransport` implements `Transport` trait (once defined) returning `MockStream` via `connect()`
- [ ] `MockTransportAcceptor` implements `TransportAcceptor` (once defined) returning paired `MockStream` via `accept()`
- [ ] `cargo test` succeeds (even if no real tests yet)

## References

- docs/architecture/transport.md — Transport trait contract
- docs/architecture/decisions/001-pluggable-transport.md — "mock transports can produce in-memory streams"

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
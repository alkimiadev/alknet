---
id: napi/connect-function
name: Implement NAPI connect() — single SSH channel as Duplex stream
status: completed
depends_on:
  - napi/project-setup
  - client/channel-manager
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Implement the NAPI `connect()` function per ADR-007. This is fundamentally different from CLI `alknet connect`:

- **NAPI `connect()`**: Opens a single SSH channel and returns it as a Node.js `Duplex` stream. No SOCKS5 server, no port forwarding. The caller reads and writes bytes directly.
- **CLI `alknet connect`**: Full SSH client session with SOCKS5 server and port forwarding.

The function accepts `AlknetConnectOptions` and returns `Promise<Duplex>`. The NAPI layer handles transport selection, SSH authentication, and channel setup, then hands the caller a stream.

## Acceptance Criteria

- [ ] `#[napi]` function `connect(options: AlknetConnectOptions) -> Result<DuplexStream>` in `crates/alknet-napi/src/connect.rs`
- [ ] `AlknetConnectOptions` struct with napi fields: `server`, `peer`, `transport`, `identity`, `tlsServerName`, `insecure`, `irohRelay`, `proxy`
- [ ] Transport creation from options (tcp, tls, iroh) — same logic as CLI but programmatic
- [ ] SSH client connection: create transport stream, authenticate, open single `direct_tcpip` channel
- [ ] Channel returned as `napi::DuplexStream` for JavaScript consumption
- [ ] Key material: `identity` field accepts file path (string) or `Buffer` (in-memory data) per ADR-011
- [ ] Error marshalling: Rust errors become JavaScript exceptions with descriptive messages
- [ ] TypeScript type: `(options: AlknetConnectOptions) => Promise<Duplex>`
- [ ] Integration test from JS: connect to a test server, write/receive bytes through stream

## References

- docs/architecture/napi-and-pubsub.md — NAPI connect() spec, TypeScript interfaces
- docs/architecture/decisions/007-napi-single-stream.md — single duplex stream rationale
- docs/architecture/decisions/016-napi-expose-connect-and-serve.md — both connect() and serve()

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
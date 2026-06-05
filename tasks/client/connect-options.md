---
id: client/connect-options
name: Implement ConnectOptions struct and client session orchestration with graceful shutdown
status: pending
depends_on:
  - client/channel-manager
  - client/socks5-server
  - client/port-forwarding
  - transport/trait-and-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement `ConnectOptions` — the programmatic configuration struct (ADR-011) for the client — and the top-level client session orchestrator that ties together transport, channel manager, SOCKS5 server, and port forwards.

The client session lifecycle:
1. Create transport based on `ConnectOptions`
2. Connect transport, authenticate SSH session
3. Start SOCKS5 server
4. Start port forward listeners
5. Run until SIGTERM/SIGINT or fatal error
6. Graceful shutdown

Graceful shutdown (SIGTERM/SIGINT):
1. Stop accepting new SOCKS5 connections and port forward connections
2. Send SSH disconnect message to server
3. Wait for in-flight data to drain (~2 second timeout)
4. Close transport stream
5. Exit

## Acceptance Criteria

- [ ] `crates/alknet-core/src/client/mod.rs` re-exports all client components
- [ ] `ConnectOptions` struct with fields matching client.md CLI interface: `server`, `peer`, `transport_mode`, `identity`, `socks5_addr`, `forwards`, `remote_forwards`, `proxy`, `iroh_relay`, `tls_server_name`, `insecure`
- [ ] `ConnectOptions::identity` accepts `KeySource` (file or in-memory)
- [ ] `ClientSession::new(opts: ConnectOptions) -> Result<Self>` — creates transport, connects, authenticates
- [ ] `ClientSession::run()` — starts SOCKS5 server, port forwards, waits for shutdown signal
- [ ] SOCKS5 is always enabled when running (per constraint)
- [ ] Port forwards are optional and started based on `ConnectOptions`
- [ ] `ClientSession::shutdown()` — graceful shutdown: stop accepting, send SSH disconnect, drain timeout, close
- [ ] SIGTERM/SIGINT handled via tokio signal
- [ ] Integration test: full client-to-server session via mock transport, SOCKS5 proxy works, shutdown completes

## References

- docs/architecture/client.md — full client spec, CLI interface, graceful shutdown
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — ConnectOptions programmatic struct

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
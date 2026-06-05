---
id: server/serve-loop
name: Implement server accept loop, graceful shutdown, and ServeOptions config
status: completed
depends_on:
  - server/handler
  - server/channel-proxy
  - server/rate-limiting-and-logging
  - transport/trait-and-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the server's main accept loop and configuration. This ties together the transport acceptor, server handler, rate limiting, and logging into a coherent server process.

`ServeOptions` is the programmatic configuration struct (ADR-011) for the server. The accept loop:
1. Binds a `TransportAcceptor` based on transport mode
2. Accepts incoming connections (respecting rate limits)
3. Creates a `ServerHandler` per connection
4. Passes the stream to `russh::server::run_stream()`
5. Handles graceful shutdown on SIGTERM/SIGINT

## Acceptance Criteria

- [x] `crates/alknet-core/src/server/mod.rs` re-exports all server components
- [x] `ServeOptions` struct with fields matching server.md CLI interface: `key`, `authorized_keys`, `cert_authority`, `transport_mode`, `listen_addr`, `tls_cert`, `tls_key`, `acme_domain`, `stealth`, `proxy`, `iroh_relay`, `max_connections_per_ip`, `max_auth_attempts`
- [x] `Server::new(opts: ServeOptions) -> Result<Server>` — creates server with bound acceptor, auth config, rate limiter
- [x] `Server::run()` — enters accept loop, for each connection: check rate limit → create handler → `run_stream()`
- [x] Stealth mode integration: if enabled, protocol detection before `run_stream()`
- [x] Graceful shutdown: `Server::shutdown()` method and signal handler (SIGTERM/SIGINT)
  - Stop accepting new connections
  - Send SSH disconnect to active sessions
  - Wait for drain timeout (~2 seconds per session)
  - Forcibly terminate remaining connections
- [x] iroh mode: prints endpoint ID on startup
- [x] `ServeOptions::key` and `ServeOptions::authorized_keys` accept `KeySource` (file or in-memory)
- [x] Integration test: start server, client connects via mock transport, session works, shutdown completes

## References

- docs/architecture/server.md — full server spec including graceful shutdown
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — ServeOptions programmatic struct

## Notes

Key design decisions:
- `Server::run(acceptor, endpoint_info)` takes a generic `TransportAcceptor` and optional endpoint info string, keeping transport binding separate from the accept loop
- `handle_disconnect` returns a future (`Handle::disconnect` is async in russh 0.49), takes `String` args
- `shutdown_rx` is cloned to avoid needing `&mut self` on `Arc<Server>` in the select loop
- `ServeTransportMode` is a separate enum from `TransportKind` to keep serve options independent of transport types
- Stealth mode only applies when both `stealth=true` AND `transport_mode=Tls`

## Summary

Implemented server accept loop and configuration in `crates/alknet-core/src/server/serve.rs`:
- `ServeOptions` struct with all CLI interface fields, builder pattern, KeySource support
- `Server::new()` creates server with russh config, auth config, rate limiter
- `Server::run(acceptor, endpoint_info)` enters accept loop with rate limiting, stealth detection, russh::server::run_stream()
- `Server::shutdown()` sends SSH disconnect to active sessions, waits drain timeout, aborts remaining
- SIGTERM/SIGINT handler on unix platforms
- iroh endpoint ID logged on startup
- All 216 tests pass, clippy clean
---
id: server/handler
name: Implement ServerHandler — russh server handler with auth and channel dispatch
status: pending
depends_on:
  - auth/server-auth-handler
  - transport/trait-and-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the core `ServerHandler` that implements `russh::server::Handler`. This is the heart of the server. Per server.md, it has two primary responsibilities:

1. **`auth_publickey()`**: Delegated to `ServerAuthConfig` — checks key against authorized set or validates cert-authority
2. **`channel_open_direct_tcpip()`**: Routes the channel — either to a TCP target (directly or via proxy) or internally for reserved `alknet-*` destinations (ADR-018)

At this stage, implement the handler struct, auth delegation, and the channel dispatch skeleton (actual TCP connection and proxy logic in dependent tasks).

## Acceptance Criteria

- [ ] `crates/alknet-core/src/server/handler.rs` exports `ServerHandler`
- [ ] `ServerHandler` implements `russh::server::Handler`
- [ ] `ServerHandler` holds: `Arc<ServerAuthConfig>`, `outbound_proxy: Option<ProxyConfig>`, `remote_addr: Option<SocketAddr>`
- [ ] `auth_publickey()` delegates to `ServerAuthConfig` and returns `Accept` or `Reject`
- [ ] `channel_open_direct_tcpip()` dispatches: if `host.starts_with("alknet-")`, route to internal handler (stub for control channel); otherwise, spawn TCP proxy task (stub that logs and returns error for now)
- [ ] One `ServerHandler` instance per connection; state is not shared between connections (unless explicitly Arc'd)
- [ ] Structured auth logging via `tracing::info!` with `remote_addr`, `key_fingerprint`, `result` (ADR-013)
- [ ] Unit tests: auth delegation works, reserved destination routing logic, unknown channel types rejected

## References

- docs/architecture/server.md — Server Handler Behavior section, channel handling
- docs/architecture/decisions/018-control-channel-for-pubsub.md — reserved `alknet-*` destinations
- docs/architecture/decisions/013-fail2ban-friendly-logging.md — structured auth logging

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
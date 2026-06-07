---
id: client/channel-manager
name: Implement ChannelManager — SSH session management, channel opens, reconnection
status: completed
depends_on:
  - auth/client-auth-handler
  - transport/trait-and-types
  - auth/error-types
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Implement the `ChannelManager` that owns the `Arc<client::Handle<ClientHandler>>` and provides the core client methods:

- `open_direct_tcpip(host, port)` — open a tunnel channel to a remote host
- `open_streamlocal(socket_path)` — open a tunnel to a Unix socket (stub for now)
- `request_tcpip_forward(addr, port)` — request remote listening
- `cancel_tcpip_forward(addr, port)` — cancel remote listening

Most importantly, the channel manager handles **reconnection** on transport failure:
1. Detect via `handle.is_closed()` or transport read error
2. Exponential backoff reconnect (1s, 2s, 4s, ... max 30s)
3. Re-establish transport connection (call `transport.connect()` again)
4. Re-authenticate SSH session
5. Notify SOCKS5 server and port forwards (in-flight connections fail, new connections work)

Reconnection is always enabled. The backoff caps at 30 seconds and continues indefinitely.

## Acceptance Criteria

- [x] `crates/alknet-core/src/client/channel_manager.rs` exports `ChannelManager`
- [x] `ChannelManager` holds: `Arc<Transport>`, `Arc<ClientAuthConfig>`, `Arc<client::Handle<ClientHandler>>` (behind RwLock for reconnection)
- [x] `ChannelManager::new()` establishes initial transport connection, authenticates, returns manager
- [x] `open_direct_tcpip(host, port)` — opens SSH channel to target
- [x] `request_tcpip_forward(addr, port)` — sends `tcpip_forward` request
- [x] `cancel_tcpip_forward(addr, port)` — sends `cancel_tcpip_forward` request
- [x] Reconnection detection: monitors `handle.is_closed()`, triggers reconnect on failure
- [x] Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s (cap), continues indefinitely
- [x] Full reconnect: new transport stream, new SSH session over it (ADR-004)
- [x] After reconnect: port forward listeners (`-L`, `-R`) re-registered with new session
- [x] In-flight connections on old session fail gracefully (channel errors, not session-wide)
- [x] Unit tests: channel open, reconnection trigger, backoff timing, forward re-registration

## References

- docs/architecture/client.md — Channel Manager section, Reconnection section
- docs/architecture/decisions/004-ssh-over-transport.md — full reconnect, not "SSH reconnects over same transport"

## Notes

- Converted `client.rs` (single file) to directory module: `client/mod.rs` + `client/channel_manager.rs`
- Used `russh::keys::PrivateKey` and `russh::keys::PublicKey` (not the nonexistent `russh::key::KeyPair`)
- Reconnection monitor runs as a spawned tokio task that polls `handle.is_closed()` every 1s
- On reconnect: creates new transport stream + new SSH session (ADR-004 full reconnect)
- `ForwardRequest` struct tracks registered port forwards for re-registration after reconnect
- In-flight channels on old session naturally fail with `ChannelError::ChannelClosed` since the handle is replaced

## Summary

Implemented `ChannelManager` in `crates/alknet-core/src/client/channel_manager.rs` with SSH session management, channel opens (`open_direct_tcpip`), port forward requests (`request_tcpip_forward`/`cancel_tcpip_forward`), and automatic reconnection with exponential backoff (1s→30s cap). Full reconnect per ADR-004 creates new transport stream + new SSH session. Port forwards are re-registered after successful reconnect. 8 unit tests covering backoff timing, forward tracking, transport failure, and reconnection detection.
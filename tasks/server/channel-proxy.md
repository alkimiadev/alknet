---
id: server/channel-proxy
name: Implement server channel proxy — direct TCP and outbound proxy connections
status: completed
depends_on:
  - server/handler
  - auth/error-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the server's channel proxy logic that makes outbound TCP connections on behalf of SSH clients. When `channel_open_direct_tcpip(host, port)` is called for a non-reserved destination:

1. Connect to `host:port`, either directly or via the configured outbound proxy
2. Run `tokio::io::copy_bidirectional` between the SSH channel stream and the outbound TCP stream
3. Clean up when either side disconnects

Supports three outbound proxy modes per server.md: Direct, SOCKS5 proxy, HTTP CONNECT proxy.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/server/channel_proxy.rs` exports channel proxy functions
- [ ] `ProxyConfig` enum: `Direct`, `Socks5 { addr: SocketAddr }`, `HttpConnect { addr: SocketAddr }`
- [ ] `connect_outbound(target: SocketAddr, proxy: &ProxyConfig) -> Result<TcpStream>` — connects to target directly or via proxy
- [ ] Direct mode: `TcpStream::connect(target)`
- [ ] SOCKS5 proxy: establishes SOCKS5 handshake, sends CONNECT command for target
- [ ] HTTP CONNECT proxy: sends `CONNECT host:port HTTP/1.1` to proxy, reads 200 response
- [ ] `proxy_channel(channel: ChannelStream, target: SocketAddr, proxy: &ProxyConfig)` — spawns bidirectional copy task
- [ ] Channel errors (target unreachable, proxy failure) close that channel without affecting SSH session
- [ ] No logging of tunnel destinations (ADR-006) — only transport/auth events are logged
- [ ] Unit tests: direct connection proxy, SOCKS5 proxy handshake, HTTP CONNECT proxy handshake, target unreachable handling

## References

- docs/architecture/server.md — Channel Handling, Outbound Proxy Modes sections
- docs/architecture/decisions/006-no-logging-of-tunnel-destinations.md — no destination logging
- docs/architecture/decisions/019-proxy-dual-semantics.md — server `--proxy` meaning

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
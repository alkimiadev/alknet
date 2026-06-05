---
id: client/socks5-server
name: Implement SOCKS5 server — local proxy that forwards through SSH channels
status: pending
depends_on:
  - auth/client-auth-handler
  - transport/trait-and-types
  - auth/error-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the local SOCKS5 proxy server — the primary client interface (ADR-005). Listens on a local port (default `127.0.0.1:1080`), accepts SOCKS5 connections, and for each connection:

1. Reads the SOCKS5 handshake (auth method negotiation, target address)
2. Opens `channel_open_direct_tcpip(target_host, target_port, originator_addr, originator_port)` on the SSH session
3. Converts the SSH channel to a stream via `channel.into_stream()`
4. Runs `tokio::io::copy_bidirectional(&mut local_socket, &mut ssh_stream)` to proxy data

Supports SOCKS5h (domain names resolved server-side) by default. This prevents DNS leaks — the client never resolves target hostnames locally (ADR-006).

## Acceptance Criteria

- [ ] `crates/alknet-core/src/socks5/mod.rs` exports `Socks5Server`
- [ ] `Socks5Server` binds to configurable listen address (default `127.0.0.1:1080`)
- [ ] SOCKS5 handshake: method negotiation (no-auth only), target address parsing (IPv4, IPv6, domain name)
- [ ] Domain name targets (SOCKS5h) sent unresolved to server — no local DNS resolution
- [ ] For each SOCKS5 connection, opens SSH `direct_tcpip` channel and proxies bytes bidirectionally
- [ ] Connection errors (SSH session down, channel open failed) result in SOCKS5 error response to client
- [ ] No logging of SOCKS5 request targets (ADR-006) — only connection-level events logged
- [ ] SOCKS5 server always enabled when `alknet connect` runs (per client.md constraint)
- [ ] Unit tests: SOCKS5 handshake parsing, address type handling, bidirectional proxy flow (with mock transport)

## References

- docs/architecture/client.md — SOCKS5 Server section
- docs/architecture/decisions/005-socks5-before-tun.md — SOCKS5 as primary interface
- docs/architecture/decisions/006-no-logging-of-tunnel-destinations.md — no DNS leak, no logging

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
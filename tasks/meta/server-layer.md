---
id: meta/server-layer
name: Complete server layer — handler, channel proxy, stealth, rate limiting, control channel, serve loop
status: completed
depends_on:
  - server/handler
  - server/channel-proxy
  - server/stealth-mode
  - server/rate-limiting-and-logging
  - server/control-channel
  - server/serve-loop
scope: system
risk: high
impact: phase
level: planning
---

## Description

Meta task that clusters all server module tasks. Once complete, the server accepts SSH connections via any transport, authenticates clients, proxies channel traffic to TCP targets (directly or via proxy), handles stealth mode, rate limits connections, routes reserved `alknet-` destinations, and shuts down gracefully.

## Acceptance Criteria

- [x] All server tasks completed
- [x] Server handles SSH connections over TCP, TLS, and iroh transports
- [x] Authentication via Ed25519 keys and cert-authority
- [x] Channel proxying with direct, SOCKS5, and HTTP CONNECT outbound modes
- [x] Stealth mode detects SSH vs HTTP and returns fake nginx 404
- [x] Rate limiting and structured logging
- [x] Control channel routing for `alknet-*` destinations
- [x] Graceful shutdown

## References

- docs/architecture/server.md

## Notes

All server module tasks completed across Gens 4-7. Server layer is fully implemented.

## Summary

Server layer complete: handler (auth + channel dispatch), channel proxy (direct/SOCKS5/HTTP CONNECT), stealth mode (protocol multiplexing), rate limiting (per-IP connection limits), control channel (alknet-* destination routing), serve loop (accept loop + graceful shutdown). All 229 tests pass.
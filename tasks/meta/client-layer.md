---
id: meta/client-layer
name: Complete client layer — SOCKS5, port forwarding, channel manager, ConnectOptions
status: completed
depends_on:
  - client/socks5-server
  - client/port-forwarding
  - client/channel-manager
  - client/connect-options
scope: system
risk: high
impact: phase
level: planning
---

## Description

Meta task that clusters all client module tasks. Once complete, the client establishes SSH sessions via any transport, runs a local SOCKS5 proxy, manages port forwards, handles reconnection with exponential backoff, and shuts down gracefully.

## Acceptance Criteria

- [ ] All client tasks completed
- [ ] SOCKS5 proxy works with DNS leak prevention (SOCKS5h)
- [ ] Local and remote port forwarding work
- [ ] Channel manager handles reconnection with exponential backoff (1s → 30s cap)
- [ ] Port forwards re-registered after reconnection
- [ ] ConnectOptions programmatic struct and CLI flags available
- [ ] Graceful shutdown on SIGTERM/SIGINT

## References

- docs/architecture/client.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
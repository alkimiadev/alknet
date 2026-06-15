---
status: planned
last_updated: 2026-06-15
---

# alknet-ssh

> **Status: Planned** — This spec has not been written yet. It will be produced as part of Phase 2 architecture work.

## Purpose

SSH handler implementing `ProtocolHandler` on ALPN `alknet/ssh`. Provides russh-based SSH-2 handshake, channel multiplexing, SOCKS5 proxy, and port forwarding (direct-tcpip, forwarded-tcpip, streamlocal-forward).

## Port Source

| Old module | Lines | Notes |
|---|---|---|
| `src/interface/ssh.rs` | 982 | SSH channel handling |
| `src/server/handler.rs` | 974 | SSH server handler |
| `src/server/channel_proxy.rs` | 555 | Channel proxy |
| `src/client/*` | ~1900 | SOCKS5 client, connect logic |
| `src/socks5/*` | ~800 | SOCKS5 protocol |

## References

- [overview.md](../../overview.md)
- ADR-002: ProtocolHandler trait
- ADR-004: Auth as shared core
- russh reference: `docs/research/references/ssh/russh/`
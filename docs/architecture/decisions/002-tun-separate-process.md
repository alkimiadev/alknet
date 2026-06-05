# ADR-002: TUN Shim as Separate Process

## Status
Superseded by ADR-014

## Context
TUN interface creation requires root privileges or `CAP_NET_ADMIN` on Linux, Administrator on Windows, or platform-specific VPN APIs on macOS/iOS/Android. If the core alknet binary required these privileges, the attack surface of root-required code would include the entire SSH implementation, key handling, and transport negotiation.

The primary use cases (SOCKS5 proxy, port forwarding) need no privileges at all. Only the "route all traffic through TUN" use case needs root.

## Decision
The TUN functionality is a separate `alknet-tun` binary that:
1. Creates a TUN device (requires root / CAP_NET_ADMIN)
2. Reads IP packets from it
3. Forwards each connection to the core alknet's SOCKS5 port (127.0.0.1:1080)
4. Proxies bytes between TUN packets and SOCKS5 connections

The core `alknet connect` binary never needs root. The `alknet-tun` binary is ~200-500 lines and does nothing except TUN ↔ SOCKS5 forwarding.

## Consequences
- **Positive**: Root-required code surface is tiny and auditable.
- **Positive**: Core binary runs unprivileged. SOCKS5 and port forwarding work without any special permissions.
- **Positive**: TUN process can crash without affecting the SSH session (it just reconnects to SOCKS5).
- **Positive**: Matches the proven tun2proxy architecture.
- **Negative**: Two processes to manage instead of one. Requires process supervision (systemd, etc.).
- **Negative**: SOCKS5 adds a small latency overhead vs. direct TUN → SSH packet routing. This is acceptable for the security benefit.

## References
- [tun-shim.md](../tun-shim.md)
- [tun2proxy](https://github.com/tun2proxy/tun2proxy) — proven architecture for TUN → SOCKS5 proxy
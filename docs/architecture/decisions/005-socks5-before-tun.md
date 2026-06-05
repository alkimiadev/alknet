# ADR-005: SOCKS5 as Primary Interface, TUN as Add-on

## Status
Accepted

## Context
A "VPN-like" tool needs to route traffic. There are three approaches:

1. **TUN only**: Create a TUN interface, route all OS traffic through it. Full VPN experience but requires root.
2. **SOCKS5 only**: Local SOCKS5 proxy. Applications configure proxy settings. No root needed but application support varies.
3. **SOCKS5 primary, TUN add-on**: SOCKS5 is the core interface. TUN forwards to SOCKS5.

## Decision
SOCKS5 is the primary interface. TUN is a separate process that forwards to SOCKS5 (Option 3).

SOCKS5 is the core because:
- It requires no privileges
- `curl --socks5-hostname` works everywhere
- Browsers, most CLI tools, and many applications support SOCKS5
- SOCKS5h prevents DNS leaks by resolving names server-side
- It's the interface that the NAPI wrapper and pubsub adapter build on
- TUN is only needed for "route all traffic" use cases, which are a subset of users

TUN forwards to SOCKS5 rather than directly to SSH because:
- The SOCKS5 code already handles TCP connection establishment and bidirectional proxying
- TUN's job is just IP packet → SOCKS5 connection, not IP packet → SSH channel
- The `alknet-tun` binary stays minimal (~200-500 lines)
- No root code in the core binary

## Consequences
- **Positive**: Core binary is root-free. TUN functionality is provided by the external `tun2proxy` tool (ADR-014).
- **Positive**: SOCKS5 is testable without TUN — just `curl` against it.
- **Positive**: The TUN approach is validated by tun2proxy, a well-tested existing tool. No custom TUN code to maintain.
- **Negative**: VPN-like behavior requires running `tun2proxy` alongside `alknet connect` — two processes instead of one integrated binary.
- **Negative**: SOCKS5 doesn't capture UDP (except DNS via SOCKS5h). TUN mode via tun2proxy handles this separately.

## References
- [client.md](../client.md)
- [tun-shim.md](../tun-shim.md)
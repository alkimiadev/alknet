# ADR-014: Defer TUN Implementation, Recommend Local SOCKS5 + tun2proxy

## Status
Accepted

## Context
The original plan included a TUN shim (`alknet-tun`) as Phase 3 — a separate root-requiring process that creates a TUN device and forwards IP packets through alknet's SOCKS5 port. This would provide VPN-like "route all traffic" behavior.

However, TUN implementation has significant complexities:
- Platform differences (Linux TUN, macOS utun, Windows wintun.dll)
- TCP reconstruction in userspace (smoltcp or tun2proxy's ip-stack)
- Virtual DNS handling
- Root/CAP_NET_ADMIN requirements
- TUN is easy to get wrong and hard to debug

The core SOCKS5 interface already works for the vast majority of use cases. For users who truly need VPN-like "route all traffic" behavior, `tun2proxy` is an existing, well-tested tool that does exactly this: creates a TUN device and routes traffic through a SOCKS5 proxy.

## Decision
Defer TUN implementation entirely. Remove `alknet-tun` from the architecture. Instead:

1. **Core interface**: alknet's local SOCKS5 proxy (always available, no root required)
2. **VPN-like behavior**: Users who need it run `tun2proxy --proxy socks5://127.0.0.1:1080` alongside `alknet connect`
3. **Documentation**: Recommend tun2proxy in the README/wiki for "route all traffic" use cases

This removes TUN from the project scope while still providing a path to VPN-like behavior. If demand justifies it later, `alknet-tun` can be added as a thin wrapper around tun2proxy's pattern.

The `tun` feature flag and `alknet-tun` binary are removed from the architecture. The `tun-rs` dependency is removed.

## Consequences
- **Positive**: Significantly reduces project scope and complexity. No TUN code to write, test, or maintain across platforms.
- **Positive**: tun2proxy is already well-tested for this exact use case.
- **Positive**: Core binary remains unprivileged. No root code anywhere in the project.
- **Positive**: Cleaner architecture — alknet only does SSH tunneling + SOCKS5. tun2proxy does TUN.
- **Negative**: Users need two tools instead of one for VPN-like behavior. Mitigated by documentation.
- **Negative**: tun2proxy is an external dependency in practice, though it's widely available in package managers.
- **Negative**: No first-class Windows/macOS TUN story. tun2proxy handles these platforms but users need to install it separately.

## References
- [tun-shim.md](../tun-shim.md) — this spec is now deprecated
- [ADR-002](002-tun-separate-process.md) — superseded; TUN is no longer in scope
- [ADR-005](005-socks5-before-tun.md) — SOCKS5 is still the primary interface; TUN forwarding is now external
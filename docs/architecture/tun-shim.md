---
status: deprecated
last_updated: 2026-06-01
---

# TUN Shim (Deprecated)

> **Note**: TUN functionality has been deferred from the alknet project. For VPN-like "route all traffic" behavior, use `tun2proxy` alongside alknet's SOCKS5 proxy. See ADR-014 for the rationale.

## What Changed

The `alknet-tun` separate process and all TUN-related code is out of scope. The recommended approach for VPN-like behavior is:

```bash
# Terminal 1: alknet SOCKS5 proxy (no root required)
alknet connect --server example.com --identity ~/.ssh/id_ed25519

# Terminal 2: tun2proxy routes all traffic through alknet's SOCKS5
sudo tun2proxy --proxy socks5://127.0.0.1:1080
```

This keeps the core alknet binary free of TUN complexity and leverages an existing, well-tested tool for TUN-to-SOCKS5 bridging.

## References

- [ADR-014](decisions/014-defer-tun-recommend-socks5-proxy.md) — decision to defer TUN
- [ADR-005](decisions/005-socks5-before-tun.md) — SOCKS5 is still the primary interface
- [tun2proxy](https://github.com/tun2proxy/tun2proxy) — recommended external tool for TUN support
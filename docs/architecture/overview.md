---
status: reviewed
last_updated: 2026-06-02
---

# Alknet Overview

## Purpose

Alknet is a self-hostable SSH-based tunnel tool that provides VPN-like functionality without being a VPN protocol. It enables:

- **Private tunneling** of services (Postgres, Redis, internal APIs) over SSH
- **Censorship circumvention** — SSH over TLS on port 443 looks like HTTPS to DPI
- **NAT traversal** — iroh transport allows peer-to-peer connections without public IPs or port forwarding
- **Service mesh connectivity** — a lightweight transport layer for the pubsub/operations event system

The core insight: SSH tunnels work because SSH is fundamental infrastructure. Blocking it breaks the internet. Alknet makes SSH tunneling accessible through a simple CLI with pluggable transports.

## Exports

### Binary: `alknet`

A single binary with subcommands:

```
alknet serve     — Start the server (accepts SSH connections)
alknet connect  — Start the client (opens SSH session, exposes SOCKS5/port-forwards)
```

### Library: `alknet-core`

The `alknet-core` crate exports the pluggable components for embedding or programmatic use:

- `Transport` trait — produces a duplex stream for SSH to run over
- `TcpTransport` — direct TCP connection
- `TlsTransport` — TCP + tokio-rustls TLS
- `IrohTransport` — iroh QUIC P2P connection
- `Socks5Server` — local SOCKS5 proxy that forwards through SSH channels
- `PortForwarder` — manages local/remote port forwards
- `ServerHandler` — russh server handler with configurable auth and channel policies
- `ConnectOptions` / `ServeOptions` — programmatic configuration structs (no file parsing)

## Dependencies

| Dependency | Purpose | Feature-gated |
|------------|---------|---------------|
| `russh` | SSH client & server | No (core) |
| `tokio` | Async runtime | No (core) |
| `tokio-rustls` | TLS wrapping | Yes (`tls`) |
| `rustls` | TLS implementation | Yes (`tls`) |
| `rustls-acme` | ACME/Let's Encrypt auto-cert | Yes (`acme`) |
| `iroh` | P2P QUIC transport | Yes (`iroh`) |
| `clap` | CLI argument parsing | No (core) |
| `tracing` | Structured logging | No (core) |
| `anyhow` / `thiserror` | Error handling | No (core) |

> Note: `tun-rs` is no longer a dependency. TUN support is deferred in favor of the external `tun2proxy` tool (ADR-014).

## Architecture Constraints

1. **SSH runs over transport, not alongside** — The transport layer produces a single `AsyncRead+AsyncWrite+Unpin+Send` stream. SSH runs over that stream via `russh::client::connect_stream()` / `russh::server::run_stream()`. The SSH layer never knows what transport it's on. (ADR-001, ADR-004)

2. **SOCKS5 is the primary client interface** — Port forwarding is built on top of SOCKS5-like channel management. For VPN-like "route all traffic" behavior, users run `tun2proxy` alongside alknet's SOCKS5 proxy. TUN is not in the project scope. (ADR-005, ADR-014)

3. **No logging of tunnel destinations** — The server logs auth attempts and connections (for fail2ban) but does not log `channel_open_direct_tcpip` destinations, DNS lookups, or bytes transferred. (ADR-006, ADR-013)

4. **Programmatic-first API** — Configuration via CLI flags, library API structs (`ConnectOptions`, `ServeOptions`), and environment variables. No `~/.ssh/config` parsing, no custom config files. (ADR-011)

5. **Feature flags control transport inclusion** — `tls`, `iroh`, `acme` are feature-gated so the base install is lean. Users opt in to heavier dependencies.

6. **Authentication is key-based** — Ed25519 public key (default) and OpenSSH certificate authority. No password authentication over SSH. (ADR-012)

7. **NAPI exposes both connect() and serve()** — The napi-rs wrapper provides client and server functionality, using napi-rs as the FFI bridge. The NAPI layer is transport-agnostic and not tied to pubsub. (ADR-015, ADR-016)

8. **Error handling follows a consistent layered pattern** — Transport and auth errors cause reconnection (client, with exponential backoff) or connection rejection (server). Channel-level errors (target unreachable, proxy failure) close the individual channel without killing the session. Library API errors propagate via `anyhow::Result` / `thiserror` types. CLI reports errors to stderr with appropriate exit codes. NAPI errors are marshalled as JavaScript exceptions.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-pluggable-transport.md) | Pluggable transport | Transport trait produces `AsyncRead+AsyncWrite+Unpin+Send`, SSH consumes it |
| [002](decisions/002-tun-separate-process.md) | TUN shim separate | Superseded — TUN is deferred, use tun2proxy (ADR-014) |
| [003](decisions/003-iroh-stream-join.md) | iroh stream join | `tokio::io::join(recv, send)` combines QUIC halves |
| [004](decisions/004-ssh-over-transport.md) | SSH over transport | SSH never accesses TCP/iroh/TLS directly |
| [005](decisions/005-socks5-before-tun.md) | SOCKS5 first | SOCKS5 is the primary interface; TUN is external (tun2proxy) |
| [006](decisions/006-no-logging-of-tunnel-destinations.md) | No logging of tunnel destinations | Server logs auth and connections, not destinations |
| [007](decisions/007-napi-single-stream.md) | NAPI single stream | NAPI exposes duplex streams, not SSH multiplexing |
| [008](decisions/008-acme-lets-encrypt.md) | ACME/Let's Encrypt | Auto-provision TLS certs, domain and IP paths |
| [009](decisions/009-default-iroh-relay.md) | Default iroh relay | n0 relay by default, `--iroh-relay` override |
| [010](decisions/010-transport-chaining-cli.md) | Transport chaining | `--proxy` works with all transports natively |
| [011](decisions/011-no-ssh-config-programmatic-api.md) | Programmatic-first | No file-based config; options are structs, env vars, CLI flags |
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Key + cert-authority | Ed25519 keys + OpenSSH CA; no password auth |
| [013](decisions/013-fail2ban-friendly-logging.md) | Fail2ban-friendly | Structured auth logs + built-in rate limiting |
| [014](decisions/014-defer-tun-recommend-socks5-proxy.md) | Defer TUN | Use tun2proxy for VPN-like behavior; no alknet-tun binary |
| [015](decisions/015-napi-rs-for-ffi-bridge.md) | napi-rs | Standard Node.js native addon tooling |
| [016](decisions/016-napi-expose-connect-and-serve.md) | connect + serve | NAPI exposes both client and server from the start |
| [017](decisions/017-stealth-mode-protocol-multiplexing.md) | Stealth mode | Protocol multiplexing on port 443 |
| [018](decisions/018-control-channel-for-pubsub.md) | Control channel | Reserved `alknet-control` destination for pubsub |
| [019](decisions/019-proxy-dual-semantics.md) | Proxy dual semantics | `--proxy` routes transport on client, data on server |

## Open Questions

All open questions have been resolved. See [open-questions.md](open-questions.md) for resolution details.

## References

- [Feasibility Assessment](../../../conversations/research/ssh-tunnel-vpn-alternative-feasibility.md)
- [russh API](/workspace/russh) — SSH client/server library
- [Dispatch](/workspace/@alkdev/dispatch) — Reference implementation of russh port forwarding
- [iroh](/workspace/iroh) — P2P QUIC connections
- [tun2proxy](https://github.com/tun2proxy/tun2proxy) — Recommended external TUN-to-SOCKS5 tool
- [Production certbot setup](/workspace/system/dev1/certbot.md) — Let's Encrypt on our infrastructure
- [Production fail2ban setup](/workspace/system/dev1/fail2ban.md) — fail2ban with nftables on our infrastructure
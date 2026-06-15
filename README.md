# Alknet

> **Status: Pre-alpha** — This project is undergoing a major architectural pivot to an ALPN-as-service model. The previous implementation has been archived and a greenfield rebuild is in progress.

A self-hostable networking toolkit built on QUIC+TLS with ALPN-based protocol dispatch. Each protocol handler (SSH, SFTP, Git, HTTP, DNS, messaging, call protocol) registers an ALPN string on a shared endpoint. The ALPN negotiation during the TLS/QUIC handshake routes connections to the correct handler before any application bytes are read.

## Core Insight

**A service IS an ALPN.** One endpoint, one port, many protocols — dispatched by the TLS handshake, not by application-level peeking or separate listeners.

## Crates

| Crate | Status | Description |
|-------|--------|-------------|
| `alknet-secret` | stable | BIP39/SLIP-0010/AES-GCM key derivation and encryption |
| `alknet-core` | planned | ProtocolHandler trait, ALPN router, auth/identity, config |
| `alknet-ssh` | planned | SSH handler (russh), SOCKS5, port forwarding |
| `alknet-call` | planned | JSON-RPC call protocol (EventEnvelope framing) |
| `alknet-fs` | planned | Content-addressed file storage (iroh-blobs backend) |
| `alknet-sftp` | planned | SFTP handler (russh-sftp protocol core) |
| `alknet-git` | planned | Git smart protocol handler (gix) |
| `alknet-http` | planned | HTTP handler (axum, REST API, MCP) |
| `alknet-dns` | planned | DNS handler (hickory-proto, pkarr) |
| `alknet-msg` | planned | E2E encrypted messaging, mixnet support |
| `alknet` | planned | CLI binary (assembles and registers handlers) |

## Documentation

- [ALPN-as-service architecture](docs/research/pivot/alpn-service-architecture.md) — pivot proposal
- [Cleanup plan](docs/research/pivot/cleanup-plan.md) — greenfield transition plan
- [SDD process](docs/sdd_process.md) — spec-driven development process
- [Research references](docs/research/references/) — iroh, russh, russh-sftp deep dives

Reference implementation (previous architecture) is preserved at `/workspace/@alkdev/alknet-main/`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
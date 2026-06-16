# ADR-003: Crate Decomposition

## Status

Accepted

## Context

The previous alknet-core crate was a monolith containing transport, interface, server, client, call, auth, config, socks5, credentials, and HTTP — all in one crate with interdependent modules. This created coupling (interface types depended on auth, server depended on call, everything depended on config) and made it impossible to use individual components independently.

The new ALPN dispatch model eliminates the need for a shared interface layer. Each handler is self-contained — it receives a byte stream and manages its own protocol. This naturally decomposes into separate crates.

Key constraints:
- Protocol crates must depend on alknet-core for auth/identity/config — but not on each other
- alknet-secret is already standalone (no alknet-core dependency) and must remain so (renamed to alknet-vault — see ADR-008)
- The CLI binary assembles everything — it's the only crate that depends on all handler crates
- Some handlers (SFTP, call protocol) need to compile to WASM for browser/client use
- irpc is the foundation for the call protocol — it provides the operation registry, framing, and pub/sub patterns

## Decision

The workspace decomposes into the following crates:

| Crate | Responsibility | Depends on |
|-------|---------------|------------|
| `alknet-core` | ProtocolHandler trait, ALPN router, endpoint, BiStream, AuthContext, IdentityProvider, config, ArcSwap dynamic config | tokio, quinn, rustls, irpc |
| `alknet-vault` | Local key vault: BIP39/SLIP-0010/AES-GCM key derivation, encryption, VaultProtocol dispatch | (standalone, no alknet-core) |
| `alknet-ssh` | SshAdapter (russh, SOCKS5, port forwarding) | alknet-core, russh |
| `alknet-call` | CallAdapter (JSON-RPC via irpc, operation registry, pub/sub, access control) | alknet-core, irpc |
| `alknet-git` | GitAdapter (gix, pkt-line protocol) | alknet-core, gix |
| `alknet-sftp` | SftpAdapter (russh-sftp protocol core) | alknet-core, russh-sftp |
| `alknet-msg` | MessageAdapter (E2E encryption, mixnet) | alknet-core |
| `alknet-http` | HttpAdapter (axum, REST API, MCP endpoint) | alknet-core, axum |
| `alknet-dns` | DnsAdapter (hickory-proto, pkarr, service discovery) | alknet-core, hickory-proto |
| `alknet-napi` | Node.js native addon (call protocol client) | alknet-call, napi-rs |
| `alknet` | CLI binary — registers handlers, starts endpoint | all handler crates |

Dependency flow:
```
alknet-vault (standalone)
alknet-core ← all handler crates ← alknet (CLI)
alknet-call ← alknet-napi
```

No handler crate depends on another handler crate. Cross-handler communication goes through the call protocol (alknet-call) or through alknet-core's endpoint.

## Consequences

**Positive:**
- Each handler can be developed, tested, and versioned independently
- WASM-compatible handlers (sftp, call) don't pull in heavy dependencies (russh, axum)
- alknet-vault remains standalone — no circular dependency risk
- New handlers are added by creating a crate and registering it with the endpoint
- Clean separation of concerns — each crate has one job

**Negative:**
- More crates to manage in the workspace — workspace Cargo.toml and version coordination
- Shared types (AuthContext, BiStream) must live in alknet-core — if they change, all handlers recompile
- The CLI binary has a large dependency tree (all handlers) — but this is expected for a binary that assembles everything
- Testing cross-handler behavior requires integration tests in the CLI or a test utility crate

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- ADR-004: Auth as shared core (IdentityProvider)
- ADR-005: irpc as call protocol foundation
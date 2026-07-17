---
id: client/crate-init
name: Initialize alknet-client crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: [tls/review-tls, endpoint/review-endpoint]
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Phase 3, Task 1 of the crate extraction (per `docs/research/alknet-crate-extraction/findings.md`).
Initialize the `alknet-client` crate from scratch. This crate provides the native client dial
seam — `AlknetClient`, the client-side analogue of `AlknetEndpoint`. A multi-transport dialer
that takes pre-built transport handles (quinn, TCP+TLS, iroh), dials a remote `AlknetEndpoint`
on a chosen ALPN, and produces a `Connection` for the protocol take-overs to consume.

The dial is the client-side mirror of the server-side accept loop (ADR-083): one type that
takes pre-built transport handles and produces a `Connection`, with the transport choice as a
parameter. The protocol take-overs (`CallClient::spawn_dispatch`, `ChannelClient::from_connection`)
are unchanged — they consume the `Connection` and do not know `AlknetClient` produced it.

### Crate setup

Create `crates/alknet-client/` with:

- `Cargo.toml` — package metadata, dependencies, feature flags
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/error.rs` — `ClientDialError` enum (5 variants: `TlsConfig`, `Connect`, `Handshake`, `NoTransport`, `Proxy`)
  - `src/client.rs` — `AlknetClient` struct, `new`, builder methods (`with_quinn`, `with_tcp_tls`, `with_iroh`, `with_socks5_proxy`)
  - `src/dial/quinn.rs` — `dial_quic` implementation (feature-gated on `quinn`)
  - `src/dial/tcp_tls.rs` — `dial_tcp_tls` implementation (feature-gated on `tcp`)
  - `src/dial/iroh.rs` — `dial_iroh` implementation (feature-gated on `iroh`)
  - `src/dial/mod.rs` — dial module declarations
  - `src/socks5.rs` — `Socks5ProxyConfig`, `Socks5Credentials`, `Socks5UdpSocket`, HTTP-to-SOCKS5 bridge (feature-gated on `socks5`)

### Dependencies

Per the findings (Phase 3) and the architecture spec:

| Crate | Purpose |
|-------|---------|
| `alknet-core` | `Connection`, `ConnectionCredentials`, `RemoteIdentity`, `Ed25519SecretKey`, types (workspace path) |
| `alknet-tls` | `TlsClientConfig` — for quinn + tcp dials (workspace path) |
| `quinn` 0.11 | QUIC transport (optional, feature-gated) |
| `tokio-rustls` 0.26 | TCP+TLS transport (optional, feature-gated) |
| `iroh` 1.0 | Iroh transport (optional, feature-gated) |
| `fast-socks5` 1 | SOCKS5 client (optional, feature-gated — ADR-090) |
| `tokio` 1 (full) | Async runtime, TcpStream, spawn |
| `thiserror` 2 | `ClientDialError` |

`alknet-client` depends on `alknet-tls` (for `TlsClientConfig`) and `alknet-core` (for
`Connection`, `ConnectionCredentials`, `RemoteIdentity`, and types). It does **not** depend
on `alknet-call` or `alknet-channels-call` — the dial is below the protocol.

### Feature flags

```toml
[features]
default = []
quinn = ["dep:quinn", "alknet-tls/quinn", "alknet-core/quinn"]      # dial_quic
tcp = ["dep:tokio-rustls", "alknet-tls/tcp"]                        # dial_tcp_tls
iroh = ["dep:iroh", "alknet-core/iroh"]                              # dial_iroh
socks5 = ["dep:fast-socks5"]                                         # proxied dial paths (ADR-090)
```

The `quinn` and `tcp` features pull the corresponding features on `alknet-tls` (for
`TlsClientConfig::for_quinn` / `for_tcp_tls`). The `quinn` and `iroh` features also pull
the corresponding features on `alknet-core` — `dial_quic` produces a `Connection` via
`Connection::from_quinn_with_alpn` and `dial_iroh` via `Connection::from_iroh`, both of
which live in `alknet-core`'s `types.rs` behind core's `quinn` / `iroh` features.

The `socks5` feature (ADR-090) is independent of the transport features — it enables the
proxy code path that `dial_quic` (UDP ASSOCIATE) and `dial_tcp_tls` (CONNECT) use when a
proxy is configured. The `fast-socks5` dep is behind `socks5`, so deployments that don't
use a proxy don't pay the dep.

### Workspace Cargo.toml

Add `crates/alknet-client` to the workspace `members` list in the root `Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-client: Native client dial seam — multi-transport dialer that
//! produces `Connection`s for protocol take-overs.
//!
//! `AlknetClient` is the client-side analogue of `AlknetEndpoint`: a
//! multi-transport dialer that takes pre-built transport handles (quinn,
//! TCP+TLS, iroh), dials a remote `AlknetEndpoint` on a chosen ALPN, and
//! produces a `Connection`. The protocol take-overs
//! (`CallClient::spawn_dispatch`, `ChannelClient::from_connection`)
//! consume the `Connection` — the dial is below the protocol.
//!
//! An optional SOCKS5 proxy (ADR-090) routes the dials through a proxy
//! to hide the client's real IP from the hub.

pub mod client;
pub mod dial;
pub mod error;
#[cfg(feature = "socks5")]
pub mod socks5;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-client/Cargo.toml` exists with all dependencies and feature flags
- [ ] `crates/alknet-client/src/lib.rs` exists with module declarations
- [ ] Module skeleton files exist: `error.rs`, `client.rs`, `dial/mod.rs`, `dial/quinn.rs`, `dial/tcp_tls.rs`, `dial/iroh.rs`, `socks5.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-client`
- [ ] `cargo check -p alknet-client` succeeds
- [ ] `cargo clippy -p alknet-client` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-core` dependency uses workspace path (`path = "../alknet-core"`)
- [ ] `alknet-tls` dependency uses workspace path (`path = "../alknet-tls"`)
- [ ] No dependency on `alknet-call` (dial is below the protocol)
- [ ] Feature flags: `quinn`, `tcp`, `iroh`, `socks5` (all optional, default off)
- [ ] `quinn` feature pulls `alknet-tls/quinn` + `alknet-core/quinn`
- [ ] `tcp` feature pulls `alknet-tls/tcp`
- [ ] `iroh` feature pulls `alknet-core/iroh`
- [ ] `socks5` feature pulls `dep:fast-socks5` only (no alknet feature deps)
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 3
- docs/architecture/crates/client/README.md — full architecture spec
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- crates/alknet-tls/Cargo.toml — reference for dep versions and feature flag pattern
- crates/alknet-endpoint/Cargo.toml — reference for builder-method pattern
- crates/alknet-call/src/client/call_client.rs — reference `connect()` implementation (lines 142-168)

## Notes

> This is the foundational setup task for alknet-client. All subsequent client/*
> tasks depend on this one. The crate depends on `alknet-tls` (for `TlsClientConfig`)
> and `alknet-core` (for `Connection`, `ConnectionCredentials`, `RemoteIdentity`).
> It does NOT depend on `alknet-call` — the dial is below the protocol. The old
> `CallClient::connect` in `alknet-call` is intentionally still present (duplicated)
> — the prune happens in Phase 5. The `socks5` feature and `fast-socks5` dep are
> opt-in; deployments that don't use a proxy pay nothing.

## Summary

> To be filled on completion

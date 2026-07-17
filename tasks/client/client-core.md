---
id: client/client-core
name: Implement AlknetClient struct, new, and builder methods (with_quinn, with_tcp_tls, with_iroh, with_socks5_proxy)
status: completed
depends_on: [client/error-type]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 3, Task 3. Implement the `AlknetClient` struct and its builder methods in
`crates/alknet-client/src/client.rs`. This is the central type — the client-side
analogue of `AlknetEndpoint`. Holds pre-built transport handles, all optional —
the client dials with whichever transport the remote endpoint type implies.

This is a **fresh build against the ADR-089/090/091 shape**, not a copy of the old
`CallClient::connect`. The old `connect()` built transports internally from
`CallCredentials`; the new `AlknetClient` receives pre-built transports via builder
methods, mirroring `AlknetEndpoint`'s builder pattern (ADR-083).

### Target shape (per architecture spec)

```rust
use std::sync::Arc;

#[cfg(feature = "quinn")]
use quinn;
#[cfg(feature = "tcp")]
use tokio_rustls;
#[cfg(feature = "iroh")]
use iroh;

#[cfg(feature = "socks5")]
use crate::socks5::Socks5ProxyConfig;

/// Native client dial seam — multi-transport dialer that produces
/// `Connection`s for protocol take-overs.
///
/// Holds pre-built transport handles, all optional — the client dials
/// with whichever transport the remote endpoint type implies. The
/// builder mirrors `AlknetEndpoint`'s `with_quinn` / `with_iroh` /
/// `with_tcp_tls` (ADR-083) — the assembly layer builds the transport
/// handles and hands them to the client via builder methods.
pub struct AlknetClient {
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_connector: Option<tokio_rustls::TlsConnector>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
    /// When set, `dial_quic` and `dial_tcp_tls` route through this
    /// SOCKS5 proxy (UDP ASSOCIATE / CONNECT respectively). `dial_iroh`
    /// forces relay-only via an HTTP-to-SOCKS5 bridge — see ADR-090 §5.
    /// Feature-gated on `socks5`.
    #[cfg(feature = "socks5")]
    socks5: Option<Socks5ProxyConfig>,
}

impl AlknetClient {
    /// Create a new `AlknetClient` with no transport handles configured.
    /// Use the builder methods to add transports.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "quinn")]
            quinn: None,
            #[cfg(feature = "tcp")]
            tcp_connector: None,
            #[cfg(feature = "iroh")]
            iroh: None,
            #[cfg(feature = "socks5")]
            socks5: None,
        }
    }

    /// Set the QUIC transport handle. The assembly layer builds a
    /// `quinn::Endpoint` (with or without a SOCKS5 proxy — the proxy
    /// is applied inside `dial_quic`, not at construction time) and
    /// hands it to the client.
    #[cfg(feature = "quinn")]
    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self {
        self.quinn = Some(endpoint);
        self
    }

    /// Set the TCP+TLS transport handle. The assembly layer builds a
    /// `tokio_rustls::TlsConnector` and hands it to the client.
    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(mut self, connector: tokio_rustls::TlsConnector) -> Self {
        self.tcp_connector = Some(connector);
        self
    }

    /// Set the iroh transport handle. The assembly layer builds an
    /// `iroh::Endpoint` and hands it to the client.
    #[cfg(feature = "iroh")]
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self {
        self.iroh = Some(endpoint);
        self
    }

    /// Set the SOCKS5 proxy for all subsequent dials. When set, every
    /// dial routes its transport through this proxy: UDP ASSOCIATE for
    /// `dial_quic`, CONNECT for `dial_tcp_tls`, and force-relay-only +
    /// HTTP-to-SOCKS5 bridge for `dial_iroh` (ADR-090 §5).
    /// Feature-gated on `socks5`.
    #[cfg(feature = "socks5")]
    pub fn with_socks5_proxy(mut self, proxy: Socks5ProxyConfig) -> Self {
        self.socks5 = Some(proxy);
        self
    }
}

impl Default for AlknetClient {
    fn default() -> Self {
        Self::new()
    }
}
```

### Key design decisions

1. **Builder pattern mirrors `AlknetEndpoint`**: `with_quinn`, `with_iroh`, `with_tcp_tls`
   are the same builder method names as the server side (ADR-083). The assembly layer
   builds the transport handles and hands them to the client.

2. **`new()` takes no parameters**: No `StaticConfig`, no `TlsClientConfig`, no
   credentials. The client receives pre-built transports. This is the same pattern
   as `AlknetEndpoint::new()`.

3. **`with_tcp_tls` takes a `TlsConnector`**: The assembly layer builds the
   `TlsConnector` from `TlsClientConfig::for_tcp_tls()` (or directly from a
   `rustls::ClientConfig`). The client does not build TLS configs — it receives
   pre-built connectors.

4. **`with_socks5_proxy` is a client-level setting**: The proxy is set once on the
   client (all dials use it), not per-dial. The proxy is a client-level privacy
   posture, not a per-connection choice. Feature-gated on `socks5`.

5. **No `connect()` method**: The old `CallClient::connect()` welded the dial into
   the protocol crate. `AlknetClient` has three separate dial methods
   (`dial_quic`, `dial_tcp_tls`, `dial_iroh`) — each is a separate task.

6. **No protocol take-over**: The client produces a `Connection`; the caller hands
   it to `CallClient::spawn_dispatch` or `ChannelClient::from_connection`.
   `AlknetClient` does not spawn the dispatch loop.

### What this does NOT include

- The dial methods (`dial_quic`, `dial_tcp_tls`, `dial_iroh`) — separate tasks
- The SOCKS5 proxy implementation (`Socks5UdpSocket`, HTTP-to-SOCKS5 bridge) — separate task
- `Socks5ProxyConfig` / `Socks5Credentials` types — defined in `socks5.rs` (separate task)
- Tests — separate task

## Acceptance Criteria

- [ ] `AlknetClient` struct defined in `crates/alknet-client/src/client.rs`
- [ ] Fields: `quinn` (feature-gated), `tcp_connector` (feature-gated), `iroh` (feature-gated), `socks5` (feature-gated)
- [ ] `AlknetClient::new()` takes no parameters, initializes all fields to `None`
- [ ] `with_quinn(endpoint)` builder method (feature-gated on `quinn`)
- [ ] `with_tcp_tls(connector)` builder method (feature-gated on `tcp`)
- [ ] `with_iroh(endpoint)` builder method (feature-gated on `iroh`)
- [ ] `with_socks5_proxy(proxy)` builder method (feature-gated on `socks5`)
- [ ] `Default` impl delegates to `new()`
- [ ] `Debug` impl (manual or derived — lists which transports are configured, no transport internals)
- [ ] No `connect()` method (the old welded dial is not replicated)
- [ ] No `StaticConfig` parameter on `new()` (assembly layer reads it)
- [ ] No `TlsClientConfig` construction (client receives pre-built connectors)
- [ ] No dependency on `alknet-call` (dial is below the protocol)
- [ ] Feature gates correct: `quinn`, `tcp`, `iroh`, `socks5` each gate their respective fields/methods
- [ ] `cargo check -p alknet-client` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-client` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/client/README.md — `AlknetClient` section (lines 100-153)
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090 §1-2
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- crates/alknet-endpoint/src/endpoint.rs — `AlknetEndpoint` builder pattern (reference)
- crates/alknet-call/src/client/call_client.rs — old `CallClient` struct (lines 102-187, reference for what NOT to replicate)

## Notes

> This is the core structural task of Phase 3. The `AlknetClient` is built fresh
> against the ADR-089/090/091 shape — it's not a copy of the old `CallClient`.
> The key difference: the old `connect()` built transports internally from
> `CallCredentials`; the new client receives pre-built transports via builder
> methods. The dial methods are separate tasks. The old code in `call_client.rs`
> is NOT deleted — that's Phase 5.

## Summary

> To be filled on completion

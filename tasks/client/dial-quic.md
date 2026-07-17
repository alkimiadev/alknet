---
id: client/dial-quic
name: Implement dial_quic — QUIC dial via quinn, producing a Connection
status: pending
depends_on: [client/client-core]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 3, Task 4. Implement `AlknetClient::dial_quic` in `crates/alknet-client/src/dial/quinn.rs`.
The QUIC dial: builds a `TlsClientConfig` from `ConnectionCredentials`, constructs a
`quinn::ClientConfig`, dials `addr` on `alpn`, and returns a `Connection` via
`Connection::from_quinn_with_alpn`.

This is a **fresh build** against the ADR-089/091 shape, not a copy of the old
`CallClient::connect`. The old `connect()` hardcoded `alknet/call` ALPN and returned a
`CallConnection` (welding the dial to the call protocol). The new `dial_quic` takes the
ALPN as a parameter and returns a `Connection` — the protocol take-over is the caller's
concern.

### Target shape (per architecture spec)

```rust
impl AlknetClient {
    /// QUIC dial. Builds a `TlsClientConfig` from `creds`
    /// (ADR-034 verifier selection + ADR-084 provider), dials `addr`
    /// on `alpn`, returns a `Connection` via
    /// `Connection::from_quinn_with_alpn`. The `server_name` is the
    /// TLS SNI / name (for X.509; ignored for raw-key pinning).
    /// Feature-gated on `quinn`.
    #[cfg(feature = "quinn")]
    pub async fn dial_quic(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;
}
```

### Implementation outline

```rust
#[cfg(feature = "quinn")]
pub async fn dial_quic(
    &self,
    addr: SocketAddr,
    server_name: &str,
    alpn: &[u8],
    creds: &ConnectionCredentials,
) -> Result<Connection, ClientDialError> {
    // 1. Build TlsClientConfig from credentials + ALPN
    let tls_config = TlsClientConfig::new(creds, alpn)?;  // TlsError → ClientDialError::TlsConfig via #[from]

    // 2. Convert to quinn::ClientConfig
    let client_config = tls_config.for_quinn()?;

    // 3. Build or use the quinn endpoint
    let endpoint = match &self.quinn {
        Some(ep) => ep.clone(),
        None => return Err(ClientDialError::NoTransport { transport: "quinn" }),
    };

    // 4. Connect
    let conn = endpoint
        .connect_with(client_config, addr, server_name)
        .map_err(|e| ClientDialError::Connect(e.to_string()))?
        .await
        .map_err(|e| ClientDialError::Connect(e.to_string()))?;

    // 5. Wrap as Connection
    Ok(Connection::from_quinn_with_alpn(conn, alpn.to_vec()))
}
```

### Key design decisions

1. **`TlsClientConfig::new(creds, alpn)`**: The TLS config is built from
   `ConnectionCredentials` (ADR-091) — the unified transport-level credential bundle.
   The `TlsError` from config construction is converted to `ClientDialError::TlsConfig`
   via the `#[from]` impl.

2. **`server_name` is the TLS SNI**: For X.509 endpoints, this is the hostname the
   server's cert was issued for. For raw-key endpoints, it's ignored by the verifier
   (fingerprint pin doesn't use SNI). The parameter is always present for caller
   simplicity — the caller doesn't need to know which verifier path is active.

3. **`alpn` is a byte slice**: The ALPN protocol identifier (e.g., `b"alknet/call"`,
   `b"alknet/channels"`). The dial is ALPN-agnostic — it dials any ALPN the remote
   endpoint advertises.

4. **Returns `Connection`, not `CallConnection`**: The old `connect()` returned a
   `CallConnection` (welding the dial to the call protocol). The new `dial_quic`
   returns a `Connection` — the caller hands it to `CallClient::spawn_dispatch` or
   `ChannelClient::from_connection`.

5. **`NoTransport` error when `with_quinn` not set**: The dial checks that a quinn
   endpoint was configured. If not, it returns `ClientDialError::NoTransport`.

6. **SOCKS5 proxy path**: When `self.socks5` is `Some`, the dial routes through the
   proxy (UDP ASSOCIATE) instead of using the pre-built quinn endpoint directly.
   This is implemented in the `client/socks5-proxy` task — this task implements the
   direct (no-proxy) path. The proxy integration point is a conditional branch:
   ```rust
   let conn = if let Some(proxy) = &self.socks5 {
       // proxied path (implemented in socks5-proxy task)
       dial_quic_via_socks5(proxy, addr, server_name, alpn, creds).await?
   } else {
       // direct path (this task)
       endpoint.connect_with(client_config, addr, server_name)?.await?
   };
   ```
   For this task, the proxy branch can be a `todo!()` or `unimplemented!()` — the
   `socks5-proxy` task fills it in.

### Reference: the old `connect()` (what we're replacing)

The old `CallClient::connect` (lines 142-168 of `call_client.rs`):
```rust
pub async fn connect(&self, addr: SocketAddr, credentials: CallCredentials)
    -> Result<CallConnection, ClientError>
{
    let alpn = b"alknet/call".to_vec();  // hardcoded ALPN
    let client_config = build_quinn_client_config(&credentials, &alpn)?;
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("valid bind addr");
    let endpoint = quinn::Endpoint::client(bind_addr)?;  // builds endpoint internally
    let connection = endpoint.connect_with(client_config, addr, "alknet")?.await?;
    let connection = Connection::from_quinn_with_alpn(connection, alpn);
    Ok(self.spawn_dispatch(connection))  // welds dial to protocol take-over
}
```

The new `dial_quic` differs in every dimension: ALPN is a parameter (not hardcoded),
the endpoint is pre-built (not constructed internally), credentials are
`ConnectionCredentials` (not `CallCredentials`), the return type is `Connection`
(not `CallConnection`), and the error type is `ClientDialError` (not `ClientError`).

### What this does NOT include

- The SOCKS5 proxy path — separate task (`client/socks5-proxy`)
- `dial_tcp_tls` — separate task
- `dial_iroh` — separate task
- Tests — separate task

## Acceptance Criteria

- [ ] `AlknetClient::dial_quic` implemented in `crates/alknet-client/src/dial/quinn.rs`
- [ ] Signature: `pub async fn dial_quic(&self, addr: SocketAddr, server_name: &str, alpn: &[u8], creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
- [ ] Feature-gated on `#[cfg(feature = "quinn")]`
- [ ] Builds `TlsClientConfig::new(creds, alpn)` — `TlsError` converts to `ClientDialError::TlsConfig` via `#[from]`
- [ ] Converts to `quinn::ClientConfig` via `tls_config.for_quinn()`
- [ ] Uses pre-built quinn endpoint from `self.quinn` (cloned)
- [ ] Returns `NoTransport` error when `self.quinn` is `None`
- [ ] Connects via `endpoint.connect_with(client_config, addr, server_name)`
- [ ] `Connect` errors (pre- and post-handshake) map to `ClientDialError::Connect(String)`
- [ ] Returns `Connection::from_quinn_with_alpn(conn, alpn.to_vec())`
- [ ] Does NOT call `spawn_dispatch` (protocol take-over is caller's concern)
- [ ] Does NOT hardcode `alknet/call` ALPN (ALPN is a parameter)
- [ ] SOCKS5 proxy branch is a `todo!()` or conditional on `#[cfg(feature = "socks5")]` (filled in by `client/socks5-proxy`)
- [ ] `cargo check -p alknet-client --features quinn` succeeds
- [ ] `cargo clippy -p alknet-client --features quinn` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/client/README.md — `dial_quic` section (lines 156-171)
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089 §3
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 (verifier selection)
- crates/alknet-tls/src/client.rs — `TlsClientConfig::new` + `for_quinn` (the TLS config the dial consumes)
- crates/alknet-core/src/types.rs — `Connection::from_quinn_with_alpn` (lines 519-526)
- crates/alknet-core/src/credentials.rs — `ConnectionCredentials` (the credential bundle)
- crates/alknet-call/src/client/call_client.rs — old `connect()` (lines 142-168, reference for what NOT to replicate)

## Notes

> This is the primary dial method — QUIC is the default transport for native alknet
> connections. The implementation is straightforward because `TlsClientConfig` and
> `Connection::from_quinn_with_alpn` already exist. The key difference from the old
> `connect()`: ALPN is a parameter (not hardcoded), the endpoint is pre-built (not
> constructed internally), credentials are `ConnectionCredentials` (not
> `CallCredentials`), and the return type is `Connection` (not `CallConnection`).
> The SOCKS5 proxy path is a separate task — this task implements the direct path.

## Summary

> To be filled on completion

---
id: client/dial-tcp-tls
name: Implement dial_tcp_tls — TCP+TLS dial via tokio-rustls, producing a Connection
status: completed
depends_on: [client/client-core]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 3, Task 5. Implement `AlknetClient::dial_tcp_tls` in `crates/alknet-client/src/dial/tcp_tls.rs`.
The TCP+TLS dial: builds a `TlsClientConfig` from `ConnectionCredentials`, constructs a
`TlsConnector`, connects a `TcpStream` to `addr`, wraps with TLS using `host` as the SNI,
and returns a `Connection` via `Connection::from_bidi` (ADR-065).

This is a **fresh build** — there is no existing TCP+TLS dial in the codebase to reference.
The old `CallClient::connect` was QUIC-only. This is the second transport's dial, validating
the transport-polymorphic design.

### Target shape (per architecture spec)

```rust
impl AlknetClient {
    /// TCP+TLS dial. Builds a `TlsClientConfig` from `creds`,
    /// connects a `TcpStream` to `addr`, wraps with `TlsConnector`
    /// using `host` as the SNI, returns a `Connection` via
    /// `Connection::from_bidi` (ADR-065). Feature-gated on `tcp`.
    #[cfg(feature = "tcp")]
    pub async fn dial_tcp_tls(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;
}
```

### Implementation outline

```rust
#[cfg(feature = "tcp")]
pub async fn dial_tcp_tls(
    &self,
    host: &str,
    addr: SocketAddr,
    alpn: &[u8],
    creds: &ConnectionCredentials,
) -> Result<Connection, ClientDialError> {
    // 1. Build TlsClientConfig from credentials + ALPN
    let tls_config = TlsClientConfig::new(creds, alpn)?;

    // 2. Build or use the TlsConnector
    let connector = match &self.tcp_connector {
        Some(c) => c.clone(),
        None => {
            // If no pre-built connector, build one from the TlsClientConfig.
            // The assembly layer can either pre-build a TlsConnector (via
            // with_tcp_tls) or let the dial build one from the rustls config.
            // For now, build from the TlsClientConfig's inner rustls config.
            let rustls_config = Arc::new(tls_config.into_rustls_config());
            tokio_rustls::TlsConnector::from(rustls_config)
        }
    };

    // 3. Connect TCP
    let tcp_stream = TcpStream::connect(addr)
        .await
        .map_err(|e| ClientDialError::Connect(e.to_string()))?;

    // 4. TLS handshake
    let server_name = rustls::pki_types::ServerName::try_from(host)
        .map_err(|e| ClientDialError::Connect(e.to_string()))?;
    let tls_stream = connector
        .connect(server_name, tcp_stream)
        .await
        .map_err(|e| ClientDialError::Handshake(e.to_string()))?;

    // 5. Wrap as Connection (single bidi stream — ADR-065)
    Ok(Connection::from_bidi(
        tls_stream,
        alpn.to_vec(),
        Some(addr),
    ))
}
```

### Key design decisions

1. **`host` is the TLS SNI**: The hostname for TLS SNI. For X.509 endpoints, this must
   match the server's certificate. For raw-key endpoints, it's ignored by the verifier.
   Separate from `addr` because the hostname may differ from the IP address (DNS
   resolution happens at the assembly layer).

2. **`addr` is the `SocketAddr`**: The IP:port to connect to. The assembly layer
   resolves the hostname to an address before calling the dial.

3. **`Connection::from_bidi`**: TCP+TLS is a single-stream transport — there's one
   bidirectional stream (the TLS-wrapped TCP connection). `from_bidi` splits it
   internally via `tokio::io::split` and wraps it as a `Connection`. `accept_bi()`
   yields the stream once, then `ConnectionClosed` (ADR-070's yield-once contract).

4. **`TlsConnector` from pre-built or on-the-fly**: The assembly layer can either
   pre-build a `TlsConnector` via `with_tcp_tls` or let the dial build one from the
   `TlsClientConfig`. The implementation supports both: if `self.tcp_connector` is
   `Some`, use it; otherwise build from the `TlsClientConfig`'s inner rustls config.

5. **`Handshake` vs `Connect` errors**: TCP connect failures map to
   `ClientDialError::Connect`. TLS handshake failures (rejected cert, ALPN mismatch)
   map to `ClientDialError::Handshake`. This distinction lets callers differentiate
   "couldn't reach the server" from "the server rejected our identity."

6. **SOCKS5 proxy path**: When `self.socks5` is `Some`, the dial routes through the
   proxy (CONNECT) instead of connecting directly. The proxy path: connect TCP to the
   proxy, perform SOCKS5 CONNECT handshake to `addr`, then TLS over the proxied stream.
   This is implemented in the `client/socks5-proxy` task — this task implements the
   direct (no-proxy) path.

### What this does NOT include

- The SOCKS5 proxy path — separate task (`client/socks5-proxy`)
- `dial_quic` — separate task
- `dial_iroh` — separate task
- Tests — separate task

## Acceptance Criteria

- [ ] `AlknetClient::dial_tcp_tls` implemented in `crates/alknet-client/src/dial/tcp_tls.rs`
- [ ] Signature: `pub async fn dial_tcp_tls(&self, host: &str, addr: SocketAddr, alpn: &[u8], creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
- [ ] Feature-gated on `#[cfg(feature = "tcp")]`
- [ ] Builds `TlsClientConfig::new(creds, alpn)` — `TlsError` converts to `ClientDialError::TlsConfig` via `#[from]`
- [ ] Uses pre-built `TlsConnector` from `self.tcp_connector` if available, or builds one from `TlsClientConfig`
- [ ] Connects TCP via `TcpStream::connect(addr)`
- [ ] `Connect` errors map to `ClientDialError::Connect(String)`
- [ ] Performs TLS handshake via `connector.connect(server_name, tcp_stream)`
- [ ] `Handshake` errors map to `ClientDialError::Handshake(String)`
- [ ] Returns `Connection::from_bidi(tls_stream, alpn.to_vec(), Some(addr))`
- [ ] Does NOT call `spawn_dispatch` (protocol take-over is caller's concern)
- [ ] SOCKS5 proxy branch is a `todo!()` or conditional on `#[cfg(feature = "socks5")]` (filled in by `client/socks5-proxy`)
- [ ] `cargo check -p alknet-client --features tcp` succeeds
- [ ] `cargo clippy -p alknet-client --features tcp` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/client/README.md — `dial_tcp_tls` section (lines 173-184)
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089 §3
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- docs/architecture/decisions/065-connection-from-stream-generic-single-stream.md — ADR-065 (`Connection::from_bidi`)
- docs/architecture/decisions/070-bidistreamsource-trait.md — ADR-070 (yield-once contract)
- crates/alknet-tls/src/client.rs — `TlsClientConfig::new` (the TLS config the dial consumes)
- crates/alknet-core/src/types.rs — `Connection::from_bidi` (lines 562-569)
- crates/alknet-core/src/credentials.rs — `ConnectionCredentials` (the credential bundle)

## Notes

> This is the second transport's dial — the one that validates the transport-polymorphic
> design. There is no existing TCP+TLS dial in the codebase to reference (the old
> `connect()` was QUIC-only). The implementation is straightforward because
> `TlsClientConfig`, `TlsConnector`, and `Connection::from_bidi` already exist.
> The `TlsConnector` can be pre-built by the assembly layer (via `with_tcp_tls`) or
> built on-the-fly from the `TlsClientConfig`. The SOCKS5 proxy path is a separate task.

## Summary

> To be filled on completion

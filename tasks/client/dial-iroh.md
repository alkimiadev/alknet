---
id: client/dial-iroh
name: Implement dial_iroh — iroh dial, producing a Connection
status: pending
depends_on: [client/client-core]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 3, Task 6. Implement `AlknetClient::dial_iroh` in `crates/alknet-client/src/dial/iroh.rs`.
The iroh dial: extracts the `Ed25519SecretKey` from `ConnectionCredentials.local_identity`,
derives the remote `NodeId` from `creds.remote_identity.fingerprint`, dials on `alpn` via
the iroh endpoint, and returns a `Connection` via `Connection::from_iroh`.

This is the **key-not-config** dial — iroh has its own TLS (shares the key, not the
rustls config — ADR-087 §3). The dial does NOT use `TlsClientConfig`. The consistency
is in the rule (ADR-034 verifier selection), not in the type.

### Target shape (per architecture spec)

```rust
impl AlknetClient {
    /// Iroh dial. Dials on `alpn` via the iroh endpoint. The iroh path
    /// does NOT use `TlsClientConfig` — iroh has its own TLS (shares the
    /// `Ed25519SecretKey`, not the rustls config — ADR-087 §3, ADR-089
    /// §3). The local key is extracted from `creds.local_identity`; the
    /// remote `NodeId` is derived from `creds.remote_identity.fingerprint`
    /// (`ed25519:<hex>` → `NodeId::from_bytes`). The verifier is iroh's
    /// `NodeId` match (fingerprint pin by another name — ADR-034 §3).
    /// An unknown iroh remote fails closed (no CA). Feature-gated on
    /// `iroh`.
    #[cfg(feature = "iroh")]
    pub async fn dial_iroh(
        &self,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError>;
}
```

### Implementation outline

```rust
#[cfg(feature = "iroh")]
pub async fn dial_iroh(
    &self,
    alpn: &[u8],
    creds: &ConnectionCredentials,
) -> Result<Connection, ClientDialError> {
    // 1. Get the iroh endpoint
    let endpoint = match &self.iroh {
        Some(ep) => ep.clone(),
        None => return Err(ClientDialError::NoTransport { transport: "iroh" }),
    };

    // 2. Extract the remote NodeId from credentials
    let node_id = match &creds.remote_identity {
        Some(ri) => {
            // fingerprint format: "ed25519:<hex>" or "SHA256:<base64>"
            // For iroh, we need the ed25519 hex bytes → NodeId
            extract_iroh_node_id(&ri.fingerprint)
                .map_err(|e| ClientDialError::TlsConfig(
                    alknet_tls::TlsError::Config(e)
                ))?
        }
        None => {
            // Unknown iroh remote — fail closed (no CA to fall back to)
            return Err(ClientDialError::TlsConfig(
                alknet_tls::TlsError::Config(
                    "iroh requires a known remote (remote_identity must be Some); \
                     unknown iroh remotes fail closed (ADR-034 §3)".into()
                )
            ));
        }
    };

    // 3. Connect via iroh
    let conn = endpoint
        .connect(node_id, alpn)
        .await
        .map_err(|e| ClientDialError::Connect(e.to_string()))?;

    // 4. Wrap as Connection
    Ok(Connection::from_iroh(conn))
}
```

### Key design decisions

1. **No `TlsClientConfig`**: iroh has its own TLS. The dial does not use
   `TlsClientConfig` at all — it extracts the key and fingerprint directly from
   `ConnectionCredentials`. This is the same exception as the server side
   (ADR-082, ADR-087 §3).

2. **`node_id` derived from `remote_identity.fingerprint`**: The fingerprint string
   (e.g., `"ed25519:abcdef123456..."`) is parsed to extract the Ed25519 public key
   bytes, then converted to `iroh::NodeId::from_bytes`. This is the same extraction
   pattern the rustls dials use for the verifier — the consistency is in the rule
   (ADR-034), not in the type.

3. **Unknown iroh remote fails closed**: `remote_identity: None` with iroh returns
   a `TlsConfig` error. There is no CA to fall back to for iroh — raw-key remotes
   are always known peers (ADR-034 §2, Assumption 1).

4. **No `addr` or `server_name` parameter**: iroh handles addressing internally
   (via relays, hole-punching, etc.). The dial only needs the `NodeId` and ALPN.

5. **Returns `Connection`, not `CallConnection`**: Same as the other dials — the
   protocol take-over is the caller's concern.

6. **SOCKS5 proxy path**: When `self.socks5` is `Some`, the iroh endpoint should
   have been built with force-relay-only + `proxy_url` by the assembly layer
   (ADR-090 §5). The dial itself doesn't change — the proxy is applied at
   endpoint construction time, not at dial time. This task does not need to
   handle the proxy path specially.

### Helper: `extract_iroh_node_id`

```rust
/// Extract an `iroh::NodeId` from a fingerprint string.
///
/// Supports two formats:
/// - `"ed25519:<hex>"` — raw Ed25519 public key (64 hex chars)
/// - `"SHA256:<base64>"` — SHA-256 hash of the cert (for X.509; not valid for iroh)
///
/// For iroh, only the `ed25519:` prefix is valid — iroh uses Ed25519 keys.
fn extract_iroh_node_id(fingerprint: &str) -> Result<iroh::NodeId, String> {
    if let Some(hex) = fingerprint.strip_prefix("ed25519:") {
        let bytes = hex::decode(hex).map_err(|e| format!("invalid ed25519 fingerprint hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "invalid ed25519 fingerprint length: expected 32 bytes, got {}",
                bytes.len()
            ));
        }
        let arr: [u8; 32] = bytes.try_into().map_err(|_| "invalid ed25519 fingerprint length".to_string())?;
        Ok(iroh::NodeId::from_bytes(&arr)?)
    } else {
        Err(format!(
            "iroh requires an ed25519: fingerprint, got: {}",
            fingerprint
        ))
    }
}
```

### What this does NOT include

- The SOCKS5 proxy path for iroh — the proxy is applied at endpoint construction time
  by the assembly layer (ADR-090 §5), not at dial time
- `dial_quic` — separate task
- `dial_tcp_tls` — separate task
- Tests — separate task

## Acceptance Criteria

- [ ] `AlknetClient::dial_iroh` implemented in `crates/alknet-client/src/dial/iroh.rs`
- [ ] Signature: `pub async fn dial_iroh(&self, alpn: &[u8], creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
- [ ] Feature-gated on `#[cfg(feature = "iroh")]`
- [ ] Uses pre-built iroh endpoint from `self.iroh` (cloned)
- [ ] Returns `NoTransport` error when `self.iroh` is `None`
- [ ] Extracts `NodeId` from `creds.remote_identity.fingerprint` (supports `ed25519:<hex>` format)
- [ ] Unknown iroh remote (`remote_identity: None`) fails closed with `TlsConfig` error
- [ ] Connects via `endpoint.connect(node_id, alpn)`
- [ ] `Connect` errors map to `ClientDialError::Connect(String)`
- [ ] Returns `Connection::from_iroh(conn)`
- [ ] Does NOT use `TlsClientConfig` (iroh has its own TLS)
- [ ] Does NOT call `spawn_dispatch` (protocol take-over is caller's concern)
- [ ] Does NOT take `addr` or `server_name` parameters (iroh handles addressing internally)
- [ ] `cargo check -p alknet-client --features iroh` succeeds
- [ ] `cargo clippy -p alknet-client --features iroh` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds (old code untouched)

## References

- docs/architecture/crates/client/README.md — `dial_iroh` section (lines 186-201)
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089 §3
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- docs/architecture/decisions/087-tlsclientconfig-not-blocked-on-dial.md — ADR-087 §3 (iroh shares the key, not the config)
- docs/architecture/decisions/034-outgoing-only-x509-and-three-peer-roles.md — ADR-034 §2-3 (verifier selection, fail-closed for unknown raw-key)
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090 §5 (iroh proxy: force relay-only, applied at endpoint construction)
- crates/alknet-core/src/types.rs — `Connection::from_iroh` (lines 528-536)
- crates/alknet-core/src/credentials.rs — `ConnectionCredentials` (the credential bundle)
- crates/alknet-core/src/config.rs — `Ed25519SecretKey` (the key type)

## Notes

> This is the key-not-config dial — iroh has its own TLS and does not use
> `TlsClientConfig`. The dial extracts the key and fingerprint directly from
> `ConnectionCredentials`. The `node_id` is derived from `remote_identity.fingerprint`
> (the same extraction pattern the rustls dials use for the verifier). Unknown iroh
> remotes fail closed (no CA to fall back to). The SOCKS5 proxy for iroh is applied
> at endpoint construction time by the assembly layer (force relay-only +
> `proxy_url`), not at dial time — this task does not need to handle it.

## Summary

> To be filled on completion

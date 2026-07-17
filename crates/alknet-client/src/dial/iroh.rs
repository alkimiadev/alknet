//! `dial_iroh` — iroh dial, producing a `Connection`.
//!
//! Feature-gated on `iroh`. The iroh path does NOT use `TlsClientConfig` —
//! iroh has its own TLS (shares the `Ed25519SecretKey`, not the rustls config
//! — ADR-087 §3, ADR-089 §3). The local key is extracted from
//! `creds.local_identity`; the remote `EndpointId` is derived from
//! `creds.remote_identity.fingerprint`.

use alknet_core::credentials::ConnectionCredentials;
use alknet_core::types::Connection;

use crate::error::ClientDialError;
use crate::client::AlknetClient;

impl AlknetClient {
    /// Iroh dial. Dials on `alpn` via the iroh endpoint. The iroh path
    /// does NOT use `TlsClientConfig` — iroh has its own TLS (shares the
    /// `Ed25519SecretKey`, not the rustls config — ADR-087 §3, ADR-089
    /// §3). The local key is extracted from `creds.local_identity`; the
    /// remote `EndpointId` is derived from `creds.remote_identity.fingerprint`
    /// (`ed25519:<hex>` → `EndpointId::from_bytes`). The verifier is iroh's
    /// `EndpointId` match (fingerprint pin by another name — ADR-034 §3).
    /// An unknown iroh remote fails closed (no CA). Feature-gated on
    /// `iroh`.
    #[cfg(feature = "iroh")]
    pub async fn dial_iroh(
        &self,
        alpn: &[u8],
        creds: &ConnectionCredentials,
    ) -> Result<Connection, ClientDialError> {
        let endpoint = self.iroh.as_ref().ok_or(ClientDialError::NoTransport {
            transport: "iroh",
        })?;

        let node_id = match &creds.remote_identity {
            Some(ri) => extract_iroh_endpoint_id(&ri.fingerprint).map_err(|e| {
                ClientDialError::TlsConfig(alknet_tls::TlsError::Config(e))
            })?,
            None => {
                return Err(ClientDialError::TlsConfig(alknet_tls::TlsError::Config(
                    "iroh requires a known remote (remote_identity must be Some); \
                     unknown iroh remotes fail closed (ADR-034 §3)"
                        .into(),
                )));
            }
        };

        let conn = endpoint
            .connect(node_id, alpn)
            .await
            .map_err(|e| ClientDialError::Connect(e.to_string()))?;

        Ok(Connection::from_iroh(conn))
    }
}

/// Extract an `iroh::EndpointId` from a fingerprint string.
///
/// Supports two formats:
/// - `"ed25519:<hex>"` — raw Ed25519 public key (64 hex chars)
/// - `"SHA256:<base64>"` — SHA-256 hash of the cert (for X.509; not valid for iroh)
///
/// For iroh, only the `ed25519:` prefix is valid — iroh uses Ed25519 keys.
fn extract_iroh_endpoint_id(fingerprint: &str) -> Result<iroh::EndpointId, String> {
    if let Some(hex_str) = fingerprint.strip_prefix("ed25519:") {
        let bytes =
            hex::decode(hex_str).map_err(|e| format!("invalid ed25519 fingerprint hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "invalid ed25519 fingerprint length: expected 32 bytes, got {}",
                bytes.len()
            ));
        }
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "invalid ed25519 fingerprint length".to_string())?;
        iroh::EndpointId::from_bytes(&arr)
            .map_err(|e| format!("invalid iroh EndpointId: {e}"))
    } else {
        Err(format!(
            "iroh requires an ed25519: fingerprint, got: {}",
            fingerprint
        ))
    }
}

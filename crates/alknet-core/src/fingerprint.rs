//! TLS certificate fingerprint extraction (ADR-030 §6, ADR-034 §3).
//!
//! Fingerprint formats:
//! - **Ed25519 raw key** (RFC 7250 SPKI): `ed25519:<hex of 32-byte pub key>`.
//!   The fingerprint IS the trust anchor — raw-key remotes have no CA, so the
//!   fingerprint is the identity (ADR-034 §2 assumption 1). Normalized to
//!   `ed25519:<hex>` across quinn and iroh (ADR-030 §6).
//! - **X.509 cert**: `SHA256:<hex of DER>`. Used by the hub X.509 path
//!   (ADR-034 §3 — fingerprint pinning for known hubs with a prior P2P trust
//!   relationship). Not used for arbitrary public APIs (those use CA
//!   verification via `WebPkiServerVerifier`, not fingerprint pinning).
//!
//! Shared by the server-side endpoint (`alknet_core::endpoint`, which extracts
//! the fingerprint from the presented client cert for `PeerEntry` resolution)
//! and the client-side `FingerprintPinVerifier` in `alknet_call::client`
//! (which matches the server's presented cert against a pinned fingerprint).

use sha2::{Digest, Sha256};

/// Compute the fingerprint of a TLS certificate DER (RFC 7250 raw public key
/// SPKI or X.509 cert). Returns `ed25519:<hex>` when `cert_der` is an Ed25519
/// SPKI, otherwise `SHA256:<hex of full DER>`. Returns `None` only when the
/// input is empty (a non-Ed25519 SPKI or a malformed DER still hashes to a
/// `SHA256:` fingerprint — the hash is the fallback).
pub fn fingerprint_from_cert_der(cert_der: &[u8]) -> Option<String> {
    if let Some(raw_key) = extract_ed25519_raw_key_from_spki(cert_der) {
        return Some(format!("ed25519:{}", hex::encode(raw_key)));
    }
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    let digest = hasher.finalize();
    Some(format!("SHA256:{}", hex::encode(digest)))
}

/// `SubjectPublicKeyInfo ::= SEQUENCE { algorithm AlgorithmIdentifier, subjectPublicKey BIT STRING }`
/// `AlgorithmIdentifier ::= SEQUENCE { algorithm OBJECT IDENTIFIER, parameters ANY OPTIONAL }`
/// For Ed25519 the algorithm OID is `1.3.101.112` (DER bytes `2b 65 70`), with
/// no parameters, and `subjectPublicKey` is a BIT STRING containing one
/// unused-bits byte (`0x00`) followed by the 32-byte raw Ed25519 public key.
/// Returns the 32 raw key bytes when `cert_der` is an RFC 7250 raw public key
/// (SPKI) with the Ed25519 algorithm identifier; returns `None` otherwise
/// (X.509 cert, non-Ed25519 SPKI, or malformed DER), in which case callers
/// should fall back to hashing the full DER.
pub fn extract_ed25519_raw_key_from_spki(cert_der: &[u8]) -> Option<[u8; 32]> {
    const ED25519_OID_BYTES: [u8; 3] = [0x2b, 0x65, 0x70];

    let mut parser = DerParser::new(cert_der);
    let spki_contents = parser.expect_sequence()?;
    let mut spki_parser = DerParser::new(spki_contents);

    let alg_id_contents = spki_parser.expect_sequence()?;
    let mut alg_id_parser = DerParser::new(alg_id_contents);
    let oid_bytes = alg_id_parser.expect_oid()?;
    if oid_bytes != ED25519_OID_BYTES {
        return None;
    }

    let bit_string_contents = spki_parser.expect_bit_string()?;
    if bit_string_contents.len() != 33 || bit_string_contents[0] != 0x00 {
        return None;
    }
    let mut raw_key = [0u8; 32];
    raw_key.copy_from_slice(&bit_string_contents[1..33]);
    Some(raw_key)
}

struct DerParser<'a> {
    bytes: &'a [u8],
}

impl<'a> DerParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn read_tlv(&mut self) -> Option<(u8, &'a [u8])> {
        let (tag, len_size, header_len) = self.decode_header()?;
        let total = header_len.checked_add(len_size)?;
        if total > self.bytes.len() {
            return None;
        }
        let content = &self.bytes[header_len..total];
        self.bytes = &self.bytes[total..];
        Some((tag, content))
    }

    fn decode_header(&self) -> Option<(u8, usize, usize)> {
        if self.bytes.is_empty() {
            return None;
        }
        let tag = self.bytes[0];
        if self.bytes.len() < 2 {
            return None;
        }
        let first_len = self.bytes[1];
        if first_len < 0x80 {
            return Some((tag, first_len as usize, 2));
        }
        let num_bytes = (first_len & 0x7f) as usize;
        if num_bytes == 0 || num_bytes > 4 {
            return None;
        }
        if self.bytes.len() < 2 + num_bytes {
            return None;
        }
        let mut len: usize = 0;
        for i in 0..num_bytes {
            len = (len << 8) | (self.bytes[2 + i] as usize);
        }
        Some((tag, len, 2 + num_bytes))
    }

    fn expect_sequence(&mut self) -> Option<&'a [u8]> {
        let (tag, content) = self.read_tlv()?;
        if tag == 0x30 {
            Some(content)
        } else {
            None
        }
    }

    fn expect_oid(&mut self) -> Option<&'a [u8]> {
        let (tag, content) = self.read_tlv()?;
        if tag == 0x06 {
            Some(content)
        } else {
            None
        }
    }

    fn expect_bit_string(&mut self) -> Option<&'a [u8]> {
        let (tag, content) = self.read_tlv()?;
        if tag == 0x03 {
            Some(content)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_ed25519_spki_der(raw_key: &[u8; 32]) -> Vec<u8> {
        let spki = rustls::sign::public_key_to_spki(&rustls::pki_types::alg_id::ED25519, raw_key);
        spki.to_vec()
    }

    #[test]
    fn fingerprint_from_cert_der_produces_sha256_hex_format() {
        let cert_der = b"fake-leaf-cert-der-bytes";
        let fp = fingerprint_from_cert_der(cert_der).expect("non-empty cert produces fingerprint");
        assert!(
            fp.starts_with("SHA256:"),
            "fingerprint must be SHA256-prefixed, got: {fp}"
        );
        let hex_part = &fp["SHA256:".len()..];
        assert_eq!(
            hex_part.len(),
            64,
            "hex digest must be 64 chars (32 bytes), got: {fp}"
        );
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hex part must be lowercase hex, got: {fp}"
        );
        let mut hasher = Sha256::new();
        hasher.update(cert_der);
        let expected = format!("SHA256:{}", hex::encode(hasher.finalize()));
        assert_eq!(fp, expected, "fingerprint must match SHA-256 of cert DER");
    }

    #[test]
    fn fingerprint_from_cert_der_deterministic() {
        let cert = b"some-cert";
        let a = fingerprint_from_cert_der(cert).unwrap();
        let b = fingerprint_from_cert_der(cert).unwrap();
        assert_eq!(a, b, "same cert DER must produce same fingerprint");
    }

    #[test]
    fn fingerprint_from_ed25519_spki_produces_ed25519_prefix() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let raw_key = sk.public().to_bytes();
        let spki_der = build_ed25519_spki_der(&raw_key);
        let fp = fingerprint_from_cert_der(&spki_der).expect("spki produces fingerprint");
        assert!(
            fp.starts_with("ed25519:"),
            "Ed25519 raw key SPKI must produce ed25519: fingerprint, got: {fp}"
        );
    }

    #[test]
    fn fingerprint_from_ed25519_spki_is_lowercase_hex_of_32_byte_key() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let raw_key = sk.public().to_bytes();
        let spki_der = build_ed25519_spki_der(&raw_key);
        let fp = fingerprint_from_cert_der(&spki_der).expect("spki produces fingerprint");
        let hex_part = &fp["ed25519:".len()..];
        assert_eq!(
            hex_part.len(),
            64,
            "ed25519 hex part must be 64 chars (32 bytes), got: {fp}"
        );
        assert!(
            hex_part
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "ed25519 hex part must be lowercase hex, got: {fp}"
        );
        assert_eq!(
            hex_part,
            hex::encode(raw_key),
            "ed25519 fingerprint must be hex of the raw 32-byte key, not the DER wrapper"
        );
    }

    #[test]
    fn fingerprint_from_ed25519_spki_matches_iroh_format() {
        let sk = crate::config::Ed25519SecretKey::generate();
        let raw_key = sk.public().to_bytes();
        let spki_der = build_ed25519_spki_der(&raw_key);
        let quinn_fp = fingerprint_from_cert_der(&spki_der).expect("spki produces fingerprint");
        let iroh_fp = format!("ed25519:{}", hex::encode(raw_key));
        assert_eq!(
            quinn_fp, iroh_fp,
            "same Ed25519 key must produce the same fingerprint via quinn SPKI and iroh NodeId paths"
        );
    }

    #[test]
    fn fingerprint_from_x509_cert_stays_sha256_of_der() {
        let cert_der = b"fake-x509-cert-der-bytes-not-an-spki";
        let fp = fingerprint_from_cert_der(cert_der).expect("x509 produces fingerprint");
        assert!(
            fp.starts_with("SHA256:"),
            "X.509 cert must keep SHA256: format, got: {fp}"
        );
        let mut hasher = Sha256::new();
        hasher.update(cert_der);
        assert_eq!(
            fp,
            format!("SHA256:{}", hex::encode(hasher.finalize())),
            "X.509 fingerprint must be SHA-256 of cert DER"
        );
    }

    #[test]
    fn fingerprint_from_non_ed25519_spki_falls_back_to_sha256() {
        let raw_key = [0u8; 32];
        let fake_non_ed25519_spki: Vec<u8> = vec![
            0x30, 0x1c, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x06, 0x01, 0x03, 0x15, 0x00, 0x20,
        ]
        .into_iter()
        .chain(raw_key.iter().copied())
        .collect();
        let fp = fingerprint_from_cert_der(&fake_non_ed25519_spki).expect("fallback fingerprint");
        assert!(
            fp.starts_with("SHA256:"),
            "non-Ed25519 SPKI must fall back to SHA256, got: {fp}"
        );
    }
}

//! Ed25519 signing key usable as both a rustls `SigningKey` and `Signer`.
//! Consolidated — one copy used by both server (`RawKeyCertResolver`) and
//! client (`RawKeyClientCertResolver`).

#[derive(Clone)]
pub struct Ed25519SigningKey {
    key: alknet_core::config::Ed25519SecretKey,
}

impl std::fmt::Debug for Ed25519SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ed25519SigningKey").finish()
    }
}

impl Ed25519SigningKey {
    pub fn new(key: alknet_core::config::Ed25519SecretKey) -> Self {
        Self { key }
    }

    pub fn spki_public_key(&self) -> rustls::pki_types::SubjectPublicKeyInfoDer<'static> {
        rustls::sign::public_key_to_spki(
            &rustls::pki_types::alg_id::ED25519,
            self.key.public().as_bytes(),
        )
    }
}

impl rustls::sign::SigningKey for Ed25519SigningKey {
    fn choose_scheme(
        &self,
        offered: &[rustls::SignatureScheme],
    ) -> Option<Box<dyn rustls::sign::Signer>> {
        if offered.contains(&rustls::SignatureScheme::ED25519) {
            Some(Box::new(self.clone()))
        } else {
            None
        }
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ED25519
    }

    fn public_key(&self) -> Option<rustls::pki_types::SubjectPublicKeyInfoDer<'_>> {
        Some(self.spki_public_key())
    }
}

impl rustls::sign::Signer for Ed25519SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        Ok(self.key.sign(message).to_bytes().to_vec())
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ED25519
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ed25519_signing_key_choose_scheme_returns_some_for_ed25519() {
        use rustls::sign::SigningKey;
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key.choose_scheme(&[rustls::SignatureScheme::ED25519]);
        assert!(
            signer.is_some(),
            "must produce a signer when ED25519 is offered"
        );
    }

    #[test]
    fn ed25519_signing_key_choose_scheme_returns_none_without_ed25519() {
        use rustls::sign::SigningKey;
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key.choose_scheme(&[rustls::SignatureScheme::RSA_PSS_SHA256]);
        assert!(
            signer.is_none(),
            "must not produce a signer when ED25519 is not offered"
        );
    }

    #[test]
    fn ed25519_signing_key_algorithm_is_ed25519() {
        use rustls::sign::SigningKey;
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        assert_eq!(signing_key.algorithm(), rustls::SignatureAlgorithm::ED25519);
    }

    #[test]
    fn ed25519_signing_key_public_key_returns_spki() {
        use rustls::sign::SigningKey;
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let spki = signing_key.public_key();
        assert!(spki.is_some(), "public_key must return an SPKI");
        assert!(!spki.unwrap().as_ref().is_empty(), "SPKI must be non-empty");
    }

    #[test]
    fn ed25519_signing_key_signer_signs_message() {
        use rustls::sign::SigningKey;
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let signer = signing_key
            .choose_scheme(&[rustls::SignatureScheme::ED25519])
            .expect("ED25519 offered");
        let message = b"alknet coverage signing test";
        let sig = signer.sign(message).expect("sign must succeed");
        assert_eq!(sig.len(), 64, "ed25519 signature must be 64 bytes");
        assert_eq!(signer.scheme(), rustls::SignatureScheme::ED25519);
    }

    #[test]
    fn ed25519_signing_key_debug_does_not_leak_material() {
        let sk = alknet_core::config::Ed25519SecretKey::generate();
        let signing_key = Ed25519SigningKey::new(sk);
        let dbg = format!("{signing_key:?}");
        assert!(dbg.contains("Ed25519SigningKey"));
    }
}

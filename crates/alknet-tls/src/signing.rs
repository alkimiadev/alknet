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

//! PEM loading helpers: `load_cert_chain`, `load_private_key`.
//! Consolidated — one copy used by both server and client.

use std::io;
use std::path::Path;

use crate::TlsError;

pub fn load_cert_chain(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, TlsError> {
    let bytes = std::fs::read(path).map_err(TlsError::Io)?;
    let mut reader = io::BufReader::new(bytes.as_slice());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsError::Io(io::Error::other(e)))
}

pub fn load_private_key(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, TlsError> {
    let bytes = std::fs::read(path).map_err(TlsError::Io)?;
    let mut reader = io::BufReader::new(bytes.as_slice());
    match rustls_pemfile::private_key(&mut reader) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(TlsError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "no private key found in file",
        ))),
        Err(e) => Err(TlsError::Io(io::Error::other(e))),
    }
}

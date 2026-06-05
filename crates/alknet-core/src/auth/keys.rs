//! Key loading and parsing for SSH authentication.
//!
//! Supports `KeySource` (file path or in-memory) for private keys, public keys,
//! and certificate authority entries. All keys must be in OpenSSH format.
//! PEM-encoded keys (PKCS#1, PKCS#8) are rejected with a clear error message.

use std::path::PathBuf;

use russh::keys::{PrivateKey, PublicKey, decode_secret_key, parse_public_key_base64};

use crate::error::ConfigError;

/// Source for key material — either a filesystem path or in-memory bytes.
///
/// Used throughout the API to accept keys without committing to a specific
/// loading mechanism. In-memory keys are primarily for the NAPI wrapper.
#[derive(Debug, Clone)]
pub enum KeySource {
    File(PathBuf),
    Memory(Vec<u8>),
}

/// A certificate authority entry parsed from an `authorized_keys` file.
///
/// Contains the CA public key and its associated options (e.g., `cert-authority`,
/// `permit-port-forwarding`). Used by `ServerAuthConfig` for certificate validation.
#[derive(Debug, Clone)]
pub struct CertAuthorityEntry {
    pub public_key: PublicKey,
    pub options: Vec<String>,
}

fn resolve_bytes(source: &KeySource) -> Result<Vec<u8>, ConfigError> {
    match source {
        KeySource::File(path) => {
            if !path.exists() {
                return Err(ConfigError::KeyFileNotFound {
                    path: path.display().to_string(),
                });
            }
            std::fs::read(path).map_err(|_| ConfigError::KeyFileNotFound {
                path: path.display().to_string(),
            })
        }
        KeySource::Memory(data) => Ok(data.clone()),
    }
}

fn check_openssh_private_key(data: &[u8]) -> Result<(), ConfigError> {
    let s = String::from_utf8_lossy(data);
    if s.contains("-----BEGIN OPENSSH PRIVATE KEY-----") {
        return Ok(());
    }
    if s.contains("-----BEGIN RSA PRIVATE KEY-----")
        || s.contains("-----BEGIN PRIVATE KEY-----")
        || s.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
        || s.contains("-----BEGIN EC PRIVATE KEY-----")
    {
        return Err(ConfigError::InvalidFlag {
            name: "PEM-encoded key is not supported; use OpenSSH format (-----BEGIN OPENSSH PRIVATE KEY-----)".to_string(),
        });
    }
    Err(ConfigError::InvalidFlag {
        name: "unrecognized private key format; expected OpenSSH format (-----BEGIN OPENSSH PRIVATE KEY-----)".to_string(),
    })
}

pub fn load_private_key(source: KeySource) -> Result<PrivateKey, ConfigError> {
    let data = resolve_bytes(&source)?;
    check_openssh_private_key(&data)?;
    let s = String::from_utf8_lossy(&data);
    decode_secret_key(&s, None).map_err(|e| ConfigError::InvalidFlag {
        name: format!("failed to decode private key: {e}"),
    })
}

fn parse_authorized_keys_line(line: &str) -> Option<Result<(PublicKey, Vec<String>), ConfigError>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts: Vec<&str> = line.splitn(4, ' ').collect();
    if parts.len() < 2 {
        return None;
    }

    let mut options = Vec::new();
    let key_type_idx;

    if parts[0].starts_with("cert-authority")
        || parts[0].starts_with("no-")
        || parts[0].starts_with("permit-")
        || parts[0].starts_with("from=")
        || parts[0].starts_with("command=")
        || parts[0].starts_with("environment=")
        || parts[0].starts_with("tunnel=")
        || parts[0].starts_with("principals=")
    {
        let opts_str = parts[0];
        options = opts_str
            .split(',')
            .map(|s| s.to_string())
            .collect();
        key_type_idx = 1;
    } else if parts[0].starts_with("ssh-") || parts[0].starts_with("ecdsa-") {
        key_type_idx = 0;
    } else {
        return None;
    }

    if parts.len() <= key_type_idx {
        return None;
    }

    let key_base64 = parts[key_type_idx + 1];
    match parse_public_key_base64(key_base64) {
        Ok(pk) => Some(Ok((pk, options))),
        Err(_) => None,
    }
}

pub fn load_public_keys(source: KeySource) -> Result<Vec<PublicKey>, ConfigError> {
    let data = resolve_bytes(&source)?;
    let s = String::from_utf8_lossy(&data);
    let mut keys = Vec::new();
    for line in s.lines() {
        if let Some(Ok((pk, _))) = parse_authorized_keys_line(line) {
            keys.push(pk);
        }
    }
    Ok(keys)
}

pub fn load_cert_authority_entries(
    source: KeySource,
) -> Result<Vec<CertAuthorityEntry>, ConfigError> {
    let data = resolve_bytes(&source)?;
    let s = String::from_utf8_lossy(&data);
    let mut entries = Vec::new();
    for line in s.lines() {
        if let Some(result) = parse_authorized_keys_line(line) {
            match result {
                Ok((pk, options)) if !options.is_empty() => {
                    entries.push(CertAuthorityEntry {
                        public_key: pk,
                        options,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ED25519_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01QAAAJiQ+NvMkPjb\nzAAAAAtzc2gtZWQyNTUxOQAAACBOfInDyRS33JEeDNT8xd10qRdwFN8z/QukCOgEIkv01Q\nAAAECIWwJf7+7MOuZAOOWmoQbE9i/5GxjKsFrtJHjZ34E/fk58icPJFLfckR4M1PzF3XSp\nF3AU3zP9C6QI6AQiS/TVAAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const ED25519_PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV ubuntu@ns528096";

    const PEM_PRIVATE_KEY: &[u8] = b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEINTuctv5E1hK1bbY8fdp+K06/nwoy/HU++CXqI9EdVhC\n-----END PRIVATE KEY-----\n";

    fn make_authorized_keys(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{content}").unwrap();
        f.flush().unwrap();
        f
    }

    fn make_private_key_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn load_ed25519_key_from_file() {
        let f = make_private_key_file(ED25519_PRIVATE_KEY);
        let source = KeySource::File(f.path().to_path_buf());
        let key = load_private_key(source).unwrap();
        assert_eq!(key.algorithm(), russh::keys::Algorithm::Ed25519);
    }

    #[test]
    fn load_ed25519_key_from_memory() {
        let source = KeySource::Memory(ED25519_PRIVATE_KEY.as_bytes().to_vec());
        let key = load_private_key(source).unwrap();
        assert_eq!(key.algorithm(), russh::keys::Algorithm::Ed25519);
    }

    #[test]
    fn load_key_file_not_found() {
        let source = KeySource::File(PathBuf::from("/nonexistent/key"));
        let result = load_private_key(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::KeyFileNotFound { .. }));
        assert!(err.to_string().contains("/nonexistent/key"));
    }

    #[test]
    fn reject_pem_format() {
        let source = KeySource::Memory(PEM_PRIVATE_KEY.to_vec());
        let result = load_private_key(source);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::InvalidFlag { .. }));
        assert!(err.to_string().contains("PEM"));
    }

    const ED25519_PUBLIC_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/85O/pkbUFZ6OGIt49PX3nw8iRoXE other@host";

    #[test]
    fn parse_authorized_keys_multiple_entries() {
        let content = format!(
            "{ED25519_PUBLIC_KEY}\n# comment line\n\n{ED25519_PUBLIC_KEY_2}\n"
        );
        let f = make_authorized_keys(&content);
        let source = KeySource::File(f.path().to_path_buf());
        let keys = load_public_keys(source).unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn parse_authorized_keys_from_memory() {
        let content = format!("{ED25519_PUBLIC_KEY}\n");
        let source = KeySource::Memory(content.into_bytes());
        let keys = load_public_keys(source).unwrap();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn parse_cert_authority_entry() {
        let content =
            "cert-authority,permit-port-forwarding ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE58icPJFLfckR4M1PzF3XSpF3AU3zP9C6QI6AQiS/TV CA name\n";
        let f = make_authorized_keys(content);
        let source = KeySource::File(f.path().to_path_buf());
        let entries = load_cert_authority_entries(source).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].options.len(), 2);
        assert_eq!(entries[0].options[0], "cert-authority");
        assert_eq!(entries[0].options[1], "permit-port-forwarding");
    }

    #[test]
    fn parse_mixed_authorized_keys() {
        let content = format!(
            "{ED25519_PUBLIC_KEY}\ncert-authority ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/85O/pkbUFZ6OGIt49PX3nw8iRoXE CA name\n"
        );
        let source = KeySource::Memory(content.into_bytes());
        let keys = load_public_keys(source.clone()).unwrap();
        assert_eq!(keys.len(), 2);
        let entries = load_cert_authority_entries(source).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].options, vec!["cert-authority"]);
    }
}
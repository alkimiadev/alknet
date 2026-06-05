//! Server-side authentication configuration and validation.
//!
//! `ServerAuthConfig` holds the set of authorized public keys and optional certificate
//! authority entries. Authentication is key-based only (Ed25519 + optional OpenSSH CA).
//! No password authentication. See ADR-012.

use std::collections::HashSet;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::SystemTime;

use ipnetwork::IpNetwork;
use russh::keys::helpers::EncodedExt;
use russh::keys::{Certificate, PublicKey};

use super::keys::{CertAuthorityEntry, KeySource, load_cert_authority_entries, load_public_keys};
use crate::error::AuthError;

/// Server-side authentication configuration.
///
/// Holds authorized public keys (constant-time comparison) and optional certificate
/// authority entries for validating OpenSSH certificates.
#[derive(Debug, Clone)]
pub struct ServerAuthConfig {
    pub authorized_keys: HashSet<PublicKey>,
    pub cert_authorities: Vec<CertAuthorityEntry>,
    encoded_keys: HashSet<Vec<u8>>,
}

fn encode_key_data(key: &PublicKey) -> Vec<u8> {
    key.key_data().encoded().unwrap_or_default()
}

impl ServerAuthConfig {
    pub fn from_keys_and_ca(
        authorized_keys_source: Option<KeySource>,
        cert_authority_source: Option<KeySource>,
    ) -> Result<Self, crate::error::ConfigError> {
        let authorized_keys: HashSet<PublicKey> = match authorized_keys_source {
            Some(src) => load_public_keys(src)?.into_iter().collect(),
            None => HashSet::new(),
        };

        let encoded_keys: HashSet<Vec<u8>> = authorized_keys
            .iter()
            .map(encode_key_data)
            .collect();

        let cert_authorities = match cert_authority_source {
            Some(src) => load_cert_authority_entries(src)?,
            None => Vec::new(),
        };

        Ok(ServerAuthConfig {
            authorized_keys,
            cert_authorities,
            encoded_keys,
        })
    }

    pub fn authenticate_publickey(&self, key: &PublicKey) -> Result<(), AuthError> {
        let encoded = encode_key_data(key);
        if self.encoded_keys.contains(&encoded) {
            return Ok(());
        }
        Err(AuthError::KeyRejected)
    }

    pub fn authenticate_certificate(
        &self,
        cert: &Certificate,
        user: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<(), AuthError> {
        let matching_ca = self
            .cert_authorities
            .iter()
            .find(|ca| cert.signature_key() == ca.public_key.key_data());

        let ca_entry = match matching_ca {
            Some(entry) => entry,
            None => return Err(AuthError::CertInvalid),
        };

        if cert.verify_signature().is_err() {
            return Err(AuthError::CertInvalid);
        }

        let now = SystemTime::now();
        let now_secs = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if now_secs < cert.valid_after() || now_secs >= cert.valid_before() {
            return Err(AuthError::CertExpired);
        }

        let principals = cert.valid_principals();
        if !principals.is_empty() && !principals.iter().any(|p| p == user) {
            return Err(AuthError::CertPrincipalMismatch);
        }

        check_critical_options(cert, ca_entry, client_ip)?;

        check_extensions(cert, ca_entry)?;

        Ok(())
    }
}

fn check_critical_options(
    cert: &Certificate,
    ca_entry: &CertAuthorityEntry,
    client_ip: Option<IpAddr>,
) -> Result<(), AuthError> {
    let ca_has_no_pty = ca_entry.options.iter().any(|o| o == "no-pty");

    for (name, data) in cert.critical_options().iter() {
        match name.as_str() {
            "source-address" => {
                if !check_source_address(data, client_ip) {
                    return Err(AuthError::CertInvalid);
                }
            }
            "force-command" => {}
            "no-pty" => {}
            _ => {
                let _ = ca_has_no_pty;
                return Err(AuthError::CertInvalid);
            }
        }
    }

    Ok(())
}

fn check_extensions(
    cert: &Certificate,
    ca_entry: &CertAuthorityEntry,
) -> Result<(), AuthError> {
    let ca_permit_port_forwarding = ca_entry
        .options
        .iter()
        .any(|o| o == "permit-port-forwarding");

    if ca_permit_port_forwarding {
        let cert_allows = cert
            .extensions()
            .iter()
            .any(|(n, _)| n == "permit-port-forwarding");
        if !cert_allows {
            return Err(AuthError::CertInvalid);
        }
    }

    Ok(())
}

fn check_source_address(allowed: &str, client_ip: Option<IpAddr>) -> bool {
    let Some(ip) = client_ip else {
        return false;
    };

    for pattern in allowed.split(',') {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }

        if let Ok(cidr) = IpNetwork::from_str(pattern) {
            if cidr.contains(ip) {
                return true;
            }
        }

        if let Ok(net_ip) = IpAddr::from_str(pattern) {
            if net_ip == ip {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;
    use russh::keys::{Certificate, PrivateKey, decode_secret_key};
    use russh::keys::ssh_key::certificate::{Builder, CertType};
    use std::io::Write;

    const CA_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACA6pFKBI327JsRFmZULalNjpoUPJMVxzsk9bGbDByat+gAAAJjP22Bpz9tg\naQAAAAtzc2gtZWQyNTUxOQAAACA6pFKBI327JsRFmZULalNjpoUPJMVxzsk9bGbDByat+g\nAAAEBcRrWyUU+lLpjHbaaYN5YeOlvz6HnuBndUWevEmHk00jqkUoEjfbsmxEWZlQtqU2Om\nhQ8kxXHOyT1sZsMHJq36AAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const USER_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACAoTr8X7HqltuKBdBdB2Vjb+K7bi3vVPcuWAYIb3ur5NgAAAJgM/+f3DP/n\n9wAAAAtzc2gtZWQyNTUxOQAAACAoTr8X7HqltuKBdBdB2Vjb+K7bi3vVPcuWAYIb3ur5Ng\nAAAEADN/ZEFvX/mflX8aEGwS/tMzys564rYEaMzd4vmYKZkShOvxfseqW24oF0F0HZWNv4\nrtuLe9U9y5YBghve6vk2AAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    const OTHER_PRIVATE_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\nQyNTUxOQAAACC/7V2LLT4WRm1Mfje8eSPWlhN+kNXz2ryKoqCkSrGzdgAAAJgXj2UzF49l\nMwAAAAtzc2gtZWQyNTUxOQAAACC/7V2LLT4WRm1Mfje8eSPWlhN+kNXz2ryKoqCkSrGzdg\nAAAEBVadyi5nAUfkjpp4zyQ08b8h1o4RTEgwtLejTjX5Tycb/tXYstPhZGbUx+N7x5I9aW\nE36Q1fPavIqioKRKsbN2AAAAD3VidW50dUBuczUyODA5NgECAwQFBg==\n-----END OPENSSH PRIVATE KEY-----\n";

    fn load_ca_key() -> PrivateKey {
        decode_secret_key(CA_PRIVATE_KEY, None).unwrap()
    }

    fn load_user_key() -> PrivateKey {
        decode_secret_key(USER_PRIVATE_KEY, None).unwrap()
    }

    fn load_other_key() -> PrivateKey {
        decode_secret_key(OTHER_PRIVATE_KEY, None).unwrap()
    }

    fn make_cert(
        ca_key: &PrivateKey,
        user_pub: &PublicKey,
        valid_after: u64,
        valid_before: u64,
        principals: Vec<&str>,
    ) -> Certificate {
        let key_data: russh::keys::ssh_key::public::KeyData = user_pub.into();
        let mut builder = Builder::new_with_random_nonce(
            &mut OsRng,
            key_data,
            valid_after,
            valid_before,
        )
        .unwrap();

        builder.cert_type(CertType::User).unwrap();

        for p in principals {
            builder.valid_principal(p).unwrap();
        }

        builder.sign(ca_key).unwrap()
    }

    fn make_authorized_keys_file(keys: &[&PublicKey]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for key in keys {
            let line = format!("{}\n", key.to_openssh().unwrap());
            f.write_all(line.as_bytes()).unwrap();
        }
        f.flush().unwrap();
        f
    }

    fn make_ca_file(ca_pub: &PublicKey, options: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let opts = if options.is_empty() {
            "cert-authority".to_string()
        } else {
            format!("cert-authority,{}", options.join(","))
        };
        let line = format!(
            "{} {} CA\n",
            opts,
            ca_pub.to_openssh().unwrap()
        );
        f.write_all(line.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn valid_key_accepted() {
        let user_key = load_user_key();
        let user_pub = user_key.public_key().clone();
        let f = make_authorized_keys_file(&[&user_pub]);
        let config =
            ServerAuthConfig::from_keys_and_ca(Some(KeySource::File(f.path().to_path_buf())), None)
                .unwrap();
        assert!(config.authenticate_publickey(&user_pub).is_ok());
    }

    #[test]
    fn invalid_key_rejected() {
        let user_key = load_user_key();
        let other_key = load_other_key();
        let user_pub = user_key.public_key().clone();
        let other_pub = other_key.public_key().clone();
        let f = make_authorized_keys_file(&[&user_pub]);
        let config =
            ServerAuthConfig::from_keys_and_ca(Some(KeySource::File(f.path().to_path_buf())), None)
                .unwrap();
        assert_eq!(
            config.authenticate_publickey(&other_pub),
            Err(AuthError::KeyRejected)
        );
    }

    #[test]
    fn cert_authority_signed_cert_accepted() {
        let ca_key = load_ca_key();
        let user_key = load_user_key();
        let ca_pub = ca_key.public_key().clone();
        let user_pub = user_key.public_key().clone();
        let now = now_secs();
        let cert = make_cert(&ca_key, &user_pub, now - 60, now + 3600, vec!["testuser"]);
        let f = make_ca_file(&ca_pub, &[]);
        let config =
            ServerAuthConfig::from_keys_and_ca(None, Some(KeySource::File(f.path().to_path_buf())))
                .unwrap();
        assert!(config
            .authenticate_certificate(&cert, "testuser", None)
            .is_ok());
    }

    #[test]
    fn expired_cert_rejected() {
        let ca_key = load_ca_key();
        let user_key = load_user_key();
        let ca_pub = ca_key.public_key().clone();
        let user_pub = user_key.public_key().clone();
        let now = now_secs();
        let cert = make_cert(&ca_key, &user_pub, now - 7200, now - 3600, vec!["testuser"]);
        let f = make_ca_file(&ca_pub, &[]);
        let config =
            ServerAuthConfig::from_keys_and_ca(None, Some(KeySource::File(f.path().to_path_buf())))
                .unwrap();
        assert_eq!(
            config.authenticate_certificate(&cert, "testuser", None),
            Err(AuthError::CertExpired)
        );
    }

    #[test]
    fn wrong_principal_rejected() {
        let ca_key = load_ca_key();
        let user_key = load_user_key();
        let ca_pub = ca_key.public_key().clone();
        let user_pub = user_key.public_key().clone();
        let now = now_secs();
        let cert = make_cert(&ca_key, &user_pub, now - 60, now + 3600, vec!["alice"]);
        let f = make_ca_file(&ca_pub, &[]);
        let config =
            ServerAuthConfig::from_keys_and_ca(None, Some(KeySource::File(f.path().to_path_buf())))
                .unwrap();
        assert_eq!(
            config.authenticate_certificate(&cert, "bob", None),
            Err(AuthError::CertPrincipalMismatch)
        );
    }

    #[test]
    fn cert_wildcard_principals_accepts_any_user() {
        let ca_key = load_ca_key();
        let user_key = load_user_key();
        let ca_pub = ca_key.public_key().clone();
        let user_pub = user_key.public_key().clone();
        let now = now_secs();
        let key_data: russh::keys::ssh_key::public::KeyData = (&user_pub).into();
        let mut builder = Builder::new_with_random_nonce(
            &mut OsRng,
            key_data,
            now - 60,
            now + 3600,
        )
        .unwrap();
        builder.cert_type(CertType::User).unwrap();
        builder.all_principals_valid().unwrap();
        let cert = builder.sign(&ca_key).unwrap();

        let f = make_ca_file(&ca_pub, &[]);
        let config =
            ServerAuthConfig::from_keys_and_ca(None, Some(KeySource::File(f.path().to_path_buf())))
                .unwrap();
        assert!(config
            .authenticate_certificate(&cert, "anyuser", None)
            .is_ok());
    }

    #[test]
    fn cert_wrong_ca_rejected() {
        let user_key = load_user_key();
        let other_ca_key = load_other_key();
        let user_pub = user_key.public_key().clone();
        let now = now_secs();
        let cert = make_cert(&other_ca_key, &user_pub, now - 60, now + 3600, vec!["testuser"]);
        let ca_key = load_ca_key();
        let ca_pub = ca_key.public_key().clone();
        let f = make_ca_file(&ca_pub, &[]);
        let config =
            ServerAuthConfig::from_keys_and_ca(None, Some(KeySource::File(f.path().to_path_buf())))
                .unwrap();
        assert_eq!(
            config.authenticate_certificate(&cert, "testuser", None),
            Err(AuthError::CertInvalid)
        );
    }

    #[test]
    fn no_config_accepts_nothing() {
        let config =
            ServerAuthConfig::from_keys_and_ca(None, None).unwrap();
        let other_pub = load_other_key().public_key().clone();
        assert_eq!(
            config.authenticate_publickey(&other_pub),
            Err(AuthError::KeyRejected)
        );
    }
}
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use russh::keys::ssh_key::HashAlg;

use crate::auth::identity::Identity;
use crate::auth::ServerAuthConfig;
use crate::config::forwarding::ForwardingPolicy;

pub struct AuthPolicy {
    pub authorized_keys: std::collections::HashSet<russh::keys::PublicKey>,
    pub cert_authorities: Vec<crate::auth::keys::CertAuthorityEntry>,
    encoded_keys: std::collections::HashSet<Vec<u8>>,
    fingerprint_to_key: HashMap<String, russh::keys::PublicKey>,
}

fn encode_key_data(key: &russh::keys::PublicKey) -> Vec<u8> {
    use russh::keys::helpers::EncodedExt;
    key.key_data().encoded().unwrap_or_default()
}

impl AuthPolicy {
    pub fn new(
        authorized_keys: std::collections::HashSet<russh::keys::PublicKey>,
        cert_authorities: Vec<crate::auth::keys::CertAuthorityEntry>,
    ) -> Self {
        let encoded_keys = authorized_keys.iter().map(encode_key_data).collect();
        let fingerprint_to_key = authorized_keys
            .iter()
            .map(|k| (format!("{}", k.fingerprint(HashAlg::Sha256)), k.clone()))
            .collect();

        Self {
            authorized_keys,
            cert_authorities,
            encoded_keys,
            fingerprint_to_key,
        }
    }

    pub fn from_server_auth_config(config: ServerAuthConfig) -> Self {
        Self::new(config.authorized_keys, config.cert_authorities)
    }

    pub fn empty() -> Self {
        Self::new(std::collections::HashSet::new(), Vec::new())
    }

    pub fn resolve_identity_from_fingerprint(&self, fingerprint: &str) -> Option<Identity> {
        if self.fingerprint_to_key.contains_key(fingerprint) {
            Some(Identity {
                id: fingerprint.to_string(),
                scopes: vec!["relay:connect".to_string()],
                resources: HashMap::new(),
            })
        } else {
            None
        }
    }

    pub fn authenticate_publickey(
        &self,
        key: &russh::keys::PublicKey,
    ) -> Result<(), crate::error::AuthError> {
        let encoded = encode_key_data(key);
        if self.encoded_keys.contains(&encoded) {
            return Ok(());
        }
        Err(crate::error::AuthError::KeyRejected)
    }

    pub fn authenticate_certificate(
        &self,
        cert: &russh::keys::Certificate,
        user: &str,
        client_ip: Option<std::net::IpAddr>,
    ) -> Result<(), crate::error::AuthError> {
        use std::time::SystemTime;

        let matching_ca = self
            .cert_authorities
            .iter()
            .find(|ca| cert.signature_key() == ca.public_key.key_data());

        let ca_entry = match matching_ca {
            Some(entry) => entry,
            None => return Err(crate::error::AuthError::CertInvalid),
        };

        if cert.verify_signature().is_err() {
            return Err(crate::error::AuthError::CertInvalid);
        }

        let now = SystemTime::now();
        let now_secs = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if now_secs < cert.valid_after() || now_secs >= cert.valid_before() {
            return Err(crate::error::AuthError::CertExpired);
        }

        let principals = cert.valid_principals();
        if !principals.is_empty() && !principals.iter().any(|p| p == user) {
            return Err(crate::error::AuthError::CertPrincipalMismatch);
        }

        check_critical_options(cert, ca_entry, client_ip)?;
        check_extensions(cert, ca_entry)?;

        Ok(())
    }
}

fn check_critical_options(
    cert: &russh::keys::Certificate,
    ca_entry: &crate::auth::keys::CertAuthorityEntry,
    client_ip: Option<std::net::IpAddr>,
) -> Result<(), crate::error::AuthError> {
    let ca_has_no_pty = ca_entry.options.iter().any(|o| o == "no-pty");

    for (name, data) in cert.critical_options().iter() {
        match name.as_str() {
            "source-address" => {
                if !check_source_address(data, client_ip) {
                    return Err(crate::error::AuthError::CertInvalid);
                }
            }
            "force-command" => {}
            "no-pty" => {}
            _ => {
                let _ = ca_has_no_pty;
                return Err(crate::error::AuthError::CertInvalid);
            }
        }
    }

    Ok(())
}

fn check_extensions(
    cert: &russh::keys::Certificate,
    ca_entry: &crate::auth::keys::CertAuthorityEntry,
) -> Result<(), crate::error::AuthError> {
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
            return Err(crate::error::AuthError::CertInvalid);
        }
    }

    Ok(())
}

fn check_source_address(allowed: &str, client_ip: Option<std::net::IpAddr>) -> bool {
    use ipnetwork::IpNetwork;
    use std::net::IpAddr;
    use std::str::FromStr;

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

impl std::fmt::Debug for AuthPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthPolicy")
            .field("authorized_keys_count", &self.authorized_keys.len())
            .field("cert_authorities_count", &self.cert_authorities.len())
            .finish()
    }
}

impl Clone for AuthPolicy {
    fn clone(&self) -> Self {
        Self {
            authorized_keys: self.authorized_keys.clone(),
            cert_authorities: self.cert_authorities.clone(),
            encoded_keys: self.encoded_keys.clone(),
            fingerprint_to_key: self.fingerprint_to_key.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub max_connections_per_ip: usize,
    pub max_auth_attempts: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_connections_per_ip: 0,
            max_auth_attempts: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub forwarding: ForwardingPolicy,
    pub rate_limits: RateLimitConfig,
}

impl DynamicConfig {
    pub fn new(auth: AuthPolicy) -> Self {
        Self {
            auth,
            forwarding: ForwardingPolicy::allow_all(),
            rate_limits: RateLimitConfig::default(),
        }
    }

    pub fn with_forwarding_policy(mut self, policy: ForwardingPolicy) -> Self {
        self.forwarding = policy;
        self
    }

    pub fn with_rate_limits(mut self, limits: RateLimitConfig) -> Self {
        self.rate_limits = limits;
        self
    }
}

impl Default for DynamicConfig {
    fn default() -> Self {
        Self {
            auth: AuthPolicy::empty(),
            forwarding: ForwardingPolicy::allow_all(),
            rate_limits: RateLimitConfig::default(),
        }
    }
}

pub struct ConfigReloadHandle {
    pub(crate) dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn reload(&self, new_config: DynamicConfig) {
        self.dynamic.store(Arc::new(new_config));
    }

    pub fn dynamic(&self) -> Arc<DynamicConfig> {
        self.dynamic.load_full()
    }

    pub fn dynamic_arc(&self) -> Arc<ArcSwap<DynamicConfig>> {
        Arc::clone(&self.dynamic)
    }
}

impl std::fmt::Debug for ConfigReloadHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigReloadHandle").finish()
    }
}

pub fn new_dynamic_config() -> (Arc<ArcSwap<DynamicConfig>>, ConfigReloadHandle) {
    let inner = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
    let handle = ConfigReloadHandle {
        dynamic: Arc::clone(&inner),
    };
    (inner, handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::forwarding::ForwardingAction;

    #[test]
    fn forwarding_policy_allow_all_default() {
        let policy = ForwardingPolicy::allow_all();
        assert_eq!(policy.default, ForwardingAction::Allow);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn forwarding_policy_deny_all() {
        let policy = ForwardingPolicy::deny_all();
        assert_eq!(policy.default, ForwardingAction::Deny);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn dynamic_config_default() {
        let config = DynamicConfig::default();
        assert_eq!(config.forwarding.default, ForwardingAction::Allow);
        assert_eq!(config.rate_limits.max_connections_per_ip, 0);
        assert_eq!(config.rate_limits.max_auth_attempts, 10);
    }

    #[test]
    fn config_reload_handle_updates_dynamic() {
        let (arc_swap, handle) = new_dynamic_config();
        let initial = arc_swap.load();
        assert_eq!(initial.forwarding.default, ForwardingAction::Allow);

        let new_config = DynamicConfig {
            auth: AuthPolicy::empty(),
            forwarding: ForwardingPolicy::deny_all(),
            rate_limits: RateLimitConfig::default(),
        };
        handle.reload(new_config);

        let updated = arc_swap.load();
        assert_eq!(updated.forwarding.default, ForwardingAction::Deny);
    }

    #[test]
    fn dynamic_config_with_forwarding_policy_builder() {
        let config = DynamicConfig::new(AuthPolicy::empty())
            .with_forwarding_policy(ForwardingPolicy::deny_all());
        assert_eq!(config.forwarding.default, ForwardingAction::Deny);
    }

    #[test]
    fn rate_limit_config_custom() {
        let limits = RateLimitConfig {
            max_connections_per_ip: 5,
            max_auth_attempts: 3,
        };
        assert_eq!(limits.max_connections_per_ip, 5);
        assert_eq!(limits.max_auth_attempts, 3);
    }

    #[test]
    fn forwarding_action_equality() {
        assert_eq!(ForwardingAction::Allow, ForwardingAction::Allow);
        assert_eq!(ForwardingAction::Deny, ForwardingAction::Deny);
        assert_ne!(ForwardingAction::Allow, ForwardingAction::Deny);
    }

    #[test]
    fn auth_policy_empty_rejects_all() {
        let policy = AuthPolicy::empty();
        let key_text = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHeLC1lWiCYrXsf/85O/pkbUFZ6OGIt49PX3nw8iRoXE other@host";
        let other_ssh_key =
            russh::keys::parse_public_key_base64(key_text.split_whitespace().nth(1).unwrap())
                .unwrap();
        assert_eq!(
            policy.authenticate_publickey(&other_ssh_key),
            Err(crate::error::AuthError::KeyRejected)
        );
    }

    #[test]
    fn auth_policy_debug_redacts_keys() {
        let policy = AuthPolicy::empty();
        let debug_str = format!("{:?}", policy);
        assert!(debug_str.contains("authorized_keys_count"));
        assert!(debug_str.contains("cert_authorities_count"));
    }
}

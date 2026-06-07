use std::sync::Arc;

use arc_swap::ArcSwap;
use russh::keys::PrivateKey;

use crate::auth::IdentityProvider;
use crate::config::DynamicConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceKind {
    Ssh,
    RawFraming,
}

impl std::fmt::Display for InterfaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterfaceKind::Ssh => write!(f, "ssh"),
            InterfaceKind::RawFraming => write!(f, "raw-framing"),
        }
    }
}

pub enum InterfaceConfig {
    Ssh(SshInterfaceConfig),
    RawFraming(RawFramingConfig),
}

impl InterfaceConfig {
    pub fn kind(&self) -> InterfaceKind {
        match self {
            InterfaceConfig::Ssh(_) => InterfaceKind::Ssh,
            InterfaceConfig::RawFraming(_) => InterfaceKind::RawFraming,
        }
    }
}

pub struct SshInterfaceConfig {
    pub auth: Arc<dyn IdentityProvider>,
    pub forwarding: Arc<ArcSwap<DynamicConfig>>,
    pub host_key: Arc<PrivateKey>,
}

pub struct RawFramingConfig {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_kind_display() {
        assert_eq!(InterfaceKind::Ssh.to_string(), "ssh");
        assert_eq!(InterfaceKind::RawFraming.to_string(), "raw-framing");
    }

    #[test]
    fn interface_kind_from_config() {
        let auth = Arc::new(crate::auth::ConfigIdentityProvider::new(Arc::new(
            ArcSwap::new(Arc::new(DynamicConfig::default())),
        )));
        let ssh_config = InterfaceConfig::Ssh(SshInterfaceConfig {
            auth,
            forwarding: Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default()))),
            host_key: Arc::new(
                russh::keys::PrivateKey::random(
                    &mut rand_core::OsRng,
                    russh::keys::Algorithm::Ed25519,
                )
                .unwrap(),
            ),
        });
        assert_eq!(ssh_config.kind(), InterfaceKind::Ssh);

        let raw_config = InterfaceConfig::RawFraming(RawFramingConfig {});
        assert_eq!(raw_config.kind(), InterfaceKind::RawFraming);
    }

    #[test]
    fn interface_kind_equality() {
        assert_eq!(InterfaceKind::Ssh, InterfaceKind::Ssh);
        assert_eq!(InterfaceKind::RawFraming, InterfaceKind::RawFraming);
        assert_ne!(InterfaceKind::Ssh, InterfaceKind::RawFraming);
    }

    #[test]
    fn raw_framing_config_minimal() {
        let _config = RawFramingConfig {};
    }
}

//! Configuration service for runtime config reload.
//!
//! See [ADR-030](docs/architecture/decisions/030-dynamic-config.md).

use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{DynamicConfig, ForwardingPolicy, RateLimitConfig};

pub struct ConfigServiceImpl {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigServiceImpl {
    pub fn new(dynamic: Arc<ArcSwap<DynamicConfig>>) -> Self {
        Self { dynamic }
    }

    pub fn forwarding_policy(&self) -> Arc<ForwardingPolicy> {
        Arc::new(self.dynamic.load().forwarding.clone())
    }

    pub fn rate_limits(&self) -> Arc<RateLimitConfig> {
        Arc::new(self.dynamic.load().rate_limits.clone())
    }

    pub fn reload(&self, new_config: DynamicConfig) {
        self.dynamic.store(Arc::new(new_config));
    }
}

impl std::fmt::Debug for ConfigServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigServiceImpl").finish()
    }
}

#[cfg(feature = "irpc")]
#[allow(dead_code)]
pub enum ConfigProtocol {
    GetForwardingPolicy,
    GetRateLimits,
    ReloadForwarding { policy: ForwardingPolicy },
    ReloadRateLimits { limits: RateLimitConfig },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthPolicy;

    #[test]
    fn config_service_impl_forwarding_policy() {
        let (arc_swap, _) = super::super::new_dynamic_config();
        let service = ConfigServiceImpl::new(Arc::clone(&arc_swap));
        let policy = service.forwarding_policy();
        assert_eq!(policy.default, ForwardingPolicy::allow_all().default);
    }

    #[test]
    fn config_service_impl_rate_limits() {
        let (arc_swap, _) = super::super::new_dynamic_config();
        let service = ConfigServiceImpl::new(Arc::clone(&arc_swap));
        let limits = service.rate_limits();
        assert_eq!(limits.max_auth_attempts, 10);
    }

    #[test]
    fn config_service_impl_reload() {
        let (arc_swap, _) = super::super::new_dynamic_config();
        let service = ConfigServiceImpl::new(Arc::clone(&arc_swap));
        assert_eq!(
            service.forwarding_policy().default,
            ForwardingPolicy::allow_all().default
        );

        let new_config = DynamicConfig {
            auth: AuthPolicy::empty(),
            forwarding: ForwardingPolicy::deny_all(),
            rate_limits: RateLimitConfig::default(),
            credentials: std::collections::HashMap::new(),
        };
        service.reload(new_config);

        assert_eq!(
            service.forwarding_policy().default,
            ForwardingPolicy::deny_all().default
        );
    }

    #[test]
    fn config_service_impl_debug() {
        let (arc_swap, _) = super::super::new_dynamic_config();
        let service = ConfigServiceImpl::new(Arc::clone(&arc_swap));
        let debug_str = format!("{:?}", service);
        assert!(debug_str.contains("ConfigServiceImpl"));
    }
}

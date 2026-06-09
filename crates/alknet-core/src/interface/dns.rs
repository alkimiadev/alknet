use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::call::OperationEnv;
use crate::interface::{InterfaceRequest, InterfaceResponse, MessageInterface};

pub struct DnsInterface {
    pub domain: String,
    pub identity_provider: Arc<dyn crate::auth::IdentityProvider>,
    pub registry: Arc<crate::call::OperationRegistry>,
    pub env: OperationEnv,
}

#[async_trait]
impl MessageInterface for DnsInterface {
    async fn handle_request(&self, _request: InterfaceRequest) -> Result<InterfaceResponse> {
        Ok(InterfaceResponse {
            result: Err(crate::call::CallError::new(
                "NOT_IMPLEMENTED",
                "DnsInterface is not yet implemented",
                false,
            )),
            status: 501,
            headers: std::collections::HashMap::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_interface_type_exists() {
        let registry = Arc::new(crate::call::OperationRegistry::new());
        let _iface = DnsInterface {
            domain: "alk.dev".to_string(),
            identity_provider: Arc::new(crate::auth::ConfigIdentityProvider::new(Arc::new(
                arc_swap::ArcSwap::new(Arc::new(crate::config::DynamicConfig::default())),
            ))),
            env: OperationEnv::local(crate::call::OperationRegistry::new()),
            registry,
        };
    }
}

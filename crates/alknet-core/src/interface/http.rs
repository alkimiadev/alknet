use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::call::OperationEnv;
use crate::interface::{InterfaceRequest, InterfaceResponse, MessageInterface};

pub struct HttpInterface {
    pub identity_provider: Arc<dyn crate::auth::IdentityProvider>,
    pub registry: Arc<crate::call::OperationRegistry>,
    pub env: OperationEnv,
}

#[async_trait]
impl MessageInterface for HttpInterface {
    async fn handle_request(&self, _request: InterfaceRequest) -> Result<InterfaceResponse> {
        Ok(InterfaceResponse {
            result: Err(crate::call::CallError::new(
                "NOT_IMPLEMENTED",
                "HttpInterface is not yet implemented",
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
    fn http_interface_type_exists() {
        let registry = Arc::new(crate::call::OperationRegistry::new());
        let _iface = HttpInterface {
            identity_provider: Arc::new(crate::auth::ConfigIdentityProvider::new(Arc::new(
                arc_swap::ArcSwap::new(Arc::new(crate::config::DynamicConfig::default())),
            ))),
            env: OperationEnv::local(crate::call::OperationRegistry::new()),
            registry,
        };
    }
}

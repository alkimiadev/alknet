use std::sync::Arc;

use serde_json::Value;

use crate::call::context::OperationContext;
use crate::call::registry::OperationRegistry;
use crate::call::response::ResponseEnvelope;
use crate::credentials::{CredentialProvider, CredentialSet, SecretStoreCredentialProvider};

#[derive(Clone)]
pub struct OperationEnv {
    registry: Arc<OperationRegistry>,
    credential_provider: Arc<dyn CredentialProvider>,
}

impl std::fmt::Debug for OperationEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationEnv")
            .field("registry", &self.registry)
            .finish()
    }
}

impl OperationEnv {
    pub fn local(registry: OperationRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
            credential_provider: Arc::new(SecretStoreCredentialProvider::new()),
        }
    }

    pub fn with_credential_provider(
        registry: OperationRegistry,
        credential_provider: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            registry: Arc::new(registry),
            credential_provider,
        }
    }

    pub fn credentials(&self, service: &str) -> Option<CredentialSet> {
        self.credential_provider.get_credentials(service)
    }

    pub fn invoke(&self, namespace: &str, operation: &str, input: Value) -> ResponseEnvelope {
        let name = format!("/{namespace}/{operation}");
        let request_id = format!("env{name}");
        let context = OperationContext {
            request_id: request_id.clone(),
            parent_request_id: None,
            identity: None,
            metadata: std::collections::HashMap::new(),
            env: self.clone(),
            trusted: true,
        };
        self.registry.invoke(&name, input, context)
    }

    pub fn registry_ref(&self) -> &OperationRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::registry::OperationRegistryBuilder;
    use crate::call::spec::{AccessControl, OperationSpec, OperationType};
    use crate::config::{AuthPolicy, DynamicConfig};
    use crate::credentials::ConfigCredentialProvider;
    use arc_swap::ArcSwap;
    use std::collections::HashMap;

    fn make_spec(name: &str, namespace: &str) -> OperationSpec {
        OperationSpec {
            name: name.to_string(),
            namespace: namespace.to_string(),
            op_type: OperationType::Query,
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            access_control: AccessControl {
                required_scopes: vec![],
                required_scopes_any: None,
                resource_type: None,
                resource_action: None,
            },
        }
    }

    #[test]
    fn operation_env_local_invoke() {
        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("/auth/verify", "auth"),
                Arc::new(|_input, _ctx| {
                    ResponseEnvelope::ok("env-/auth/verify", serde_json::json!({"verified": true}))
                }),
            )
            .build();

        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!({"token": "abc"}));
        assert!(result.result.is_ok());
    }

    #[test]
    fn operation_env_invoke_missing() {
        let registry = OperationRegistry::new();
        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!(null));
        assert!(result.result.is_err());
        let err = result.result.unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    #[test]
    fn operation_env_invoke_trusted() {
        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("/auth/verify", "auth"),
                Arc::new(|_input, ctx| {
                    assert!(ctx.trusted);
                    ResponseEnvelope::ok(&ctx.request_id, serde_json::json!({"ok": true}))
                }),
            )
            .build();

        let env = OperationEnv::local(registry);
        let result = env.invoke("auth", "verify", serde_json::json!(null));
        assert!(result.result.is_ok());
    }

    #[test]
    fn operation_env_provides_credentials_from_handler_context() {
        let mut credentials = HashMap::new();
        credentials.insert(
            "vast-ai".to_string(),
            CredentialSet::Bearer {
                token: "test-token".to_string(),
            },
        );
        let config = DynamicConfig::new(AuthPolicy::empty()).with_credentials(credentials);
        let dynamic = Arc::new(ArcSwap::new(Arc::new(config)));
        let provider = Arc::new(ConfigCredentialProvider::new(dynamic));

        let registry = OperationRegistryBuilder::new()
            .with(
                make_spec("/test/creds", "test"),
                Arc::new(|_input, ctx| {
                    let creds = ctx.env.credentials("vast-ai");
                    match creds {
                        Some(CredentialSet::Bearer { token }) => ResponseEnvelope::ok(
                            &ctx.request_id,
                            serde_json::json!({"token": token}),
                        ),
                        _ => ResponseEnvelope::ok(
                            &ctx.request_id,
                            serde_json::json!({"found": false}),
                        ),
                    }
                }),
            )
            .build();

        let env = OperationEnv::with_credential_provider(registry, provider);
        let result = env.invoke("test", "creds", serde_json::json!(null));
        assert!(result.result.is_ok());
        let value = result.result.unwrap();
        assert_eq!(value["token"], "test-token");
    }

    #[test]
    fn operation_env_credentials_returns_none_for_missing_service() {
        let config = DynamicConfig::default();
        let dynamic = Arc::new(ArcSwap::new(Arc::new(config)));
        let provider = Arc::new(ConfigCredentialProvider::new(dynamic));

        let registry = OperationRegistry::new();
        let env = OperationEnv::with_credential_provider(registry, provider);
        assert!(env.credentials("nonexistent").is_none());
    }

    #[test]
    fn operation_env_default_credentials_returns_none() {
        let registry = OperationRegistry::new();
        let env = OperationEnv::local(registry);
        assert!(env.credentials("vast-ai").is_none());
    }
}

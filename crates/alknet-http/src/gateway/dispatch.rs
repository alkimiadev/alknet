//! Shared dispatch spine for the `to_openapi` / `to_mcp` gateway projections.
//!
//! Thin concrete struct (not a trait — research §5.2 rules out a trait with an
//! associated output type). Holds `Arc<OperationRegistry>` +
//! `Arc<dyn IdentityProvider>` and exposes a `resolve_bearer()` + `invoke()`
//! method pair returning the neutral `ResponseEnvelope`. Each gateway maps the
//! envelope to its own wire shape (`to_openapi` → HTTP `Response`, `to_mcp` →
//! `CallToolResult`).
//!
//! # Security invariants
//!
//! - `internal: false` — ACL runs against the caller's `identity`, not a
//!   handler's composition authority (ADR-015).
//! - `forwarded_for: None` — wire-ingress only (ADR-032).
//! - The root `OperationContext` is constructed identically for both
//!   gateways, making them provably identical on the security axis (auth,
//!   authority, ACL); they diverge only on wire-framing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alknet_call::protocol::wire::ResponseEnvelope;
use alknet_call::registry::context::{AbortPolicy, OperationContext, ScopedPeerEnv};
use alknet_call::registry::env::LocalOperationEnv;
use alknet_call::registry::registration::OperationRegistry;
use alknet_core::auth::{AuthToken, Identity, IdentityProvider};
use alknet_core::types::Capabilities;
use futures::stream::BoxStream;
use serde_json::Value;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct GatewayDispatch {
    registry: Arc<OperationRegistry>,
    identity_provider: Arc<dyn IdentityProvider>,
}

impl GatewayDispatch {
    pub fn new(
        registry: Arc<OperationRegistry>,
        identity_provider: Arc<dyn IdentityProvider>,
    ) -> Self {
        Self {
            registry,
            identity_provider,
        }
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }

    pub fn identity_provider(&self) -> &Arc<dyn IdentityProvider> {
        &self.identity_provider
    }

    pub fn resolve_bearer(&self, token: &AuthToken) -> Option<Identity> {
        self.identity_provider.resolve_from_token(token)
    }

    pub async fn invoke(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> ResponseEnvelope {
        let operation_name = strip_leading_slash(op).to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let context = self.build_root_context(&request_id, &operation_name, identity);
        self.registry.invoke(&operation_name, input, context).await
    }

    pub fn invoke_streaming(
        &self,
        identity: Option<Identity>,
        op: &str,
        input: Value,
    ) -> BoxStream<'static, ResponseEnvelope> {
        let operation_name = strip_leading_slash(op).to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let context = self.build_root_context_streaming(&request_id, &operation_name, identity);
        self.registry
            .invoke_streaming(&operation_name, input, context)
    }

    fn build_root_context(
        &self,
        request_id: &str,
        operation_name: &str,
        identity: Option<Identity>,
    ) -> OperationContext {
        self.build_root_context_inner(request_id, operation_name, identity, true)
    }

    fn build_root_context_streaming(
        &self,
        request_id: &str,
        operation_name: &str,
        identity: Option<Identity>,
    ) -> OperationContext {
        self.build_root_context_inner(request_id, operation_name, identity, false)
    }

    fn build_root_context_inner(
        &self,
        request_id: &str,
        operation_name: &str,
        identity: Option<Identity>,
        bounded: bool,
    ) -> OperationContext {
        let registration = self.registry.registration(operation_name);
        let (composition_authority, capabilities, scoped_env) = match registration {
            Some(r) => (
                r.composition_authority.clone(),
                r.capabilities.clone(),
                r.scoped_env.clone().unwrap_or_else(ScopedPeerEnv::empty),
            ),
            None => (None, Capabilities::new(), ScopedPeerEnv::empty()),
        };

        let env: Arc<dyn alknet_call::registry::env::OperationEnv + Send + Sync> =
            Arc::new(LocalOperationEnv::new(Arc::clone(&self.registry)));

        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity: composition_authority,
            forwarded_for: None,
            capabilities,
            metadata: HashMap::new(),
            deadline: bounded.then(|| Instant::now() + DEFAULT_TIMEOUT),
            scoped_env,
            env,
            abort_policy: AbortPolicy::default(),
            internal: false,
        }
    }
}

fn strip_leading_slash(operation_id: &str) -> &str {
    operation_id.strip_prefix('/').unwrap_or(operation_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alknet_call::registry::discovery::{
        services_list_handler, services_list_spec, services_schema_handler, services_schema_spec,
    };
    use alknet_call::registry::registration::{
        make_handler, make_streaming_handler, HandlerKind, HandlerRegistration, OperationProvenance,
    };
    use alknet_call::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::AuthToken;
    use futures::stream::StreamExt;
    use std::sync::Mutex as StdMutex;

    struct StaticIdentityProvider {
        tokens: StdMutex<HashMap<String, Identity>>,
    }

    impl StaticIdentityProvider {
        fn new() -> Self {
            Self {
                tokens: StdMutex::new(HashMap::new()),
            }
        }

        fn with_token(self, token: &str, identity: Identity) -> Self {
            self.tokens
                .lock()
                .unwrap()
                .insert(token.to_string(), identity);
            self
        }
    }

    impl IdentityProvider for StaticIdentityProvider {
        fn resolve_from_fingerprint(&self, _fp: &str) -> Option<Identity> {
            None
        }
        fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity> {
            let token_str = String::from_utf8_lossy(&token.raw);
            self.tokens.lock().unwrap().get(token_str.as_ref()).cloned()
        }
    }

    fn identity_with_scopes(id: &str, scopes: &[&str]) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            resources: HashMap::new(),
        }
    }

    fn external_spec(name: &str, acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
        )
    }

    fn internal_spec(name: &str, acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::Internal,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
        )
    }

    fn registry_with(name: &str, visibility: Visibility, acl: AccessControl) -> OperationRegistry {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                OperationSpec::new(
                    name,
                    OperationType::Query,
                    visibility,
                    serde_json::json!({}),
                    serde_json::json!({}),
                    vec![],
                    acl,
                ),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        registry
    }

    fn registry_with_discovery(inner: Arc<OperationRegistry>) -> OperationRegistry {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                services_list_spec(),
                HandlerKind::Once(services_list_handler(Arc::clone(&inner))),
                OperationProvenance::Local,
                None,
                ScopedPeerEnv::empty().into(),
                Capabilities::new(),
            ))
            .unwrap();
        registry
            .register(HandlerRegistration::new(
                services_schema_spec(),
                HandlerKind::Once(services_schema_handler(Arc::clone(&inner))),
                OperationProvenance::Local,
                None,
                ScopedPeerEnv::empty().into(),
                Capabilities::new(),
            ))
            .unwrap();
        registry
    }

    fn subscription_spec(name: &str, visibility: Visibility, acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Subscription,
            visibility,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
        )
    }

    fn echo_streaming_handler() -> HandlerKind {
        HandlerKind::Stream(make_streaming_handler(|input, context| {
            futures::stream::iter(vec![ResponseEnvelope::ok(context.request_id, input)])
        }))
    }

    fn registry_with_subscription(
        name: &str,
        visibility: Visibility,
        acl: AccessControl,
    ) -> OperationRegistry {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                subscription_spec(name, visibility, acl),
                echo_streaming_handler(),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        registry
    }

    async fn collect_stream(
        mut stream: BoxStream<'static, ResponseEnvelope>,
    ) -> Vec<ResponseEnvelope> {
        let mut out = Vec::new();
        while let Some(env) = stream.next().await {
            out.push(env);
        }
        out
    }

    fn dispatch(
        registry: Arc<OperationRegistry>,
        provider: Arc<dyn IdentityProvider>,
    ) -> GatewayDispatch {
        GatewayDispatch::new(registry, provider)
    }

    #[tokio::test]
    async fn invoke_dispatches_registered_op_and_returns_response_envelope() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp
            .invoke(None, "echo/run", serde_json::json!({ "msg": "hi" }))
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "msg": "hi" }));
    }

    #[tokio::test]
    async fn invoke_strips_leading_slash_from_operation_name() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp.invoke(None, "/echo/run", serde_json::json!({})).await;
        assert!(response.result.is_ok());
    }

    #[tokio::test]
    async fn invoke_for_services_list_returns_access_control_filtered_list() {
        let mut inner = OperationRegistry::new();
        inner
            .register(HandlerRegistration::new(
                external_spec("public/echo", AccessControl::default()),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        inner
            .register(HandlerRegistration::new(
                external_spec(
                    "admin/secret",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let inner = Arc::new(inner);
        let discovery = Arc::new(registry_with_discovery(Arc::clone(&inner)));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(discovery, provider);

        let response = dp
            .invoke(
                Some(identity_with_scopes("regular-peer", &["user"])),
                "services/list",
                serde_json::json!({}),
            )
            .await;
        let output = response.result.expect("list ok");
        let ops = output
            .get("operations")
            .and_then(|v| v.as_array())
            .expect("operations array");
        let names: Vec<&str> = ops
            .iter()
            .filter_map(|o| o.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"public/echo"));
        assert!(
            !names.contains(&"admin/secret"),
            "unauthorized peer must not see admin op via services/list"
        );
    }

    #[tokio::test]
    async fn invoke_for_services_schema_returns_spec_for_known_op() {
        let mut inner = OperationRegistry::new();
        inner
            .register(HandlerRegistration::new(
                external_spec("fs/readFile", AccessControl::default()),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let inner = Arc::new(inner);
        let discovery = Arc::new(registry_with_discovery(Arc::clone(&inner)));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(discovery, provider);

        let response = dp
            .invoke(
                None,
                "services/schema",
                serde_json::json!({ "name": "fs/readFile" }),
            )
            .await;
        let spec = response.result.expect("schema ok");
        assert_eq!(spec.get("name"), Some(&serde_json::json!("fs/readFile")));
        assert_eq!(spec.get("namespace"), Some(&serde_json::json!("fs")));
    }

    #[tokio::test]
    async fn invoke_for_unregistered_op_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp.invoke(None, "no/such", serde_json::json!({})).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("no/such"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_for_internal_op_returns_not_found_not_leaked() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                internal_spec("secret/op", AccessControl::default()),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp.invoke(None, "secret/op", serde_json::json!({})).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("secret/op"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_with_none_identity_and_restricted_op_returns_forbidden() {
        let registry = Arc::new(registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp.invoke(None, "admin/run", serde_json::json!({})).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_with_authorized_identity_dispatches_restricted_op() {
        let registry = Arc::new(registry_with(
            "admin/run",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(
            StaticIdentityProvider::new()
                .with_token("alk_admin", identity_with_scopes("admin-peer", &["admin"])),
        );
        let dp = dispatch(registry, provider);

        let token = AuthToken {
            raw: b"alk_admin".to_vec(),
        };
        let identity = dp.resolve_bearer(&token).expect("token resolves");
        let response = dp
            .invoke(Some(identity), "admin/run", serde_json::json!({ "ok": 1 }))
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({ "ok": 1 }));
    }

    #[tokio::test]
    async fn resolve_bearer_returns_none_for_unknown_token() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let token = AuthToken {
            raw: b"alk_unknown".to_vec(),
        };
        assert!(dp.resolve_bearer(&token).is_none());
    }

    #[test]
    fn build_root_context_sets_internal_false_and_forwarded_for_none() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let ctx = dp.build_root_context("req-ctx-1", "echo/run", None);
        assert!(!ctx.internal, "internal must be false for wire-ingress");
        assert!(ctx.forwarded_for.is_none(), "forwarded_for must be None");
        assert!(ctx.parent_request_id.is_none(), "root has no parent");
        assert!(ctx.deadline.is_some(), "deadline is set");
        assert!(ctx.request_id != "req-ctx-1" || uuid::Uuid::parse_str("req-ctx-1").is_err());
    }

    #[test]
    fn build_root_context_for_unknown_op_uses_empty_capabilities_and_scoped_env() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let ctx = dp.build_root_context("req-ctx-2", "no/such", None);
        assert!(ctx.handler_identity.is_none());
        assert!(ctx.scoped_env.allowed_ops.is_empty());
        assert!(ctx.scoped_env.peer_pinned.is_empty());
    }

    #[test]
    fn build_root_context_carries_registration_bundle_fields() {
        let authority = alknet_call::registry::context::CompositionAuthority::new(
            "agent",
            ["fs:read".to_string()],
        );
        let scoped = ScopedPeerEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());

        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("agent/run", AccessControl::default()),
                HandlerKind::Once(make_handler(|input, context| async move {
                    ResponseEnvelope::ok(context.request_id, input)
                })),
                OperationProvenance::Local,
                Some(authority),
                Some(scoped.clone()),
                caps,
            ))
            .unwrap();
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let ctx = dp.build_root_context("req-ctx-3", "agent/run", None);
        assert!(ctx.handler_identity.is_some());
        assert_eq!(ctx.handler_identity.as_ref().unwrap().label, "agent");
        assert!(ctx.scoped_env.allows("fs/readFile"));
        assert!(ctx.capabilities.get("google").is_some());
    }

    #[test]
    fn strip_leading_slash_helper_works() {
        assert_eq!(strip_leading_slash("/fs/readFile"), "fs/readFile");
        assert_eq!(strip_leading_slash("fs/readFile"), "fs/readFile");
        assert_eq!(strip_leading_slash(""), "");
    }

    #[test]
    fn gateway_dispatch_is_concrete_struct_not_trait() {
        fn assert_concrete<T: Sized>() {}
        assert_concrete::<GatewayDispatch>();
    }

    #[tokio::test]
    async fn invoke_streaming_on_subscription_returns_handler_stream() {
        let registry = Arc::new(registry_with_subscription(
            "events/stream",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "events/stream", serde_json::json!({ "v": 7 }));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].result, Ok(serde_json::json!({ "v": 7 })));
    }

    #[tokio::test]
    async fn invoke_streaming_strips_leading_slash_from_operation_name() {
        let registry = Arc::new(registry_with_subscription(
            "events/stream",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "/events/stream", serde_json::json!({}));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        assert!(items[0].result.is_ok());
    }

    #[tokio::test]
    async fn invoke_streaming_on_unknown_op_yields_single_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "no/such", serde_json::json!({}));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("no/such"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_on_internal_op_from_external_yields_not_found() {
        let registry = Arc::new(registry_with_subscription(
            "secret/stream",
            Visibility::Internal,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "secret/stream", serde_json::json!({}));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("secret/stream"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_with_none_identity_and_restricted_op_yields_forbidden() {
        let registry = Arc::new(registry_with_subscription(
            "admin/stream",
            Visibility::External,
            AccessControl {
                required_scopes: vec!["admin".to_string()],
                ..Default::default()
            },
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "admin/stream", serde_json::json!({}));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_on_query_op_yields_invalid_operation_type() {
        let registry = Arc::new(registry_with(
            "echo/run",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let stream = dp.invoke_streaming(None, "echo/run", serde_json::json!({}));
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => assert_eq!(e.code, "INVALID_OPERATION_TYPE"),
            other => panic!("expected INVALID_OPERATION_TYPE, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_on_subscription_op_returns_invalid_operation_type() {
        let registry = Arc::new(registry_with_subscription(
            "events/stream",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let response = dp
            .invoke(None, "events/stream", serde_json::json!({}))
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "INVALID_OPERATION_TYPE"),
            other => panic!("expected INVALID_OPERATION_TYPE, got {other:?}"),
        }
    }

    #[test]
    fn build_root_context_streaming_sets_deadline_none() {
        let registry = Arc::new(registry_with_subscription(
            "events/stream",
            Visibility::External,
            AccessControl::default(),
        ));
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let ctx = dp.build_root_context_streaming("req-st-1", "events/stream", None);
        assert!(!ctx.internal, "internal must be false for wire-ingress");
        assert!(ctx.forwarded_for.is_none(), "forwarded_for must be None");
        assert!(ctx.parent_request_id.is_none(), "root has no parent");
        assert!(
            ctx.deadline.is_none(),
            "deadline must be None for streaming"
        );
    }

    #[test]
    fn build_root_context_streaming_carries_registration_bundle_fields() {
        let authority = alknet_call::registry::context::CompositionAuthority::new(
            "agent",
            ["fs:read".to_string()],
        );
        let scoped = ScopedPeerEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());

        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                subscription_spec(
                    "agent/stream",
                    Visibility::External,
                    AccessControl::default(),
                ),
                echo_streaming_handler(),
                OperationProvenance::Local,
                Some(authority),
                Some(scoped.clone()),
                caps,
            ))
            .unwrap();
        let registry = Arc::new(registry);
        let provider: Arc<dyn IdentityProvider> = Arc::new(StaticIdentityProvider::new());
        let dp = dispatch(registry, provider);

        let ctx = dp.build_root_context_streaming("req-st-2", "agent/stream", None);
        assert!(ctx.handler_identity.is_some());
        assert_eq!(ctx.handler_identity.as_ref().unwrap().label, "agent");
        assert!(ctx.scoped_env.allows("fs/readFile"));
        assert!(ctx.capabilities.get("google").is_some());
        assert!(ctx.deadline.is_none());
    }
}

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use alknet_core::types::Capabilities;
use futures::stream::{self, Stream};
use serde_json::Value;

use super::context::{CompositionAuthority, OperationContext, ScopedPeerEnv};
use super::spec::{AccessResult, OperationSpec, OperationType, Visibility};
use crate::protocol::wire::{CallError, ResponseEnvelope};

pub type Handler = Arc<
    dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>>
        + Send
        + Sync,
>;

pub type StreamingHandler = Arc<
    dyn Fn(Value, OperationContext) -> Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>
        + Send
        + Sync,
>;

pub type ResponseStream = Pin<Box<dyn Stream<Item = ResponseEnvelope> + Send>>;

#[derive(Clone)]
pub enum HandlerKind {
    Once(Handler),
    Stream(StreamingHandler),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationProvenance {
    Local,
    FromOpenAPI,
    FromMCP,
    FromCall,
    FromJsonSchema,
    Session,
}

pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: HandlerKind,
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>,
    pub scoped_env: Option<ScopedPeerEnv>,
    pub capabilities: Capabilities,
}

impl HandlerRegistration {
    pub fn new(
        spec: OperationSpec,
        handler: HandlerKind,
        provenance: OperationProvenance,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedPeerEnv>,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            spec,
            handler,
            provenance,
            composition_authority,
            scoped_env,
            capabilities,
        }
    }
}

pub struct OperationRegistry {
    operations: HashMap<String, HandlerRegistration>,
}

impl OperationRegistry {
    pub fn new() -> Self {
        Self {
            operations: HashMap::new(),
        }
    }

    pub fn register(&mut self, registration: HandlerRegistration) -> Result<(), String> {
        let expected = match registration.spec.op_type {
            OperationType::Query | OperationType::Mutation => "Once",
            OperationType::Subscription => "Stream",
        };
        let actual = match registration.handler {
            HandlerKind::Once(_) => "Once",
            HandlerKind::Stream(_) => "Stream",
        };
        if expected != actual {
            return Err(format!(
                "handler kind mismatch: {:?} requires HandlerKind::{} (got HandlerKind::{})",
                registration.spec.op_type, expected, actual
            ));
        }
        self.operations
            .insert(registration.spec.name.clone(), registration);
        Ok(())
    }

    pub fn registration(&self, name: &str) -> Option<&HandlerRegistration> {
        self.operations.get(name)
    }

    pub fn list_operations(&self) -> Vec<&OperationSpec> {
        self.operations
            .values()
            .filter(|r| r.spec.visibility == Visibility::External)
            .map(|r| &r.spec)
            .collect()
    }

    pub async fn invoke(
        &self,
        name: &str,
        input: Value,
        context: OperationContext,
    ) -> ResponseEnvelope {
        let request_id = context.request_id.clone();
        let registration = match self.operations.get(name) {
            Some(r) => r,
            None => return ResponseEnvelope::not_found(request_id, name),
        };

        if registration.spec.visibility == Visibility::Internal && !context.internal {
            return ResponseEnvelope::not_found(request_id, name);
        }

        let acl = &registration.spec.access_control;
        let identity = if context.internal {
            context
                .handler_identity
                .as_ref()
                .and_then(|ca| ca.as_identity())
        } else {
            context.identity.clone()
        };

        let resource_id = registration
            .spec
            .resource_id_path
            .as_ref()
            .and_then(|path| extract_json_pointer(&input, path));

        if let AccessResult::Forbidden(message) = acl.check(
            identity.as_ref(),
            resource_id.as_deref(),
            context.ownership.as_deref(),
        ) {
            return ResponseEnvelope::forbidden(request_id, message);
        }

        match &registration.handler {
            HandlerKind::Once(handler) => {
                let handler = Arc::clone(handler);
                (handler)(input, context).await
            }
            HandlerKind::Stream(_) => ResponseEnvelope::error(
                request_id,
                CallError::invalid_operation_type(
                    "invoke() called on a Subscription op; use invoke_streaming()",
                ),
            ),
        }
    }

    pub fn invoke_streaming(
        &self,
        name: &str,
        input: Value,
        context: OperationContext,
    ) -> ResponseStream {
        let request_id = context.request_id.clone();
        let name_owned = name.to_string();

        let registration = match self.operations.get(name) {
            Some(r) => r,
            None => {
                return Box::pin(stream::once(async move {
                    ResponseEnvelope::not_found(request_id, &name_owned)
                }));
            }
        };

        if registration.spec.visibility == Visibility::Internal && !context.internal {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::not_found(request_id, &name_owned)
            }));
        }

        let acl = &registration.spec.access_control;
        let identity = if context.internal {
            context
                .handler_identity
                .as_ref()
                .and_then(|ca| ca.as_identity())
        } else {
            context.identity.clone()
        };

        let resource_id = registration
            .spec
            .resource_id_path
            .as_ref()
            .and_then(|path| extract_json_pointer(&input, path));

        if let AccessResult::Forbidden(message) = acl.check(
            identity.as_ref(),
            resource_id.as_deref(),
            context.ownership.as_deref(),
        ) {
            return Box::pin(stream::once(async move {
                ResponseEnvelope::forbidden(request_id, message)
            }));
        }

        let streaming_handler = match &registration.handler {
            HandlerKind::Stream(h) => Arc::clone(h),
            HandlerKind::Once(_) => {
                return Box::pin(stream::once(async move {
                    ResponseEnvelope::error(
                        request_id,
                        CallError::invalid_operation_type(
                            "invoke_streaming() called on a Query/Mutation op; use invoke()",
                        ),
                    )
                }));
            }
        };

        streaming_handler(input, context)
    }
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OperationRegistryBuilder {
    operations: HashMap<String, HandlerRegistration>,
}

impl OperationRegistryBuilder {
    pub fn new() -> Self {
        Self {
            operations: HashMap::new(),
        }
    }

    fn store(mut self, registration: HandlerRegistration) -> Result<Self, String> {
        let name = registration.spec.name.clone();
        self.operations.insert(name, registration);
        Ok(self)
    }

    fn wrap_once(spec: &OperationSpec, handler: Handler) -> Result<HandlerKind, String> {
        match spec.op_type {
            OperationType::Query | OperationType::Mutation => Ok(HandlerKind::Once(handler)),
            OperationType::Subscription => Err(format!(
                "handler kind mismatch: {:?} requires HandlerKind::Stream (got Handler)",
                spec.op_type
            )),
        }
    }

    fn wrap_stream(spec: &OperationSpec, handler: StreamingHandler) -> Result<HandlerKind, String> {
        match spec.op_type {
            OperationType::Subscription => Ok(HandlerKind::Stream(handler)),
            OperationType::Query | OperationType::Mutation => Err(format!(
                "handler kind mismatch: {:?} requires HandlerKind::Once (got StreamingHandler)",
                spec.op_type
            )),
        }
    }

    pub fn with_local(
        self,
        spec: OperationSpec,
        handler: Handler,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedPeerEnv>,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        let kind = Self::wrap_once(&spec, handler)?;
        let registration = HandlerRegistration::new(
            spec,
            kind,
            OperationProvenance::Local,
            composition_authority,
            scoped_env,
            capabilities,
        );
        self.store(registration)
    }

    pub fn with_local_streaming(
        self,
        spec: OperationSpec,
        handler: StreamingHandler,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedPeerEnv>,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        let kind = Self::wrap_stream(&spec, handler)?;
        let registration = HandlerRegistration::new(
            spec,
            kind,
            OperationProvenance::Local,
            composition_authority,
            scoped_env,
            capabilities,
        );
        self.store(registration)
    }

    pub fn with_leaf(
        self,
        spec: OperationSpec,
        handler: Handler,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        self.with_leaf_provenance(
            spec,
            handler,
            OperationProvenance::FromOpenAPI,
            capabilities,
        )
    }

    pub fn with_leaf_provenance(
        self,
        spec: OperationSpec,
        handler: Handler,
        provenance: OperationProvenance,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        let kind = Self::wrap_once(&spec, handler)?;
        let registration =
            HandlerRegistration::new(spec, kind, provenance, None, None, capabilities);
        self.store(registration)
    }

    pub fn with_leaf_streaming(
        self,
        spec: OperationSpec,
        handler: StreamingHandler,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        self.with_leaf_streaming_provenance(
            spec,
            handler,
            OperationProvenance::FromOpenAPI,
            capabilities,
        )
    }

    pub fn with_leaf_streaming_provenance(
        self,
        spec: OperationSpec,
        handler: StreamingHandler,
        provenance: OperationProvenance,
        capabilities: Capabilities,
    ) -> Result<Self, String> {
        let kind = Self::wrap_stream(&spec, handler)?;
        let registration =
            HandlerRegistration::new(spec, kind, provenance, None, None, capabilities);
        self.store(registration)
    }

    pub fn with(self, registration: HandlerRegistration) -> Result<Self, String> {
        self.store(registration)
    }

    pub fn build(self) -> OperationRegistry {
        OperationRegistry {
            operations: self.operations,
        }
    }
}

impl Default for OperationRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn make_handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Value, OperationContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ResponseEnvelope> + Send + 'static,
{
    Arc::new(move |input, context| Box::pin(f(input, context)))
}

pub fn make_streaming_handler<S, St>(f: S) -> StreamingHandler
where
    S: Fn(Value, OperationContext) -> St + Send + Sync + 'static,
    St: Stream<Item = ResponseEnvelope> + Send + 'static,
{
    Arc::new(move |input, context| Box::pin(f(input, context)))
}

/// Extract a string value from `input` at a JSON-pointer-ish path described
/// by `$.field` or `$.field/sub` (a leading `$` followed by a slash-separated
/// pointer, the same shape `serde_json::Value::pointer` expects after the
/// leading `/` is restored). Also tolerates the dotted form `$.a.b` for
/// shallow nested lookups (translated to `/a/b`).
///
/// Returns `None` if the path is malformed, the field is missing, or the
/// value at the path is not a string. Graceful — never panics (ADR-050 §2a):
/// a missing field simply yields `resource_id: None`, and the caller's
/// `AccessControl::check` decides whether to deny based on whether the spec
/// requires a specific resource ID.
pub(crate) fn extract_json_pointer(input: &Value, path: &str) -> Option<String> {
    let trimmed = path.strip_prefix('$')?;
    if trimmed.is_empty() {
        return None;
    }
    let pointer = if let Some(rest) = trimmed.strip_prefix('/') {
        format!("/{}", rest)
    } else {
        let dotted = trimmed.replace('.', "/");
        if let Some(rest) = dotted.strip_prefix('/') {
            format!("/{}", rest)
        } else {
            format!("/{}", dotted)
        }
    };
    input
        .pointer(pointer.as_str())
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::CallError;
    use crate::registry::context::AbortPolicy;
    use crate::registry::env::OperationEnv;
    use crate::registry::spec::{AccessControl, OperationType};
    use alknet_core::auth::Identity;
    use alknet_core::ownership::OwnershipProvider;
    use std::collections::HashMap;
    use std::time::Duration;

    struct NoopEnv;

    #[async_trait::async_trait]
    impl OperationEnv for NoopEnv {
        async fn invoke_with_policy(
            &self,
            _namespace: &str,
            _operation: &str,
            _input: Value,
            _parent: &OperationContext,
            _policy: AbortPolicy,
        ) -> ResponseEnvelope {
            ResponseEnvelope::error("test", CallError::internal("noop env does not dispatch"))
        }

        fn contains(&self, _name: &str) -> bool {
            false
        }
    }

    fn root_context(
        request_id: &str,
        identity: Option<Identity>,
        handler_identity: Option<CompositionAuthority>,
        internal: bool,
        scoped_env: ScopedPeerEnv,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env,
            env: Arc::new(NoopEnv),
            abort_policy: AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal,
            ownership: None,
        }
    }

    fn root_context_with_ownership(
        request_id: &str,
        identity: Option<Identity>,
        ownership: Option<Arc<dyn OwnershipProvider>>,
        scoped_env: ScopedPeerEnv,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity: None,
            forwarded_for: None,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env,
            env: Arc::new(NoopEnv),
            abort_policy: AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal: false,
            ownership,
        }
    }

    fn echo_handler() -> Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    fn error_handler() -> Handler {
        make_handler(|_input, context| async move {
            ResponseEnvelope::error(context.request_id, CallError::internal("handler failure"))
        })
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
            None,
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
            None,
        )
    }

    fn identity_with_scopes(id: &str, scopes: &[&str]) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            resources: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn register_and_invoke_simple_operation() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("echo", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-1", None, None, false, ScopedPeerEnv::empty());
        let response = registry
            .invoke("echo", serde_json::json!({"hi": 1}), ctx)
            .await;
        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.result, Ok(serde_json::json!({"hi": 1})));
    }

    #[tokio::test]
    async fn internal_op_from_external_call_returns_not_found() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                internal_spec("secret", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-2", None, None, false, ScopedPeerEnv::empty());
        let response = registry.invoke("secret", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("secret"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn internal_op_from_internal_call_invokes_handler() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                internal_spec("secret", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-3", None, None, true, ScopedPeerEnv::empty());
        let response = registry
            .invoke("secret", serde_json::json!({"x": 2}), ctx)
            .await;
        assert_eq!(response.request_id, "req-3");
        assert_eq!(response.result, Ok(serde_json::json!({"x": 2})));
    }

    #[tokio::test]
    async fn unknown_op_returns_not_found() {
        let registry = OperationRegistry::new();
        let ctx = root_context("req-4", None, None, false, ScopedPeerEnv::empty());
        let response = registry.invoke("missing", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn acl_sufficient_scopes_allowed() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec(
                    "admin",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-5",
            Some(identity_with_scopes("caller", &["admin"])),
            None,
            false,
            ScopedPeerEnv::empty(),
        );
        let response = registry.invoke("admin", serde_json::json!({}), ctx).await;
        assert!(response.result.is_ok());
    }

    #[tokio::test]
    async fn acl_insufficient_scopes_forbidden() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec(
                    "admin",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-6",
            Some(identity_with_scopes("caller", &["user"])),
            None,
            false,
            ScopedPeerEnv::empty(),
        );
        let response = registry.invoke("admin", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("admin"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn acl_restricted_op_no_identity_forbidden() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec(
                    "admin",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-7", None, None, false, ScopedPeerEnv::empty());
        let response = registry.invoke("admin", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert_eq!(e.message, "authentication required");
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn internal_call_acl_uses_handler_identity() {
        let mut registry = OperationRegistry::new();
        let composing_authority = CompositionAuthority::new("agent-chat", ["admin".to_string()]);
        registry
            .register(HandlerRegistration::new(
                internal_spec(
                    "secret",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-8",
            Some(identity_with_scopes("user", &["user"])),
            Some(composing_authority),
            true,
            ScopedPeerEnv::empty(),
        );
        let response = registry.invoke("secret", serde_json::json!({}), ctx).await;
        assert!(
            response.result.is_ok(),
            "internal call should use handler_identity (admin), not caller (user)"
        );
    }

    #[tokio::test]
    async fn internal_call_acl_insufficient_handler_identity_forbidden() {
        let mut registry = OperationRegistry::new();
        let weak_authority = CompositionAuthority::new("weak", ["user".to_string()]);
        registry
            .register(HandlerRegistration::new(
                internal_spec(
                    "secret",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-9",
            Some(identity_with_scopes("user", &["admin"])),
            Some(weak_authority),
            true,
            ScopedPeerEnv::empty(),
        );
        let response = registry.invoke("secret", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("admin"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn external_call_acl_uses_caller_identity_not_handler_identity() {
        let mut registry = OperationRegistry::new();
        let handler_authority = CompositionAuthority::new("agent", ["admin".to_string()]);
        registry
            .register(HandlerRegistration::new(
                external_spec(
                    "gate",
                    AccessControl {
                        required_scopes: vec!["admin".to_string()],
                        ..Default::default()
                    },
                ),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                Some(handler_authority),
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-10",
            Some(identity_with_scopes("user", &["user"])),
            Some(CompositionAuthority::new("agent", ["admin".to_string()])),
            false,
            ScopedPeerEnv::empty(),
        );
        let response = registry.invoke("gate", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "FORBIDDEN"),
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_operations_returns_external_only() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("echo", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        registry
            .register(HandlerRegistration::new(
                internal_spec("secret", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ops = registry.list_operations();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "echo");
    }

    #[tokio::test]
    async fn handler_returned_error_passes_through() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("boom", AccessControl::default()),
                HandlerKind::Once(error_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-11", None, None, false, ScopedPeerEnv::empty());
        let response = registry.invoke("boom", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "INTERNAL"),
            other => panic!("expected INTERNAL error, got {other:?}"),
        }
    }

    #[test]
    fn builder_with_local_sets_provenance_local() {
        let registry = OperationRegistryBuilder::new()
            .with_local(
                external_spec("echo", AccessControl::default()),
                echo_handler(),
                CompositionAuthority::none(),
                ScopedPeerEnv::empty().into(),
                Capabilities::new(),
            )
            .unwrap()
            .build();
        let reg = registry.registration("echo").expect("registered");
        assert_eq!(reg.provenance, OperationProvenance::Local);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_some());
    }

    #[test]
    fn builder_with_local_carries_authority_and_scoped_env() {
        let registry = OperationRegistryBuilder::new()
            .with_local(
                external_spec("agent", AccessControl::default()),
                echo_handler(),
                Some(CompositionAuthority::new("agent", ["fs:read".to_string()])),
                Some(ScopedPeerEnv::new(["fs/readFile"])),
                Capabilities::new(),
            )
            .unwrap()
            .build();
        let reg = registry.registration("agent").expect("registered");
        assert_eq!(reg.provenance, OperationProvenance::Local);
        let authority = reg.composition_authority.as_ref().expect("authority set");
        assert_eq!(authority.label, "agent");
        assert_eq!(authority.scopes, vec!["fs:read".to_string()]);
        assert!(reg.scoped_env.is_some());
        assert!(reg.scoped_env.as_ref().unwrap().allows("fs/readFile"));
    }

    #[test]
    fn builder_with_leaf_sets_provenance_and_no_authority() {
        let registry = OperationRegistryBuilder::new()
            .with_leaf(
                external_spec("vastai", AccessControl::default()),
                echo_handler(),
                Capabilities::new(),
            )
            .unwrap()
            .build();
        let reg = registry.registration("vastai").expect("registered");
        assert_eq!(reg.provenance, OperationProvenance::FromOpenAPI);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_none());
    }

    #[test]
    fn builder_with_leaf_provenance_overrides_provenance() {
        let registry = OperationRegistryBuilder::new()
            .with_leaf_provenance(
                external_spec("remote", AccessControl::default()),
                echo_handler(),
                OperationProvenance::FromCall,
                Capabilities::new(),
            )
            .unwrap()
            .build();
        let reg = registry.registration("remote").expect("registered");
        assert_eq!(reg.provenance, OperationProvenance::FromCall);
        assert!(reg.composition_authority.is_none());
        assert!(reg.scoped_env.is_none());
    }

    #[test]
    fn builder_with_takes_full_bundle() {
        let registration = HandlerRegistration::new(
            external_spec("agent", AccessControl::default()),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::Session,
            Some(CompositionAuthority::new("sandbox", [])),
            Some(ScopedPeerEnv::new(["fs/readFile"])),
            Capabilities::new(),
        );
        let registry = OperationRegistryBuilder::new()
            .with(registration)
            .unwrap()
            .build();
        let reg = registry.registration("agent").expect("registered");
        assert_eq!(reg.provenance, OperationProvenance::Session);
        assert!(reg.composition_authority.is_some());
        assert!(reg.scoped_env.is_some());
    }

    #[test]
    fn builder_default_is_new() {
        let builder = OperationRegistryBuilder::default();
        let registry = builder.build();
        assert!(registry.list_operations().is_empty());
    }

    #[test]
    fn registry_default_is_new() {
        let registry = OperationRegistry::default();
        assert!(registry.list_operations().is_empty());
        assert!(registry.registration("anything").is_none());
    }

    #[test]
    fn registration_lookup_returns_bundle_fields() {
        let mut registry = OperationRegistry::new();
        let authority = CompositionAuthority::new("agent", ["fs:read".to_string()]);
        let scoped = ScopedPeerEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        registry
            .register(HandlerRegistration::new(
                external_spec("agent", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                Some(authority.clone()),
                Some(scoped.clone()),
                caps.clone(),
            ))
            .unwrap();
        let reg = registry.registration("agent").expect("found");
        assert_eq!(reg.spec.name, "agent");
        assert_eq!(reg.provenance, OperationProvenance::Local);
        assert_eq!(reg.composition_authority.as_ref().unwrap().label, "agent");
        assert!(reg.scoped_env.as_ref().unwrap().allows("fs/readFile"));
    }

    fn subscription_spec(name: &str) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Subscription,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        )
    }

    fn echo_streaming_handler() -> StreamingHandler {
        make_streaming_handler(|input, context| {
            futures::stream::iter(vec![ResponseEnvelope::ok(context.request_id, input)])
        })
    }

    #[tokio::test]
    async fn invoke_on_stream_kind_returns_invalid_operation_type() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                subscription_spec("events/stream"),
                HandlerKind::Stream(echo_streaming_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-iot", None, None, false, ScopedPeerEnv::empty());
        let response = registry
            .invoke("events/stream", serde_json::json!({}), ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "INVALID_OPERATION_TYPE"),
            other => panic!("expected INVALID_OPERATION_TYPE, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_on_once_kind_dispatches_normally() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("echo", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-once", None, None, false, ScopedPeerEnv::empty());
        let response = registry
            .invoke("echo", serde_json::json!({"hi": 1}), ctx)
            .await;
        assert_eq!(response.result, Ok(serde_json::json!({"hi": 1})));
    }

    #[test]
    fn register_rejects_once_for_subscription_spec() {
        let mut registry = OperationRegistry::new();
        let result = registry.register(HandlerRegistration::new(
            subscription_spec("events/stream"),
            HandlerKind::Once(echo_handler()),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        match result {
            Err(msg) => assert!(
                msg.contains("Subscription")
                    && msg.contains("HandlerKind::Stream")
                    && msg.contains("HandlerKind::Once"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[test]
    fn register_rejects_stream_for_query_spec() {
        let mut registry = OperationRegistry::new();
        let result = registry.register(HandlerRegistration::new(
            external_spec("echo", AccessControl::default()),
            HandlerKind::Stream(echo_streaming_handler()),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        match result {
            Err(msg) => assert!(
                (msg.contains("Query") || msg.contains("Mutation"))
                    && msg.contains("HandlerKind::Once")
                    && msg.contains("HandlerKind::Stream"),
                "unexpected message: {msg}"
            ),
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn make_streaming_handler_produces_working_stream() {
        use futures::stream::StreamExt;
        let handler = echo_streaming_handler();
        let ctx = root_context("req-st", None, None, false, ScopedPeerEnv::empty());
        let mut stream = handler(serde_json::json!({"v": 1}), ctx);
        let first = stream.next().await.expect("one envelope");
        assert_eq!(first.result, Ok(serde_json::json!({"v": 1})));
        let second = stream.next().await;
        assert!(second.is_none(), "stream ends after one value");
    }

    #[test]
    fn call_error_invalid_operation_type_is_not_retryable() {
        let err = CallError::invalid_operation_type("bad path");
        assert_eq!(err.code, "INVALID_OPERATION_TYPE");
        assert!(!err.retryable);
        assert!(err.details.is_none());
    }

    async fn collect_stream(mut s: ResponseStream) -> Vec<ResponseEnvelope> {
        use futures::stream::StreamExt;
        let mut out = Vec::new();
        while let Some(env) = s.next().await {
            out.push(env);
        }
        out
    }

    #[tokio::test]
    async fn invoke_streaming_on_subscription_dispatches_handler_stream() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                subscription_spec("events/stream"),
                HandlerKind::Stream(echo_streaming_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-is-1", None, None, false, ScopedPeerEnv::empty());
        let stream = registry.invoke_streaming("events/stream", serde_json::json!({"v": 7}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].request_id, "req-is-1");
        assert_eq!(items[0].result, Ok(serde_json::json!({"v": 7})));
    }

    #[tokio::test]
    async fn invoke_streaming_on_unknown_op_yields_single_not_found() {
        let registry = OperationRegistry::new();
        let ctx = root_context("req-is-2", None, None, false, ScopedPeerEnv::empty());
        let stream = registry.invoke_streaming("missing", serde_json::json!({}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("missing"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_on_query_op_yields_invalid_operation_type() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                external_spec("echo", AccessControl::default()),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-is-3", None, None, false, ScopedPeerEnv::empty());
        let stream = registry.invoke_streaming("echo", serde_json::json!({}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => assert_eq!(e.code, "INVALID_OPERATION_TYPE"),
            other => panic!("expected INVALID_OPERATION_TYPE, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_internal_op_from_external_yields_not_found() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                internal_subscription_spec(AccessControl::default()),
                HandlerKind::Stream(echo_streaming_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context("req-is-4", None, None, false, ScopedPeerEnv::empty());
        let stream = registry.invoke_streaming("events/stream", serde_json::json!({}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "NOT_FOUND");
                assert!(e.message.contains("events/stream"));
            }
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_acl_denied_yields_forbidden() {
        let mut registry = OperationRegistry::new();
        registry
            .register(HandlerRegistration::new(
                subscription_spec_with_acl(AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                }),
                HandlerKind::Stream(echo_streaming_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-is-5",
            Some(identity_with_scopes("caller", &["user"])),
            None,
            false,
            ScopedPeerEnv::empty(),
        );
        let stream = registry.invoke_streaming("events/stream", serde_json::json!({}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        match &items[0].result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("admin"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_streaming_internal_call_uses_handler_identity_for_acl() {
        let mut registry = OperationRegistry::new();
        let composing_authority = CompositionAuthority::new("agent-chat", ["admin".to_string()]);
        registry
            .register(HandlerRegistration::new(
                internal_subscription_spec(AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                }),
                HandlerKind::Stream(echo_streaming_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let ctx = root_context(
            "req-is-6",
            Some(identity_with_scopes("user", &["user"])),
            Some(composing_authority),
            true,
            ScopedPeerEnv::empty(),
        );
        let stream = registry.invoke_streaming("events/stream", serde_json::json!({"ok": 1}), ctx);
        let items = collect_stream(stream).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].request_id, "req-is-6");
        assert_eq!(items[0].result, Ok(serde_json::json!({"ok": 1})));
    }

    fn subscription_spec_with_acl(acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            "events/stream",
            OperationType::Subscription,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
            None,
        )
    }

    fn internal_subscription_spec(acl: AccessControl) -> OperationSpec {
        OperationSpec::new(
            "events/stream",
            OperationType::Subscription,
            Visibility::Internal,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
            None,
        )
    }

    struct MockOwnership {
        owned: Vec<(String, String)>,
    }

    impl OwnershipProvider for MockOwnership {
        fn owns(
            &self,
            _identity: &Identity,
            resource_type: &str,
            resource_id: &str,
            _action: &str,
        ) -> bool {
            self.owned
                .iter()
                .any(|(rt, rid)| rt == resource_type && rid == resource_id)
        }

        fn owned_resources(&self, _identity: &Identity, resource_type: &str) -> Vec<String> {
            self.owned
                .iter()
                .filter(|(rt, _)| rt == resource_type)
                .map(|(_, rid)| rid.clone())
                .collect()
        }

        fn owns_any(&self, _identity: &Identity, resource_type: &str) -> bool {
            self.owned.iter().any(|(rt, _)| rt == resource_type)
        }
    }

    fn empty_identity(id: &str) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        }
    }

    fn resource_spec(
        name: &str,
        acl: AccessControl,
        resource_id_path: Option<&str>,
    ) -> OperationSpec {
        OperationSpec::new(
            name,
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            acl,
            resource_id_path.map(|s| s.to_string()),
        )
    }

    #[test]
    fn extract_json_pointer_resolves_single_field() {
        let input = serde_json::json!({"containerId": "abc123"});
        assert_eq!(
            extract_json_pointer(&input, "$.containerId"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn extract_json_pointer_missing_field_returns_none() {
        let input = serde_json::json!({"other": 1});
        assert_eq!(extract_json_pointer(&input, "$.containerId"), None);
    }

    #[test]
    fn extract_json_pointer_non_string_value_returns_none() {
        let input = serde_json::json!({"containerId": 42});
        assert_eq!(extract_json_pointer(&input, "$.containerId"), None);
    }

    #[test]
    fn extract_json_pointer_no_leading_dollar_returns_none() {
        let input = serde_json::json!({"containerId": "abc"});
        assert_eq!(extract_json_pointer(&input, "containerId"), None);
    }

    #[test]
    fn extract_json_pointer_empty_after_dollar_returns_none() {
        let input = serde_json::json!({"a": "b"});
        assert_eq!(extract_json_pointer(&input, "$"), None);
    }

    #[test]
    fn extract_json_pointer_nested_slash_path() {
        let input = serde_json::json!({"data": {"id": "xyz"}});
        assert_eq!(
            extract_json_pointer(&input, "$.data/id"),
            Some("xyz".to_string())
        );
    }

    #[test]
    fn extract_json_pointer_nested_dotted_path() {
        let input = serde_json::json!({"data": {"id": "xyz"}});
        assert_eq!(
            extract_json_pointer(&input, "$.data.id"),
            Some("xyz".to_string())
        );
    }

    #[tokio::test]
    async fn invoke_with_ownership_provider_allows_owned_resource() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        registry
            .register(HandlerRegistration::new(
                resource_spec("container/exec", acl, Some("$.containerId")),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let provider: Arc<dyn OwnershipProvider> = Arc::new(MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        });
        let ctx = root_context_with_ownership(
            "req-owns",
            Some(empty_identity("alice")),
            Some(provider),
            ScopedPeerEnv::empty(),
        );
        let response = registry
            .invoke(
                "container/exec",
                serde_json::json!({"containerId": "c1"}),
                ctx,
            )
            .await;
        assert!(response.result.is_ok(), "owned resource should be allowed");
    }

    #[tokio::test]
    async fn invoke_with_ownership_provider_forbids_unowned_resource() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        registry
            .register(HandlerRegistration::new(
                resource_spec("container/exec", acl, Some("$.containerId")),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let provider: Arc<dyn OwnershipProvider> = Arc::new(MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        });
        let ctx = root_context_with_ownership(
            "req-not-owns",
            Some(empty_identity("alice")),
            Some(provider),
            ScopedPeerEnv::empty(),
        );
        let response = registry
            .invoke(
                "container/exec",
                serde_json::json!({"containerId": "c2"}),
                ctx,
            )
            .await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("container/c2"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_with_ownership_provider_missing_field_falls_back_to_owns_any() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        registry
            .register(HandlerRegistration::new(
                resource_spec("container/exec", acl, Some("$.containerId")),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let provider: Arc<dyn OwnershipProvider> = Arc::new(MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        });
        let ctx = root_context_with_ownership(
            "req-missing",
            Some(empty_identity("alice")),
            Some(provider),
            ScopedPeerEnv::empty(),
        );
        let response = registry
            .invoke("container/exec", serde_json::json!({}), ctx)
            .await;
        assert!(
            response.result.is_ok(),
            "missing field → resource_id None → owns_any path allowed"
        );
    }

    #[tokio::test]
    async fn invoke_with_ownership_provider_missing_field_forbids_when_not_owns_any() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        registry
            .register(HandlerRegistration::new(
                resource_spec("container/exec", acl, Some("$.containerId")),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let provider: Arc<dyn OwnershipProvider> = Arc::new(MockOwnership {
            owned: vec![("volume".to_string(), "v1".to_string())],
        });
        let ctx = root_context_with_ownership(
            "req-missing-denied",
            Some(empty_identity("alice")),
            Some(provider),
            ScopedPeerEnv::empty(),
        );
        let response = registry
            .invoke("container/exec", serde_json::json!({}), ctx)
            .await;
        match response.result {
            Err(e) => {
                assert_eq!(e.code, "FORBIDDEN");
                assert!(e.message.contains("container"));
            }
            other => panic!("expected FORBIDDEN, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invoke_without_ownership_provider_falls_back_to_static_resources() {
        let mut registry = OperationRegistry::new();
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        registry
            .register(HandlerRegistration::new(
                resource_spec("container/exec", acl, Some("$.containerId")),
                HandlerKind::Once(echo_handler()),
                OperationProvenance::Local,
                None,
                None,
                Capabilities::new(),
            ))
            .unwrap();
        let mut resources = HashMap::new();
        resources.insert("container".to_string(), vec!["exec".to_string()]);
        let identity = Identity {
            id: "alice".to_string(),
            scopes: vec![],
            resources,
        };
        let ctx =
            root_context_with_ownership("req-static", Some(identity), None, ScopedPeerEnv::empty());
        let response = registry
            .invoke(
                "container/exec",
                serde_json::json!({"containerId": "c1"}),
                ctx,
            )
            .await;
        assert!(
            response.result.is_ok(),
            "no provider wired → static Identity.resources fallback allows"
        );
    }
}

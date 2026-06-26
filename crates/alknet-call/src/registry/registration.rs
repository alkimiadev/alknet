use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use alknet_core::types::Capabilities;
use serde_json::Value;

use super::context::{CompositionAuthority, OperationContext, ScopedOperationEnv};
use super::spec::{AccessResult, OperationSpec, Visibility};
use crate::protocol::wire::ResponseEnvelope;

pub type Handler = Arc<
    dyn Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>>
        + Send
        + Sync,
>;

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
    pub handler: Handler,
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>,
    pub scoped_env: Option<ScopedOperationEnv>,
    pub capabilities: Capabilities,
    /// Whether this op is exposed to `CallClient` peers. Defaults to `false`
    /// across all provenance (ADR-028 §4) — default-deny. The `CallClient`
    /// dispatch path filters on this field (trusted-peer mode bypasses it,
    /// ADR-028 §3). Data-only here; the filtering behavior is wired in the
    /// `CallClient` task, not here.
    pub remote_safe: bool,
}

impl HandlerRegistration {
    pub fn new(
        spec: OperationSpec,
        handler: Handler,
        provenance: OperationProvenance,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedOperationEnv>,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            spec,
            handler,
            provenance,
            composition_authority,
            scoped_env,
            capabilities,
            remote_safe: false,
        }
    }

    /// Chainable setter for `remote_safe` (ADR-028). Flips this op to be
    /// exposed to `CallClient` peers in default-deny mode.
    pub fn remote_safe(mut self, remote_safe: bool) -> Self {
        self.remote_safe = remote_safe;
        self
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

    pub fn register(&mut self, registration: HandlerRegistration) {
        self.operations
            .insert(registration.spec.name.clone(), registration);
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

    /// List `External` op specs, additionally filtered by `remote_safe` for
    /// peer-scoped serving (ADR-028 Assumption 2). When `trusted_peer` is true,
    /// the `remote_safe` filter is bypassed (all `External` ops listed).
    pub fn list_operations_peer_scoped(&self, trusted_peer: bool) -> Vec<&OperationSpec> {
        self.operations
            .values()
            .filter(|r| r.spec.visibility == Visibility::External)
            .filter(|r| trusted_peer || r.remote_safe)
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

        if let AccessResult::Forbidden(message) = acl.check(identity.as_ref()) {
            return ResponseEnvelope::forbidden(request_id, message);
        }

        let handler = Arc::clone(&registration.handler);
        (handler)(input, context).await
    }
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct OperationRegistryBuilder {
    operations: HashMap<String, HandlerRegistration>,
    last_name: Option<String>,
}

impl OperationRegistryBuilder {
    pub fn new() -> Self {
        Self {
            operations: HashMap::new(),
            last_name: None,
        }
    }

    fn store(mut self, registration: HandlerRegistration) -> Self {
        self.last_name = Some(registration.spec.name.clone());
        self.operations
            .insert(registration.spec.name.clone(), registration);
        self
    }

    pub fn with_local(
        self,
        spec: OperationSpec,
        handler: Handler,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedOperationEnv>,
        capabilities: Capabilities,
    ) -> Self {
        let registration = HandlerRegistration::new(
            spec,
            handler,
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
    ) -> Self {
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
    ) -> Self {
        let registration =
            HandlerRegistration::new(spec, handler, provenance, None, None, capabilities);
        self.store(registration)
    }

    pub fn with(self, registration: HandlerRegistration) -> Self {
        self.store(registration)
    }

    /// Mark the most-recently-registered op as `remote_safe: true` (ADR-028).
    /// Chainable right after a `with_local` / `with_leaf` / `with_leaf_provenance`
    /// / `with` call. Panics if nothing has been registered yet (programming
    /// error, not a runtime condition).
    pub fn remote_safe(mut self) -> Self {
        let name = self
            .last_name
            .clone()
            .expect("remote_safe() called before any registration");
        let last = self
            .operations
            .get_mut(&name)
            .expect("last-registered op must be present");
        last.remote_safe = true;
        self
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::CallError;
    use crate::registry::context::AbortPolicy;
    use crate::registry::env::OperationEnv;
    use crate::registry::spec::{AccessControl, OperationType};
    use alknet_core::auth::Identity;
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
        scoped_env: ScopedOperationEnv,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env,
            env: Arc::new(NoopEnv),
            abort_policy: AbortPolicy::default(),
            deadline: Some(std::time::Instant::now() + Duration::from_secs(30)),
            internal,
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
        registry.register(HandlerRegistration::new(
            external_spec("echo", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context("req-1", None, None, false, ScopedOperationEnv::empty());
        let response = registry
            .invoke("echo", serde_json::json!({"hi": 1}), ctx)
            .await;
        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.result, Ok(serde_json::json!({"hi": 1})));
    }

    #[tokio::test]
    async fn internal_op_from_external_call_returns_not_found() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            internal_spec("secret", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context("req-2", None, None, false, ScopedOperationEnv::empty());
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
        registry.register(HandlerRegistration::new(
            internal_spec("secret", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context("req-3", None, None, true, ScopedOperationEnv::empty());
        let response = registry
            .invoke("secret", serde_json::json!({"x": 2}), ctx)
            .await;
        assert_eq!(response.request_id, "req-3");
        assert_eq!(response.result, Ok(serde_json::json!({"x": 2})));
    }

    #[tokio::test]
    async fn unknown_op_returns_not_found() {
        let registry = OperationRegistry::new();
        let ctx = root_context("req-4", None, None, false, ScopedOperationEnv::empty());
        let response = registry.invoke("missing", serde_json::json!({}), ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn acl_sufficient_scopes_allowed() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec(
                "admin",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context(
            "req-5",
            Some(identity_with_scopes("caller", &["admin"])),
            None,
            false,
            ScopedOperationEnv::empty(),
        );
        let response = registry.invoke("admin", serde_json::json!({}), ctx).await;
        assert!(response.result.is_ok());
    }

    #[tokio::test]
    async fn acl_insufficient_scopes_forbidden() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec(
                "admin",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context(
            "req-6",
            Some(identity_with_scopes("caller", &["user"])),
            None,
            false,
            ScopedOperationEnv::empty(),
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
        registry.register(HandlerRegistration::new(
            external_spec(
                "admin",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context("req-7", None, None, false, ScopedOperationEnv::empty());
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
        registry.register(HandlerRegistration::new(
            internal_spec(
                "secret",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context(
            "req-8",
            Some(identity_with_scopes("user", &["user"])),
            Some(composing_authority),
            true,
            ScopedOperationEnv::empty(),
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
        registry.register(HandlerRegistration::new(
            internal_spec(
                "secret",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context(
            "req-9",
            Some(identity_with_scopes("user", &["admin"])),
            Some(weak_authority),
            true,
            ScopedOperationEnv::empty(),
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
        registry.register(HandlerRegistration::new(
            external_spec(
                "gate",
                AccessControl {
                    required_scopes: vec!["admin".to_string()],
                    ..Default::default()
                },
            ),
            echo_handler(),
            OperationProvenance::Local,
            Some(handler_authority),
            None,
            Capabilities::new(),
        ));
        let ctx = root_context(
            "req-10",
            Some(identity_with_scopes("user", &["user"])),
            Some(CompositionAuthority::new("agent", ["admin".to_string()])),
            false,
            ScopedOperationEnv::empty(),
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
        registry.register(HandlerRegistration::new(
            external_spec("echo", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        registry.register(HandlerRegistration::new(
            internal_spec("secret", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ops = registry.list_operations();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "echo");
    }

    #[tokio::test]
    async fn handler_returned_error_passes_through() {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            external_spec("boom", AccessControl::default()),
            error_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        ));
        let ctx = root_context("req-11", None, None, false, ScopedOperationEnv::empty());
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
                ScopedOperationEnv::empty().into(),
                Capabilities::new(),
            )
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
                Some(ScopedOperationEnv::new(["fs/readFile"])),
                Capabilities::new(),
            )
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
            echo_handler(),
            OperationProvenance::Session,
            Some(CompositionAuthority::new("sandbox", [])),
            Some(ScopedOperationEnv::new(["fs/readFile"])),
            Capabilities::new(),
        );
        let registry = OperationRegistryBuilder::new().with(registration).build();
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
        let scoped = ScopedOperationEnv::new(["fs/readFile"]);
        let caps = Capabilities::new().with_api_key("google", "k".to_string());
        registry.register(HandlerRegistration::new(
            external_spec("agent", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            Some(authority.clone()),
            Some(scoped.clone()),
            caps.clone(),
        ));
        let reg = registry.registration("agent").expect("found");
        assert_eq!(reg.spec.name, "agent");
        assert_eq!(reg.provenance, OperationProvenance::Local);
        assert_eq!(reg.composition_authority.as_ref().unwrap().label, "agent");
        assert!(reg.scoped_env.as_ref().unwrap().allows("fs/readFile"));
    }

    #[test]
    fn handler_registration_default_remote_safe_is_false() {
        let reg = HandlerRegistration::new(
            external_spec("echo", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        );
        assert!(
            !reg.remote_safe,
            "remote_safe must default to false (ADR-028 §4)"
        );
    }

    #[test]
    fn handler_registration_remote_safe_setter_flips_field() {
        let reg = HandlerRegistration::new(
            external_spec("echo", AccessControl::default()),
            echo_handler(),
            OperationProvenance::Local,
            None,
            None,
            Capabilities::new(),
        )
        .remote_safe(true);
        assert!(reg.remote_safe);
    }

    #[test]
    fn all_provenance_variants_default_remote_safe_false() {
        for provenance in [
            OperationProvenance::Local,
            OperationProvenance::Session,
            OperationProvenance::FromOpenAPI,
            OperationProvenance::FromMCP,
            OperationProvenance::FromCall,
            OperationProvenance::FromJsonSchema,
        ] {
            let reg = HandlerRegistration::new(
                external_spec("op", AccessControl::default()),
                echo_handler(),
                provenance,
                None,
                None,
                Capabilities::new(),
            );
            assert!(
                !reg.remote_safe,
                "provenance {provenance:?} must default remote_safe to false (ADR-028 §4)"
            );
        }
    }

    #[test]
    fn builder_remote_safe_marks_last_inserted() {
        let registry = OperationRegistryBuilder::new()
            .with_local(
                external_spec("first", AccessControl::default()),
                echo_handler(),
                None,
                None,
                Capabilities::new(),
            )
            .with_leaf(
                external_spec("second", AccessControl::default()),
                echo_handler(),
                Capabilities::new(),
            )
            .remote_safe()
            .build();
        assert!(
            !registry.registration("first").unwrap().remote_safe,
            "first op untouched by remote_safe()"
        );
        assert!(
            registry.registration("second").unwrap().remote_safe,
            "remote_safe() marks the most-recently-registered op"
        );
    }

    #[test]
    fn builder_existing_call_sites_compile_unchanged() {
        // Verifies the field addition does not require changes to existing
        // call-site shapes (defaults to false).
        let registry = OperationRegistryBuilder::new()
            .with_local(
                external_spec("a", AccessControl::default()),
                echo_handler(),
                Some(CompositionAuthority::new("agent", ["fs:read".to_string()])),
                Some(ScopedOperationEnv::new(["fs/readFile"])),
                Capabilities::new(),
            )
            .with_leaf(
                external_spec("b", AccessControl::default()),
                echo_handler(),
                Capabilities::new(),
            )
            .with_leaf_provenance(
                external_spec("c", AccessControl::default()),
                echo_handler(),
                OperationProvenance::FromCall,
                Capabilities::new(),
            )
            .with(HandlerRegistration::new(
                external_spec("d", AccessControl::default()),
                echo_handler(),
                OperationProvenance::Session,
                None,
                None,
                Capabilities::new(),
            ))
            .build();
        for name in ["a", "b", "c", "d"] {
            assert!(
                !registry.registration(name).unwrap().remote_safe,
                "{name} should default remote_safe false without explicit setter"
            );
        }
    }
}

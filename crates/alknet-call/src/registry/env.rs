use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::context::{generate_request_id, AbortPolicy, OperationContext, ScopedOperationEnv};
use super::registration::OperationRegistry;
use crate::protocol::wire::ResponseEnvelope;

#[async_trait::async_trait]
pub trait OperationEnv: Send + Sync {
    async fn invoke(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
    ) -> ResponseEnvelope {
        self.invoke_with_policy(namespace, operation, input, parent, parent.abort_policy)
            .await
    }

    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope;

    fn contains(&self, _name: &str) -> bool {
        true
    }
}

pub struct LocalOperationEnv {
    registry: Arc<OperationRegistry>,
}

impl LocalOperationEnv {
    pub fn new(registry: Arc<OperationRegistry>) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &Arc<OperationRegistry> {
        &self.registry
    }
}

#[async_trait::async_trait]
impl OperationEnv for LocalOperationEnv {
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
        }

        let registration = match self.registry.registration(&name) {
            Some(r) => r,
            None => return ResponseEnvelope::not_found(parent.request_id.clone(), &name),
        };

        let context = OperationContext {
            request_id: generate_request_id(),
            parent_request_id: Some(parent.request_id.clone()),
            identity: parent
                .handler_identity
                .as_ref()
                .and_then(|ca| ca.as_identity()),
            handler_identity: registration.composition_authority.clone(),
            capabilities: parent.capabilities.clone(),
            metadata: HashMap::new(),
            abort_policy: policy,
            deadline: parent.deadline,
            scoped_env: registration
                .scoped_env
                .clone()
                .unwrap_or_else(ScopedOperationEnv::empty),
            env: parent.env.clone(),
            internal: true,
        };

        self.registry.invoke(&name, input, context).await
    }
}

pub struct CompositeOperationEnv {
    session: Option<Arc<dyn OperationEnv + Send + Sync>>,
    connection: Option<Arc<dyn OperationEnv + Send + Sync>>,
    base: Arc<dyn OperationEnv + Send + Sync>,
}

impl CompositeOperationEnv {
    pub fn new(
        base: Arc<dyn OperationEnv + Send + Sync>,
        connection: Option<Arc<dyn OperationEnv + Send + Sync>>,
        session: Option<Arc<dyn OperationEnv + Send + Sync>>,
    ) -> Self {
        Self {
            session,
            connection,
            base,
        }
    }

    pub fn base(&self) -> &Arc<dyn OperationEnv + Send + Sync> {
        &self.base
    }

    pub fn connection(&self) -> &Option<Arc<dyn OperationEnv + Send + Sync>> {
        &self.connection
    }

    pub fn session(&self) -> &Option<Arc<dyn OperationEnv + Send + Sync>> {
        &self.session
    }
}

#[async_trait::async_trait]
impl OperationEnv for CompositeOperationEnv {
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(parent.request_id.clone(), &name);
        }

        if let Some(session) = &self.session {
            if session.contains(&name) {
                return session
                    .invoke_with_policy(namespace, operation, input, parent, policy)
                    .await;
            }
        }
        if let Some(connection) = &self.connection {
            if connection.contains(&name) {
                return connection
                    .invoke_with_policy(namespace, operation, input, parent, policy)
                    .await;
            }
        }
        self.base
            .invoke_with_policy(namespace, operation, input, parent, policy)
            .await
    }

    fn contains(&self, name: &str) -> bool {
        self.session.as_ref().is_some_and(|s| s.contains(name))
            || self.connection.as_ref().is_some_and(|c| c.contains(name))
            || self.base.contains(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::context::CompositionAuthority;
    use crate::registry::registration::{make_handler, HandlerRegistration, OperationProvenance};
    use crate::registry::spec::{AccessControl, OperationSpec, OperationType, Visibility};
    use alknet_core::auth::Identity;
    use alknet_core::types::Capabilities;
    use std::time::{Duration, Instant};

    struct NoopEnv {
        contains_op: bool,
    }

    #[async_trait::async_trait]
    impl OperationEnv for NoopEnv {
        async fn invoke_with_policy(
            &self,
            _namespace: &str,
            _operation: &str,
            _input: Value,
            parent: &OperationContext,
            _policy: AbortPolicy,
        ) -> ResponseEnvelope {
            ResponseEnvelope::ok(parent.request_id.clone(), Value::String("noop".into()))
        }

        fn contains(&self, _name: &str) -> bool {
            self.contains_op
        }
    }

    fn echo_handler() -> crate::registry::registration::Handler {
        make_handler(
            |input, context| async move { ResponseEnvelope::ok(context.request_id, input) },
        )
    }

    fn inspect_handler() -> crate::registry::registration::Handler {
        make_handler(|_input, context| async move {
            let internal = context.is_internal();
            let id = context.identity.as_ref().map(|i| i.id.clone());
            let metadata_empty = context.metadata.is_empty();
            let parent_set = context.parent_request_id.is_some();
            ResponseEnvelope::ok(
                context.request_id,
                serde_json::json!({
                    "internal": internal,
                    "identity_id": id,
                    "metadata_empty": metadata_empty,
                    "parent_set": parent_set,
                }),
            )
        })
    }

    fn root_context(
        request_id: &str,
        identity: Option<Identity>,
        handler_identity: Option<CompositionAuthority>,
        scoped_env: ScopedOperationEnv,
        env: Arc<dyn OperationEnv + Send + Sync>,
    ) -> OperationContext {
        OperationContext {
            request_id: request_id.to_string(),
            parent_request_id: None,
            identity,
            handler_identity,
            capabilities: Capabilities::new(),
            metadata: HashMap::new(),
            scoped_env,
            env,
            abort_policy: AbortPolicy::default(),
            deadline: Some(Instant::now() + Duration::from_secs(30)),
            internal: false,
        }
    }

    fn registry_with(
        name: &str,
        spec_visibility: Visibility,
        handler: crate::registry::registration::Handler,
        composition_authority: Option<CompositionAuthority>,
        scoped_env: Option<ScopedOperationEnv>,
    ) -> Arc<OperationRegistry> {
        let mut registry = OperationRegistry::new();
        registry.register(HandlerRegistration::new(
            OperationSpec::new(
                name,
                OperationType::Query,
                spec_visibility,
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                AccessControl::default(),
            ),
            handler,
            OperationProvenance::Local,
            composition_authority,
            scoped_env,
            Capabilities::new(),
        ));
        Arc::new(registry)
    }

    #[tokio::test]
    async fn local_env_invoke_allowed_op_dispatches() {
        let registry = registry_with("echo/run", Visibility::External, echo_handler(), None, None);
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["echo/run"]);
        let ctx = root_context("root-1", None, None, scoped, env.clone());
        let response = env
            .invoke("echo", "run", serde_json::json!({"hi": 1}), &ctx)
            .await;
        assert!(response.result.is_ok());
        assert_eq!(response.result.unwrap(), serde_json::json!({"hi": 1}));
    }

    #[tokio::test]
    async fn local_env_invoke_disallowed_op_returns_not_found() {
        let registry = registry_with("echo/run", Visibility::External, echo_handler(), None, None);
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["other/op"]);
        let ctx = root_context("root-2", None, None, scoped, env.clone());
        let response = env.invoke("echo", "run", serde_json::json!({}), &ctx).await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn local_env_invoke_internal_op_dispatches_as_internal_call() {
        let registry = registry_with(
            "secret/op",
            Visibility::Internal,
            inspect_handler(),
            None,
            None,
        );
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["secret/op"]);
        let ctx = root_context("root-3", None, None, scoped, env.clone());
        let response = env
            .invoke("secret", "op", serde_json::json!({}), &ctx)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["internal"], Value::Bool(true));
        assert_eq!(out["parent_set"], Value::Bool(true));
    }

    #[tokio::test]
    async fn local_env_child_identity_is_parent_handler_identity() {
        let authority = CompositionAuthority::new("agent-chat", ["fs:read".to_string()]);
        let registry = registry_with(
            "child/run",
            Visibility::External,
            inspect_handler(),
            None,
            None,
        );
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["child/run"]);
        let ctx = root_context(
            "root-4",
            Some(Identity {
                id: "wire-caller".to_string(),
                scopes: vec![],
                resources: HashMap::new(),
            }),
            Some(authority.clone()),
            scoped,
            env.clone(),
        );
        let response = env
            .invoke("child", "run", serde_json::json!({}), &ctx)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["identity_id"], Value::String("agent-chat".into()));
    }

    #[tokio::test]
    async fn local_env_child_metadata_is_fresh_not_parent() {
        let registry = registry_with(
            "child/run",
            Visibility::External,
            inspect_handler(),
            None,
            None,
        );
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["child/run"]);
        let mut ctx = root_context("root-5", None, None, scoped, env.clone());
        ctx.metadata
            .insert("secret".to_string(), Value::String("leak".into()));
        let response = env
            .invoke("child", "run", serde_json::json!({}), &ctx)
            .await;
        let out = response.result.expect("ok");
        assert_eq!(out["metadata_empty"], Value::Bool(true));
    }

    struct ProbeEnv {
        name: String,
        contains_set: Vec<String>,
        dispatched: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl OperationEnv for ProbeEnv {
        async fn invoke_with_policy(
            &self,
            namespace: &str,
            operation: &str,
            _input: Value,
            parent: &OperationContext,
            _policy: AbortPolicy,
        ) -> ResponseEnvelope {
            *self.dispatched.lock().unwrap() = Some(format!("{namespace}/{operation}"));
            ResponseEnvelope::ok(parent.request_id.clone(), Value::String(self.name.clone()))
        }

        fn contains(&self, name: &str) -> bool {
            self.contains_set.iter().any(|n| n == name)
        }
    }

    #[tokio::test]
    async fn composite_env_dispatches_to_session_overlay_when_contains() {
        let base = Arc::new(NoopEnv { contains_op: true });
        let session = Arc::new(ProbeEnv {
            name: "session".to_string(),
            contains_set: vec!["agent/chat".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let composite = CompositeOperationEnv::new(base, None, Some(session.clone()));
        let env: Arc<dyn OperationEnv + Send + Sync> = Arc::new(composite);
        let scoped = ScopedOperationEnv::new(["agent/chat"]);
        let ctx = root_context("root-6", None, None, scoped, env.clone());
        let response = env
            .invoke("agent", "chat", serde_json::json!({}), &ctx)
            .await;
        assert_eq!(response.result.unwrap(), Value::String("session".into()));
        assert_eq!(
            session.dispatched.lock().unwrap().as_deref(),
            Some("agent/chat")
        );
    }

    #[tokio::test]
    async fn composite_env_falls_through_to_base_when_no_overlay_contains() {
        let base = Arc::new(ProbeEnv {
            name: "base".to_string(),
            contains_set: vec!["fs/readFile".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let session = Arc::new(ProbeEnv {
            name: "session".to_string(),
            contains_set: vec!["agent/chat".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let connection = Arc::new(ProbeEnv {
            name: "connection".to_string(),
            contains_set: vec!["worker/exec".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let composite = CompositeOperationEnv::new(base.clone(), Some(connection), Some(session));
        let env: Arc<dyn OperationEnv + Send + Sync> = Arc::new(composite);
        let scoped = ScopedOperationEnv::new(["fs/readFile"]);
        let ctx = root_context("root-7", None, None, scoped, env.clone());
        let response = env
            .invoke("fs", "readFile", serde_json::json!({}), &ctx)
            .await;
        assert_eq!(response.result.unwrap(), Value::String("base".into()));
        assert_eq!(
            base.dispatched.lock().unwrap().as_deref(),
            Some("fs/readFile")
        );
    }

    #[tokio::test]
    async fn composite_env_reachability_check_returns_not_found() {
        let base = Arc::new(NoopEnv { contains_op: true });
        let composite = CompositeOperationEnv::new(base, None, None);
        let env: Arc<dyn OperationEnv + Send + Sync> = Arc::new(composite);
        let scoped = ScopedOperationEnv::empty();
        let ctx = root_context("root-8", None, None, scoped, env.clone());
        let response = env
            .invoke("agent", "chat", serde_json::json!({}), &ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn composite_env_contains_aggregates_layers() {
        let base = Arc::new(ProbeEnv {
            name: "base".to_string(),
            contains_set: vec!["fs/readFile".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let session = Arc::new(ProbeEnv {
            name: "session".to_string(),
            contains_set: vec!["agent/chat".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let connection = Arc::new(ProbeEnv {
            name: "connection".to_string(),
            contains_set: vec!["worker/exec".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let composite = CompositeOperationEnv::new(base, Some(connection), Some(session));
        assert!(composite.contains("fs/readFile"));
        assert!(composite.contains("agent/chat"));
        assert!(composite.contains("worker/exec"));
        assert!(!composite.contains("unknown/op"));
    }

    #[tokio::test]
    async fn local_env_unknown_op_after_reachability_pass_returns_not_found() {
        let registry = Arc::new(OperationRegistry::new());
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["fs/readFile"]);
        let ctx = root_context("root-9", None, None, scoped, env.clone());
        let response = env
            .invoke("fs", "readFile", serde_json::json!({}), &ctx)
            .await;
        match response.result {
            Err(e) => assert_eq!(e.code, "NOT_FOUND"),
            other => panic!("expected NOT_FOUND, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn local_env_child_inherits_parent_deadline() {
        let registry = registry_with(
            "child/run",
            Visibility::External,
            inspect_handler(),
            None,
            None,
        );
        let env = Arc::new(LocalOperationEnv::new(Arc::clone(&registry)));
        let scoped = ScopedOperationEnv::new(["child/run"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ctx = root_context("root-10", None, None, scoped, env.clone());
        ctx.deadline = Some(deadline);
        let response = env
            .invoke("child", "run", serde_json::json!({}), &ctx)
            .await;
        assert!(response.result.is_ok());
    }

    #[test]
    fn local_env_default_contains_is_true() {
        let registry = Arc::new(OperationRegistry::new());
        let env = LocalOperationEnv::new(registry);
        assert!(env.contains("anything"));
        assert!(env.contains(""));
    }

    #[test]
    fn abort_policy_is_copy() {
        let p = AbortPolicy::default();
        let _ = p;
        let _ = p;
    }

    #[test]
    fn composition_authority_none_propagates_as_none_identity() {
        assert!(CompositionAuthority::none().is_none());
    }

    #[test]
    fn local_env_new_exposes_registry() {
        let registry = Arc::new(OperationRegistry::new());
        let env = LocalOperationEnv::new(Arc::clone(&registry));
        assert!(Arc::ptr_eq(env.registry(), &registry));
    }

    #[test]
    fn composite_env_accessors_return_refs() {
        let base: Arc<dyn OperationEnv + Send + Sync> = Arc::new(NoopEnv { contains_op: true });
        let session: Arc<dyn OperationEnv + Send + Sync> = Arc::new(NoopEnv { contains_op: true });
        let connection: Arc<dyn OperationEnv + Send + Sync> =
            Arc::new(NoopEnv { contains_op: false });
        let composite = CompositeOperationEnv::new(
            Arc::clone(&base),
            Some(Arc::clone(&connection)),
            Some(Arc::clone(&session)),
        );
        assert!(Arc::ptr_eq(composite.base(), &base));
        assert!(composite.session().is_some());
        assert!(composite.connection().is_some());
    }

    #[tokio::test]
    async fn composite_env_dispatches_to_connection_when_session_absent_or_missing() {
        let base = Arc::new(ProbeEnv {
            name: "base".to_string(),
            contains_set: vec![],
            dispatched: std::sync::Mutex::new(None),
        });
        let connection = Arc::new(ProbeEnv {
            name: "connection".to_string(),
            contains_set: vec!["worker/exec".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let session = Arc::new(ProbeEnv {
            name: "session".to_string(),
            contains_set: vec!["agent/chat".to_string()],
            dispatched: std::sync::Mutex::new(None),
        });
        let composite = CompositeOperationEnv::new(base, Some(connection.clone()), Some(session));
        let env: Arc<dyn OperationEnv + Send + Sync> = Arc::new(composite);
        let scoped = ScopedOperationEnv::new(["worker/exec"]);
        let ctx = root_context("root-11", None, None, scoped, env.clone());
        let response = env
            .invoke("worker", "exec", serde_json::json!({}), &ctx)
            .await;
        assert_eq!(response.result.unwrap(), Value::String("connection".into()));
        assert_eq!(
            connection.dispatched.lock().unwrap().as_deref(),
            Some("worker/exec")
        );
    }
}

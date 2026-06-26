use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use alknet_core::auth::Identity;
use alknet_core::types::Capabilities;
use serde_json::Value;

use super::env::OperationEnv;

pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,
    pub handler_identity: Option<CompositionAuthority>,
    pub capabilities: Capabilities,
    pub metadata: HashMap<String, Value>,
    pub scoped_env: ScopedOperationEnv,
    pub env: Arc<dyn OperationEnv + Send + Sync>,
    pub abort_policy: AbortPolicy,
    pub deadline: Option<Instant>,
    pub internal: bool,
}

impl OperationContext {
    pub fn is_internal(&self) -> bool {
        self.internal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AbortPolicy {
    #[default]
    AbortDependents,
    ContinueRunning,
}

#[derive(Debug, Clone)]
pub struct CompositionAuthority {
    pub label: String,
    pub scopes: Vec<String>,
    pub resources: HashMap<String, Vec<String>>,
}

impl CompositionAuthority {
    pub fn none() -> Option<Self> {
        None
    }

    pub fn new(label: &str, scopes: impl IntoIterator<Item = String>) -> Self {
        Self {
            label: label.to_string(),
            scopes: scopes.into_iter().collect(),
            resources: HashMap::new(),
        }
    }

    pub fn as_identity(&self) -> Option<Identity> {
        Some(Identity {
            id: self.label.clone(),
            scopes: self.scopes.clone(),
            resources: self.resources.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScopedOperationEnv {
    allowed: HashSet<String>,
}

impl ScopedOperationEnv {
    pub fn empty() -> Self {
        Self {
            allowed: HashSet::new(),
        }
    }

    pub fn new(ops: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: ops.into_iter().map(|s| s.into()).collect(),
        }
    }

    pub fn allows(&self, name: &str) -> bool {
        self.allowed.contains(name)
    }
}

impl Default for ScopedOperationEnv {
    fn default() -> Self {
        Self::empty()
    }
}

#[allow(dead_code)]
pub(crate) fn generate_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_env_allows_in_set() {
        let env = ScopedOperationEnv::new(["fs/readFile", "agent/chat"]);
        assert!(env.allows("fs/readFile"));
        assert!(env.allows("agent/chat"));
    }

    #[test]
    fn scoped_env_disallows_not_in_set() {
        let env = ScopedOperationEnv::new(["fs/readFile"]);
        assert!(!env.allows("agent/chat"));
        assert!(!env.allows(""));
    }

    #[test]
    fn scoped_env_empty_allows_nothing() {
        let env = ScopedOperationEnv::empty();
        assert!(!env.allows("fs/readFile"));
    }

    #[test]
    fn composition_authority_as_identity_correct() {
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["vastai".to_string()]);
        let authority = CompositionAuthority {
            label: "agent-chat".to_string(),
            scopes: vec!["llm:call".to_string(), "fs:read".to_string()],
            resources,
        };
        let identity = authority.as_identity().expect("as_identity returns Some");
        assert_eq!(identity.id, "agent-chat");
        assert_eq!(
            identity.scopes,
            vec!["llm:call".to_string(), "fs:read".to_string()]
        );
        assert_eq!(
            identity.resources.get("service"),
            Some(&vec!["vastai".to_string()])
        );
    }

    #[test]
    fn composition_authority_new_populates_label_and_scopes() {
        let authority = CompositionAuthority::new(
            "agent-chat",
            ["llm:call".to_string(), "fs:read".to_string()],
        );
        assert_eq!(authority.label, "agent-chat");
        assert_eq!(
            authority.scopes,
            vec!["llm:call".to_string(), "fs:read".to_string()]
        );
        assert!(authority.resources.is_empty());
    }

    #[test]
    fn composition_authority_none_is_none() {
        assert!(CompositionAuthority::none().is_none());
    }

    #[test]
    fn abort_policy_default_is_abort_dependents() {
        let policy = AbortPolicy::default();
        assert!(matches!(policy, AbortPolicy::AbortDependents));
    }

    #[test]
    fn generate_request_id_is_unique_and_non_deterministic() {
        let a = generate_request_id();
        let b = generate_request_id();
        assert_ne!(a, b);
        assert!(!a.is_empty());
    }
}

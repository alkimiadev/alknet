use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use alknet_core::auth::Identity;
use alknet_core::types::Capabilities;
use serde_json::Value;

use super::env::{OperationEnv, PeerId, PeerRef};

pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,
    pub handler_identity: Option<CompositionAuthority>,
    /// The original caller when this call was forwarded by a `from_call`
    /// handler (ADR-032). **Metadata only** — `AccessControl::check` never
    /// reads it; the ACL always authorizes `identity` (the direct caller).
    /// Handlers may read it for logging, auditing, per-user rate limiting,
    /// or application context. Populated from
    /// `call.requested.forwarded_for` by the dispatch path; set to `None`
    /// for composed children (wire-ingress only, not composition-ingress).
    /// The forwarder's claim, not a verified identity — a malicious hub can
    /// lie (same property as HTTP `X-Forwarded-For`). See ADR-032.
    pub forwarded_for: Option<Identity>,
    pub capabilities: Capabilities,
    pub metadata: HashMap<String, Value>,
    pub scoped_env: ScopedPeerEnv,
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
pub struct ScopedPeerEnv {
    /// Peer-agnostic reachability — reachable via `PeerRef::Any` or
    /// `PeerRef::Specific(any)`. The common case (peer-agnostic composition).
    pub allowed_ops: HashSet<String>,
    /// Peer-pinned reachability — `"peer-id/op-name"`, reachable only via
    /// `PeerRef::Specific(that peer)`. Additive to `allowed_ops`; opt-in for
    /// the disambiguation case (ADR-029 §4).
    pub peer_pinned: HashSet<String>,
}

impl ScopedPeerEnv {
    pub fn empty() -> Self {
        Self {
            allowed_ops: HashSet::new(),
            peer_pinned: HashSet::new(),
        }
    }

    pub fn new(ops: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed_ops: ops.into_iter().map(|s| s.into()).collect(),
            peer_pinned: HashSet::new(),
        }
    }

    /// Peer-pinned reachability: `"peer-id/op-name"`. Reachable only via
    /// `PeerRef::Specific(that peer)`. Additive to `new` — call `new` for the
    /// peer-agnostic set, then `with_pinned` for the pinned set.
    pub fn with_pinned(mut self, pinned: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.peer_pinned = pinned.into_iter().map(|s| s.into()).collect();
        self
    }

    /// Peer-agnostic reachability — unchanged from `ScopedOperationEnv::allows`.
    /// A name here is reachable via any routing path (`PeerRef::Any` or
    /// `Specific`).
    pub fn allows(&self, name: &str) -> bool {
        self.allowed_ops.contains(name)
    }

    /// Peer-pinned reachability — reachable only via `PeerRef::Specific(peer)`.
    /// The entry shape is `"peer-id/op-name"` (ADR-029 §4, OQ-33).
    pub fn allows_pinned(&self, peer: &PeerId, name: &str) -> bool {
        self.peer_pinned.contains(&format!("{peer}/{name}"))
    }

    /// Does this scoped env permit `name` via `peer`? Used by the reachability
    /// gate in `invoke_peer` / `invoke_with_policy`.
    /// - `PeerRef::Any` → `allows(name)`
    /// - `PeerRef::Specific(peer)` → `allows(name) || allows_pinned(peer, name)`
    pub fn allows_via(&self, peer: &PeerRef, name: &str) -> bool {
        match peer {
            PeerRef::Any => self.allows(name),
            PeerRef::Specific(p) => self.allows(name) || self.allows_pinned(p, name),
        }
    }
}

impl Default for ScopedPeerEnv {
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
        let env = ScopedPeerEnv::new(["fs/readFile", "agent/chat"]);
        assert!(env.allows("fs/readFile"));
        assert!(env.allows("agent/chat"));
    }

    #[test]
    fn scoped_env_disallows_not_in_set() {
        let env = ScopedPeerEnv::new(["fs/readFile"]);
        assert!(!env.allows("agent/chat"));
        assert!(!env.allows(""));
    }

    #[test]
    fn scoped_env_empty_allows_nothing() {
        let env = ScopedPeerEnv::empty();
        assert!(!env.allows("fs/readFile"));
    }

    #[test]
    fn scoped_peer_env_new_with_pinned_populates_both_fields() {
        let env = ScopedPeerEnv::new(["fs/readFile"]).with_pinned(["worker-a/container/exec"]);
        assert!(env.allowed_ops.contains("fs/readFile"));
        assert!(env.peer_pinned.contains("worker-a/container/exec"));
        assert!(!env.allowed_ops.contains("worker-a/container/exec"));
        assert!(!env.peer_pinned.contains("fs/readFile"));
    }

    #[test]
    fn scoped_peer_env_allows_checks_allowed_ops_only() {
        let env = ScopedPeerEnv::empty().with_pinned(["worker-a/container/exec"]);
        assert!(
            !env.allows("container/exec"),
            "pinned-only op not in allowed_ops"
        );
        let env2 = ScopedPeerEnv::new(["container/exec"]).with_pinned(["worker-a/container/exec"]);
        assert!(
            env2.allows("container/exec"),
            "op in allowed_ops is allowed"
        );
    }

    #[test]
    fn scoped_peer_env_allows_pinned_checks_peer_pinned_shape() {
        let env = ScopedPeerEnv::empty().with_pinned(["worker-a/container/exec"]);
        assert!(env.allows_pinned(&"worker-a".to_string(), "container/exec"));
        assert!(
            !env.allows_pinned(&"worker-b".to_string(), "container/exec"),
            "wrong peer"
        );
        assert!(
            !env.allows_pinned(&"worker-a".to_string(), "other/op"),
            "wrong op"
        );
    }

    #[test]
    fn scoped_peer_env_allows_via_any_uses_allowed_ops_only() {
        let env = ScopedPeerEnv::new(["fs/readFile"]).with_pinned(["worker-a/container/exec"]);
        assert!(
            env.allows_via(&PeerRef::Any, "fs/readFile"),
            "allowed op via Any"
        );
        assert!(
            !env.allows_via(&PeerRef::Any, "container/exec"),
            "pinned-only op NOT reachable via Any"
        );
    }

    #[test]
    fn scoped_peer_env_allows_via_specific_uses_allowed_ops_or_peer_pinned() {
        let env = ScopedPeerEnv::new(["fs/readFile"]).with_pinned(["worker-a/container/exec"]);
        assert!(
            env.allows_via(&PeerRef::Specific("worker-a".to_string()), "container/exec"),
            "pinned-only op reachable via Specific(pinned peer)"
        );
        assert!(
            env.allows_via(&PeerRef::Specific("worker-a".to_string()), "fs/readFile"),
            "allowed op reachable via Specific(any peer)"
        );
        assert!(
            !env.allows_via(&PeerRef::Specific("worker-b".to_string()), "container/exec"),
            "pinned-only op NOT reachable via Specific(wrong peer)"
        );
    }

    #[test]
    fn scoped_peer_env_op_in_both_sets_reachable_via_both_any_and_specific() {
        let env = ScopedPeerEnv::new(["container/exec"]).with_pinned(["worker-a/container/exec"]);
        assert!(
            env.allows_via(&PeerRef::Any, "container/exec"),
            "op in allowed_ops reachable via Any"
        );
        assert!(
            env.allows_via(&PeerRef::Specific("worker-a".to_string()), "container/exec"),
            "op in both sets reachable via Specific(peer)"
        );
        assert!(
            env.allows_via(&PeerRef::Specific("worker-b".to_string()), "container/exec"),
            "op in allowed_ops reachable via Specific(other peer) too"
        );
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

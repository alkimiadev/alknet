//! Ownership store: `OwnershipProvider` (sync read trait), `OwnershipStore`
//! (async write trait), `InMemoryOwnershipStore` default adapter, and
//! `OwnershipError`.
//!
//! See `docs/architecture/crates/core/auth.md` and ADR-050 for the full
//! specification. Runtime-spawned resources (containers, TTYs, workspace
//! processes) have derived ownership — whoever spawned the resource owns it.
//! The static `Identity.resources` model can't represent this (the resource
//! didn't exist when the identity was resolved), so `AccessControl::check`
//! (in alknet-call) consults `OwnershipProvider` at check time. This is the
//! fourth instance of the repo/adapter pattern (ADR-033).

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;

use crate::auth::Identity;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum OwnershipError {
    #[error("backend error: {message}")]
    Backend { message: String },
    #[error("not found: {entity}")]
    NotFound { entity: String },
}

/// Read side: consulted by `AccessControl::check` on the dispatch hot path.
/// Sync — called in the dispatch loop, no `.await`.
pub trait OwnershipProvider: Send + Sync + 'static {
    /// Does `identity` own `resource_type/resource_id` with `action`?
    /// The `action` parameter is accepted but not gated on — the base model
    /// is "owner can do anything they own." Per-action grants are a future
    /// extension; this preserves the door without building the mechanism.
    fn owns(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
        action: &str,
    ) -> bool;

    /// What resources of `resource_type` does `identity` own? Returns the
    /// set of resource IDs the caller owns, for the handler to filter
    /// against (the result-filter path, ADR-050 §4a).
    fn owned_resources(&self, identity: &Identity, resource_type: &str) -> Vec<String>;

    /// Does `identity` own *any* resource of `resource_type`? The scope-gate
    /// path (ADR-050 §4a).
    fn owns_any(&self, identity: &Identity, resource_type: &str) -> bool;
}

/// Write side: called by the handler that manages the resource lifecycle.
/// Async — not on the dispatch hot path. The handler calls `record` on
/// spawn and `revoke` on teardown (ADR-050 §4b — handler-driven, not a
/// reaper). The trait takes `&self` so it can be shared as
/// `Arc<dyn OwnershipStore>` (interior mutability via `RwLock`).
#[async_trait]
pub trait OwnershipStore: Send + Sync + 'static {
    /// Record that `identity` spawned `resource_type/resource_id`.
    async fn record(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError>;

    /// Revoke ownership of `resource_type/resource_id`. Called by the
    /// handler on resource teardown (ADR-050 §4b).
    async fn revoke(&self, resource_type: &str, resource_id: &str) -> Result<(), OwnershipError>;
}

pub struct InMemoryOwnershipStore {
    inner: RwLock<HashMap<(String, String), Identity>>,
}

impl InMemoryOwnershipStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryOwnershipStore {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnershipProvider for InMemoryOwnershipStore {
    fn owns(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
        _action: &str,
    ) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .get(&(resource_type.to_string(), resource_id.to_string()))
            .map(|owner| owner.id == identity.id)
            .unwrap_or(false)
    }

    fn owned_resources(&self, identity: &Identity, resource_type: &str) -> Vec<String> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .iter()
            .filter(|((rt, _), owner)| rt == resource_type && owner.id == identity.id)
            .map(|((_, rid), _)| rid.clone())
            .collect()
    }

    fn owns_any(&self, identity: &Identity, resource_type: &str) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .iter()
            .any(|((rt, _), owner)| rt == resource_type && owner.id == identity.id)
    }
}

#[async_trait]
impl OwnershipStore for InMemoryOwnershipStore {
    async fn record(
        &self,
        identity: &Identity,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<(), OwnershipError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.insert(
            (resource_type.to_string(), resource_id.to_string()),
            identity.clone(),
        );
        Ok(())
    }

    /// Revoking a non-existent resource is a no-op: returns `Ok(())`. This
    /// mirrors `InMemoryCredentialStore::delete`, which treats a missing
    /// entry as success. Teardown paths are idempotent — a handler may call
    /// `revoke` on a resource that was already removed (e.g. crashed and
    /// re-entered cleanup), and should not error in that case.
    async fn revoke(&self, resource_type: &str, resource_id: &str) -> Result<(), OwnershipError> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.remove(&(resource_type.to_string(), resource_id.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_identity(id: &str) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: vec![],
            resources: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn record_owns_revoke_round_trip() {
        let store = InMemoryOwnershipStore::new();
        let owner = make_identity("worker-a");

        assert!(!store.owns(&owner, "container", "c1", "exec"));
        assert!(!store.owns_any(&owner, "container"));
        assert!(store.owned_resources(&owner, "container").is_empty());

        store.record(&owner, "container", "c1").await.unwrap();
        assert!(store.owns(&owner, "container", "c1", "exec"));
        assert!(store.owns(&owner, "container", "c1", "logs"));
        assert!(store.owns_any(&owner, "container"));
        assert_eq!(store.owned_resources(&owner, "container"), vec!["c1"]);

        store.revoke("container", "c1").await.unwrap();
        assert!(!store.owns(&owner, "container", "c1", "exec"));
        assert!(!store.owns_any(&owner, "container"));
        assert!(store.owned_resources(&owner, "container").is_empty());
    }

    #[tokio::test]
    async fn owned_resources_returns_all_for_owner_with_multiple() {
        let store = InMemoryOwnershipStore::new();
        let owner = make_identity("worker-a");
        store.record(&owner, "container", "c1").await.unwrap();
        store.record(&owner, "container", "c2").await.unwrap();
        store.record(&owner, "container", "c3").await.unwrap();

        let mut owned = store.owned_resources(&owner, "container");
        owned.sort();
        assert_eq!(owned, vec!["c1", "c2", "c3"]);
    }

    #[tokio::test]
    async fn owned_resources_filters_by_resource_type() {
        let store = InMemoryOwnershipStore::new();
        let owner = make_identity("worker-a");
        store.record(&owner, "container", "c1").await.unwrap();
        store.record(&owner, "tty", "t1").await.unwrap();

        let owned_containers = store.owned_resources(&owner, "container");
        assert_eq!(owned_containers, vec!["c1"]);
        let owned_ttys = store.owned_resources(&owner, "tty");
        assert_eq!(owned_ttys, vec!["t1"]);
    }

    #[tokio::test]
    async fn owns_any_returns_false_for_owner_with_no_resources_of_type() {
        let store = InMemoryOwnershipStore::new();
        let owner = make_identity("worker-a");
        store.record(&owner, "container", "c1").await.unwrap();

        assert!(store.owns_any(&owner, "container"));
        assert!(!store.owns_any(&owner, "tty"));
    }

    #[tokio::test]
    async fn revoke_on_non_existent_resource_is_no_op() {
        let store = InMemoryOwnershipStore::new();
        store.revoke("container", "never-existed").await.unwrap();
    }

    #[tokio::test]
    async fn owns_returns_false_for_different_identity() {
        let store = InMemoryOwnershipStore::new();
        let owner = make_identity("worker-a");
        let other = make_identity("worker-b");
        store.record(&owner, "container", "c1").await.unwrap();

        assert!(store.owns(&owner, "container", "c1", "exec"));
        assert!(!store.owns(&other, "container", "c1", "exec"));
        assert!(!store.owns_any(&other, "container"));
        assert!(store.owned_resources(&other, "container").is_empty());
    }

    #[tokio::test]
    async fn record_replaces_existing_owner() {
        let store = InMemoryOwnershipStore::new();
        let owner_a = make_identity("worker-a");
        let owner_b = make_identity("worker-b");
        store.record(&owner_a, "container", "c1").await.unwrap();
        store.record(&owner_b, "container", "c1").await.unwrap();

        assert!(!store.owns(&owner_a, "container", "c1", "exec"));
        assert!(store.owns(&owner_b, "container", "c1", "exec"));
    }

    #[tokio::test]
    async fn default_is_empty_store() {
        let store = InMemoryOwnershipStore::default();
        let owner = make_identity("worker-a");
        assert!(store.owned_resources(&owner, "container").is_empty());
        assert!(!store.owns_any(&owner, "container"));
    }

    #[test]
    fn ownership_error_display_formatting() {
        let backend = OwnershipError::Backend {
            message: "disk full".to_string(),
        };
        assert_eq!(backend.to_string(), "backend error: disk full");

        let not_found = OwnershipError::NotFound {
            entity: "container:c1".to_string(),
        };
        assert_eq!(not_found.to_string(), "not found: container:c1");
    }

    #[test]
    fn ownership_error_is_non_exhaustive() {
        let err = OwnershipError::Backend {
            message: "x".to_string(),
        };
        let _ = err.to_string();
    }
}

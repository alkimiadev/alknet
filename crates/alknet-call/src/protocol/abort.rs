//! Abort cascade logic for nested calls (ADR-016).
//!
//! When `call.aborted` arrives for a parent request, the protocol cascades
//! the abort to all non-terminal descendants in the call tree. The default
//! policy is `abort-dependents`; `continue-running` is an opt-in for
//! long-running work that should survive a parent's abort.
//!
//! The call tree is indexed by `parent_request_id` in the
//! `PendingRequestMap`. The root request has `parent_request_id: None`;
//! each composed call has `parent_request_id: Some(parent.request_id)`.
//! Composed child request IDs are internal — they appear in the map for
//! abort-cascade indexing but are not sent as `call.requested` to any
//! peer. The client only sees `call.aborted` for the root ID it sent; the
//! server cascades internally to descendants.

use super::pending::PendingRequestMap;
use crate::registry::context::AbortPolicy;

pub struct AbortCascade<'a> {
    pending: &'a mut PendingRequestMap,
}

impl<'a> AbortCascade<'a> {
    pub fn new(pending: &'a mut PendingRequestMap) -> Self {
        Self { pending }
    }

    /// Cascade an abort from the given request ID to all non-terminal
    /// descendants in the call tree. Returns the list of descendant
    /// request IDs that were aborted (for logging/auditing), sorted for
    /// determinism. The root request itself is not touched by this
    /// method — the caller is responsible for aborting the root (the
    /// trigger of the cascade).
    ///
    /// Under `AbortDependents` (default): all descendants are aborted,
    /// regardless of whether they have started.
    ///
    /// Under `ContinueRunning`: only descendants that have not started
    /// are aborted; started descendants continue to completion. No new
    /// descendants start (the parent is gone). This is the conservative
    /// approximation noted in ADR-016: a descendant is "started" if
    /// `PendingEntry::started` is true (the handler has begun
    /// executing). A `call.aborted` for an unknown request ID is
    /// silently discarded — `cascade_abort` on an unknown root returns
    /// an empty list and removes nothing.
    pub fn cascade_abort(&mut self, root_request_id: &str, policy: AbortPolicy) -> Vec<String> {
        if !self.pending.contains(root_request_id) {
            return Vec::new();
        }

        let descendants = self.find_descendants(root_request_id);

        let mut aborted = Vec::new();
        match policy {
            AbortPolicy::AbortDependents => {
                for id in &descendants {
                    if self.pending.handle_aborted(id) {
                        aborted.push(id.clone());
                    }
                }
            }
            AbortPolicy::ContinueRunning => {
                for id in &descendants {
                    let started = self.pending.is_started(id).unwrap_or(false);
                    if !started && self.pending.handle_aborted(id) {
                        aborted.push(id.clone());
                    }
                }
            }
        }

        aborted.sort();
        aborted
    }

    /// Find all descendants of a request ID in the call tree by walking
    /// the `parent_request_id` index. Returns descendants in
    /// breadth-first order with each level's children sorted for
    /// determinism. The root itself is not included in the result.
    fn find_descendants(&self, parent_id: &str) -> Vec<String> {
        let mut descendants = Vec::new();
        let mut frontier: Vec<String> = vec![parent_id.to_string()];

        while let Some(current) = frontier.pop() {
            let mut children: Vec<String> = self
                .pending
                .request_ids()
                .into_iter()
                .filter(|id| {
                    self.pending
                        .parent_of(id)
                        .flatten()
                        .is_some_and(|p| p == current)
                })
                .collect();
            children.sort();
            for child in children {
                descendants.push(child.clone());
                frontier.push(child);
            }
        }

        descendants
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::wire::CallError;
    use std::time::{Duration, Instant};

    fn register_call(map: &mut PendingRequestMap, id: &str, parent: Option<&str>) {
        map.register_call(
            id.to_string(),
            Instant::now() + Duration::from_secs(30),
            parent.map(|p| p.to_string()),
        );
    }

    fn register_subscribe(map: &mut PendingRequestMap, id: &str, parent: Option<&str>) {
        map.register_subscribe(id.to_string(), None, parent.map(|p| p.to_string()));
    }

    #[test]
    fn cascade_abort_unknown_root_returns_empty_and_is_noop() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("does-not-exist", AbortPolicy::AbortDependents);
        assert!(aborted.is_empty());
        assert!(cascade.pending.contains("r1"));
    }

    #[test]
    fn cascade_abort_abort_dependents_aborts_all_descendants() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-b", Some("r1"));
        register_call(&mut map, "r1-a-1", Some("r1-a"));
        register_call(&mut map, "r1-a-2", Some("r1-a"));
        register_call(&mut map, "r1-b-1", Some("r1-b"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);

        assert_eq!(
            aborted,
            vec![
                "r1-a".to_string(),
                "r1-a-1".to_string(),
                "r1-a-2".to_string(),
                "r1-b".to_string(),
                "r1-b-1".to_string(),
            ]
        );
        assert!(cascade.pending.contains("r1"));
        assert!(!cascade.pending.contains("r1-a"));
        assert!(!cascade.pending.contains("r1-b"));
        assert!(!cascade.pending.contains("r1-a-1"));
        assert!(!cascade.pending.contains("r1-a-2"));
        assert!(!cascade.pending.contains("r1-b-1"));
    }

    #[test]
    fn cascade_abort_continue_running_aborts_only_unstarted_descendants() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-b", Some("r1"));
        register_call(&mut map, "r1-a-1", Some("r1-a"));

        map.mark_started("r1-a");
        // r1-b and r1-a-1 are unstarted

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::ContinueRunning);

        assert_eq!(aborted, vec!["r1-a-1".to_string(), "r1-b".to_string()]);
        assert!(cascade.pending.contains("r1"));
        assert!(cascade.pending.contains("r1-a"));
        assert!(!cascade.pending.contains("r1-b"));
        assert!(!cascade.pending.contains("r1-a-1"));
    }

    #[test]
    fn cascade_abort_continue_running_aborts_all_when_none_started() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-b", Some("r1"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::ContinueRunning);

        assert_eq!(aborted, vec!["r1-a".to_string(), "r1-b".to_string()]);
        assert!(!cascade.pending.contains("r1-a"));
        assert!(!cascade.pending.contains("r1-b"));
    }

    #[test]
    fn cascade_abort_depth_three_aborts_all_descendants() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "root", None);
        register_call(&mut map, "root-a", Some("root"));
        register_call(&mut map, "root-b", Some("root"));
        register_call(&mut map, "root-a-1", Some("root-a"));
        register_call(&mut map, "root-a-2", Some("root-a"));
        register_call(&mut map, "root-a-1-x", Some("root-a-1"));
        register_call(&mut map, "root-a-1-y", Some("root-a-1"));
        register_call(&mut map, "root-b-1", Some("root-b"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("root", AbortPolicy::AbortDependents);

        assert_eq!(
            aborted,
            vec![
                "root-a".to_string(),
                "root-a-1".to_string(),
                "root-a-1-x".to_string(),
                "root-a-1-y".to_string(),
                "root-a-2".to_string(),
                "root-b".to_string(),
                "root-b-1".to_string(),
            ]
        );
        assert!(cascade.pending.contains("root"));
        assert_eq!(cascade.pending.len(), 1);
    }

    #[test]
    fn cascade_abort_root_with_no_descendants_returns_empty() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "lonely", None);

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("lonely", AbortPolicy::AbortDependents);
        assert!(aborted.is_empty());
        assert!(cascade.pending.contains("lonely"));
    }

    #[test]
    fn cascade_abort_only_aborts_descendants_not_siblings() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r2", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r2-a", Some("r2"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);

        assert_eq!(aborted, vec!["r1-a".to_string()]);
        assert!(cascade.pending.contains("r1"));
        assert!(cascade.pending.contains("r2"));
        assert!(cascade.pending.contains("r2-a"));
        assert!(!cascade.pending.contains("r1-a"));
    }

    #[test]
    fn cascade_abort_handles_mixed_call_and_subscribe_entries() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_subscribe(&mut map, "r1-sub", Some("r1"));
        register_call(&mut map, "r1-sub-child", Some("r1-sub"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);

        assert_eq!(
            aborted,
            vec!["r1-sub".to_string(), "r1-sub-child".to_string(),]
        );
        assert!(cascade.pending.contains("r1"));
        assert_eq!(cascade.pending.len(), 1);
    }

    #[test]
    fn cascade_abort_continue_running_with_started_descendant_keeps_its_unstarted_children() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-a-1", Some("r1-a"));

        map.mark_started("r1-a");
        // r1-a is started and continues; r1-a-1 is unstarted.
        // Under ContinueRunning, r1-a-1 is aborted (conservative: still pending).

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::ContinueRunning);

        assert_eq!(aborted, vec!["r1-a-1".to_string()]);
        assert!(cascade.pending.contains("r1-a"));
        assert!(!cascade.pending.contains("r1-a-1"));
    }

    #[test]
    fn cascade_abort_abort_dependents_aborts_started_descendants_too() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-b", Some("r1"));

        map.mark_started("r1-a");
        map.mark_started("r1-b");

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);

        assert_eq!(aborted, vec!["r1-a".to_string(), "r1-b".to_string()]);
        assert!(!cascade.pending.contains("r1-a"));
        assert!(!cascade.pending.contains("r1-b"));
    }

    #[test]
    fn find_descendants_does_not_include_root() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));

        let cascade = AbortCascade::new(&mut map);
        let descendants = cascade.find_descendants("r1");
        assert_eq!(descendants, vec!["r1-a".to_string()]);
        assert!(!descendants.contains(&"r1".to_string()));
    }

    #[test]
    fn cascade_abort_default_policy_is_abort_dependents() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        map.mark_started("r1-a");

        let mut cascade = AbortCascade::new(&mut map);
        let aborted_default = cascade.cascade_abort("r1", AbortPolicy::default());
        assert_eq!(aborted_default, vec!["r1-a".to_string()]);
    }

    #[test]
    fn cascade_abort_does_not_remove_root() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));

        let mut cascade = AbortCascade::new(&mut map);
        let _ = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);
        assert!(cascade.pending.contains("r1"));
    }

    #[test]
    fn cascade_abort_returns_sorted_descendants_for_determinism() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-z", Some("r1"));
        register_call(&mut map, "r1-a", Some("r1"));
        register_call(&mut map, "r1-m", Some("r1"));

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::AbortDependents);
        assert_eq!(
            aborted,
            vec!["r1-a".to_string(), "r1-m".to_string(), "r1-z".to_string(),]
        );
    }

    #[test]
    fn unknown_request_id_silently_discarded_no_panic() {
        let mut map = PendingRequestMap::new();
        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("totally-unknown", AbortPolicy::AbortDependents);
        assert!(aborted.is_empty());
    }

    #[test]
    fn cascade_abort_continue_running_started_descendant_survives() {
        let mut map = PendingRequestMap::new();
        register_call(&mut map, "r1", None);
        register_call(&mut map, "r1-a", Some("r1"));
        map.mark_started("r1-a");

        let mut cascade = AbortCascade::new(&mut map);
        let aborted = cascade.cascade_abort("r1", AbortPolicy::ContinueRunning);
        assert!(aborted.is_empty());
        assert!(cascade.pending.contains("r1-a"));
    }

    #[test]
    fn cascade_abort_handles_call_error_unused() {
        let _ = CallError::internal("unused");
    }
}

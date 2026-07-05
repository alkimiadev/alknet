//! Operation specifications: `OperationSpec`, `OperationType`, `Visibility`,
//! `ErrorDefinition`, and `AccessControl`.
//!
//! See `docs/architecture/crates/call/operation-registry.md` for the full
//! specification.

use alknet_core::auth::Identity;
use alknet_core::ownership::OwnershipProvider;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Query,
    Mutation,
    Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    External,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorDefinition {
    pub code: String,
    pub description: String,
    pub schema: Value,
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessResult {
    Allowed,
    Forbidden(String),
}

impl AccessResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, AccessResult::Allowed)
    }
}

impl AccessControl {
    pub fn has_restrictions(&self) -> bool {
        !self.required_scopes.is_empty()
            || self.required_scopes_any.is_some()
            || self.resource_type.is_some()
            || self.resource_action.is_some()
    }

    pub fn check(
        &self,
        identity: Option<&Identity>,
        resource_id: Option<&str>,
        ownership: Option<&dyn OwnershipProvider>,
    ) -> AccessResult {
        if !self.has_restrictions() {
            return AccessResult::Allowed;
        }
        let identity = match identity {
            Some(id) => id,
            None => return AccessResult::Forbidden("authentication required".to_string()),
        };

        for scope in &self.required_scopes {
            if !identity.scopes.iter().any(|s| s == scope) {
                return AccessResult::Forbidden(format!("missing required scope: {scope}"));
            }
        }

        if let Some(any) = &self.required_scopes_any {
            let has_one = any.iter().any(|s| identity.scopes.iter().any(|i| i == s));
            if !has_one {
                return AccessResult::Forbidden(
                    "missing required scope (any of: ".to_string() + &any.join(", ") + ")",
                );
            }
        }

        if let Some(p) = ownership {
            if let Some(rt) = &self.resource_type {
                match resource_id {
                    Some(rid) => {
                        let action = self.resource_action.as_deref().unwrap_or("");
                        if !p.owns(identity, rt, rid, action) {
                            return AccessResult::Forbidden(format!(
                                "not owner of resource: {rt}/{rid}"
                            ));
                        }
                    }
                    None => {
                        if !p.owns_any(identity, rt) {
                            return AccessResult::Forbidden(format!(
                                "no owned resources of type: {rt}"
                            ));
                        }
                    }
                }
                return AccessResult::Allowed;
            }
        }

        if let Some(rt) = &self.resource_type {
            let allowed = identity.resources.get(rt);
            match &self.resource_action {
                Some(action) => match allowed {
                    Some(actions) if actions.iter().any(|a| a == action) => {}
                    _ => {
                        return AccessResult::Forbidden(format!("missing resource: {rt}/{action}"))
                    }
                },
                None => match allowed {
                    Some(actions) if !actions.is_empty() => {}
                    _ => return AccessResult::Forbidden(format!("missing resource: {rt}")),
                },
            }
        } else if let Some(action) = &self.resource_action {
            let found = identity
                .resources
                .values()
                .any(|actions| actions.iter().any(|a| a == action));
            if !found {
                return AccessResult::Forbidden(format!("missing resource action: {action}"));
            }
        }

        AccessResult::Allowed
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OperationSpec {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub visibility: Visibility,
    pub input_schema: Value,
    pub output_schema: Value,
    pub error_schemas: Vec<ErrorDefinition>,
    pub access_control: AccessControl,
    /// JSON pointer into the input for the resource ID, when
    /// `access_control.resource_type` is set and the operation targets a
    /// specific runtime-spawned resource (ADR-050). e.g. `"$.containerId"`
    /// for `docker/container/exec`. Absent for no-specific-resource
    /// operations (the `list` case). `None` for operations with no
    /// `resource_type` or with static resource sets.
    pub resource_id_path: Option<String>,
}

impl OperationSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        op_type: OperationType,
        visibility: Visibility,
        input_schema: Value,
        output_schema: Value,
        error_schemas: Vec<ErrorDefinition>,
        access_control: AccessControl,
        resource_id_path: Option<String>,
    ) -> Self {
        let name = name.into();
        let namespace = name
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        Self {
            name,
            namespace,
            op_type,
            visibility,
            input_schema,
            output_schema,
            error_schemas,
            access_control,
            resource_id_path,
        }
    }

    pub fn path(&self) -> String {
        format!("/{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn identity(scopes: &[&str], resources: &[(&str, &[&str])]) -> Identity {
        let mut res = HashMap::new();
        for (k, v) in resources {
            res.insert(
                (*k).to_string(),
                v.iter().map(|s| (*s).to_string()).collect(),
            );
        }
        Identity {
            id: "caller".to_string(),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
            resources: res,
        }
    }

    #[test]
    fn path_has_leading_slash() {
        let spec = OperationSpec::new(
            "fs/readFile",
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        );
        assert_eq!(spec.path(), "/fs/readFile");
    }

    #[test]
    fn namespace_derived_from_name() {
        let spec = OperationSpec::new(
            "agent/chat",
            OperationType::Subscription,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        );
        assert_eq!(spec.namespace, "agent");
        assert_eq!(spec.name, "agent/chat");
    }

    #[test]
    fn namespace_for_single_segment() {
        let spec = OperationSpec::new(
            "list",
            OperationType::Query,
            Visibility::Internal,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        );
        assert_eq!(spec.namespace, "list");
    }

    #[test]
    fn resource_id_path_defaults_to_none() {
        let spec = OperationSpec::new(
            "fs/readFile",
            OperationType::Query,
            Visibility::External,
            serde_json::json!({}),
            serde_json::json!({}),
            vec![],
            AccessControl::default(),
            None,
        );
        assert_eq!(spec.resource_id_path, None);
    }

    #[test]
    fn empty_access_control_allowed_for_all() {
        let acl = AccessControl::default();
        assert_eq!(acl.check(None, None, None), AccessResult::Allowed);
        let id = identity(&[], &[]);
        assert_eq!(acl.check(Some(&id), None, None), AccessResult::Allowed);
    }

    #[test]
    fn none_identity_with_restrictions_forbidden() {
        let acl = AccessControl {
            required_scopes: vec!["read".to_string()],
            ..Default::default()
        };
        assert_eq!(
            acl.check(None, None, None),
            AccessResult::Forbidden("authentication required".to_string())
        );

        let acl2 = AccessControl {
            required_scopes_any: Some(vec!["read".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            acl2.check(None, None, None),
            AccessResult::Forbidden("authentication required".to_string())
        );

        let acl3 = AccessControl {
            resource_type: Some("service".to_string()),
            ..Default::default()
        };
        assert_eq!(
            acl3.check(None, None, None),
            AccessResult::Forbidden("authentication required".to_string())
        );
    }

    #[test]
    fn required_scopes_and_checked() {
        let acl = AccessControl {
            required_scopes: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        };
        let id_missing = identity(&["a"], &[]);
        assert!(matches!(
            acl.check(Some(&id_missing), None, None),
            AccessResult::Forbidden(_)
        ));
        let id_ok = identity(&["a", "b", "c"], &[]);
        assert_eq!(acl.check(Some(&id_ok), None, None), AccessResult::Allowed);
    }

    #[test]
    fn required_scopes_any_or_checked() {
        let acl = AccessControl {
            required_scopes_any: Some(vec!["x".to_string(), "y".to_string()]),
            ..Default::default()
        };
        let id_x = identity(&["x"], &[]);
        assert_eq!(acl.check(Some(&id_x), None, None), AccessResult::Allowed);
        let id_y = identity(&["y"], &[]);
        assert_eq!(acl.check(Some(&id_y), None, None), AccessResult::Allowed);
        let id_none = identity(&["z"], &[]);
        assert!(matches!(
            acl.check(Some(&id_none), None, None),
            AccessResult::Forbidden(_)
        ));
    }

    #[test]
    fn resource_check_with_type_and_action() {
        let acl = AccessControl {
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
            ..Default::default()
        };
        let id_ok = identity(&[], &[("service", &["read"])]);
        assert_eq!(acl.check(Some(&id_ok), None, None), AccessResult::Allowed);
        let id_missing_action = identity(&[], &[("service", &["write"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_action), None, None),
            AccessResult::Forbidden(_)
        ));
        let id_missing_type = identity(&[], &[("other", &["read"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_type), None, None),
            AccessResult::Forbidden(_)
        ));
    }

    #[test]
    fn combined_scopes_and_resources() {
        let acl = AccessControl {
            required_scopes: vec!["admin".to_string()],
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
            ..Default::default()
        };
        let id_ok = identity(&["admin"], &[("service", &["read"])]);
        assert_eq!(acl.check(Some(&id_ok), None, None), AccessResult::Allowed);
        let id_missing_scope = identity(&["user"], &[("service", &["read"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_scope), None, None),
            AccessResult::Forbidden(_)
        ));
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

    #[test]
    fn ownership_provider_allows_owned_resource() {
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        let id = empty_identity("alice");
        let provider = MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        };
        assert_eq!(
            acl.check(
                Some(&id),
                Some("c1"),
                Some(&provider as &dyn OwnershipProvider)
            ),
            AccessResult::Allowed
        );
    }

    #[test]
    fn ownership_provider_forbids_unowned_resource() {
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        let id = empty_identity("alice");
        let provider = MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        };
        assert!(matches!(
            acl.check(
                Some(&id),
                Some("c2"),
                Some(&provider as &dyn OwnershipProvider)
            ),
            AccessResult::Forbidden(_)
        ));
    }

    #[test]
    fn ownership_provider_forbids_none_identity() {
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        let provider = MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        };
        assert!(matches!(
            acl.check(None, Some("c1"), Some(&provider as &dyn OwnershipProvider)),
            AccessResult::Forbidden(_)
        ));
    }

    #[test]
    fn ownership_provider_list_allowed_when_owns_any() {
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        let id = empty_identity("alice");
        let provider = MockOwnership {
            owned: vec![("container".to_string(), "c1".to_string())],
        };
        assert_eq!(
            acl.check(Some(&id), None, Some(&provider as &dyn OwnershipProvider)),
            AccessResult::Allowed
        );
    }

    #[test]
    fn ownership_provider_list_forbidden_when_not_owns_any() {
        let acl = AccessControl {
            resource_type: Some("container".to_string()),
            resource_action: Some("exec".to_string()),
            ..Default::default()
        };
        let id = empty_identity("alice");
        let provider = MockOwnership {
            owned: vec![("volume".to_string(), "v1".to_string())],
        };
        assert!(matches!(
            acl.check(Some(&id), None, Some(&provider as &dyn OwnershipProvider)),
            AccessResult::Forbidden(_)
        ));
    }

    #[test]
    fn ownership_none_falls_back_to_static() {
        let acl = AccessControl {
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
            ..Default::default()
        };
        let id_ok = identity(&[], &[("service", &["read"])]);
        assert_eq!(acl.check(Some(&id_ok), None, None), AccessResult::Allowed);
        let id_missing = identity(&[], &[("service", &["write"])]);
        assert!(matches!(
            acl.check(Some(&id_missing), None, None),
            AccessResult::Forbidden(_)
        ));
    }
}

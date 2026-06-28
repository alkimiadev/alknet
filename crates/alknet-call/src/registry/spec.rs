//! Operation specifications: `OperationSpec`, `OperationType`, `Visibility`,
//! `ErrorDefinition`, and `AccessControl`.
//!
//! See `docs/architecture/crates/call/operation-registry.md` for the full
//! specification.

use alknet_core::auth::Identity;
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

    pub fn check(&self, identity: Option<&Identity>) -> AccessResult {
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
}

impl OperationSpec {
    pub fn new(
        name: impl Into<String>,
        op_type: OperationType,
        visibility: Visibility,
        input_schema: Value,
        output_schema: Value,
        error_schemas: Vec<ErrorDefinition>,
        access_control: AccessControl,
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
        );
        assert_eq!(spec.namespace, "list");
    }

    #[test]
    fn empty_access_control_allowed_for_all() {
        let acl = AccessControl::default();
        assert_eq!(acl.check(None), AccessResult::Allowed);
        let id = identity(&[], &[]);
        assert_eq!(acl.check(Some(&id)), AccessResult::Allowed);
    }

    #[test]
    fn none_identity_with_restrictions_forbidden() {
        let acl = AccessControl {
            required_scopes: vec!["read".to_string()],
            ..Default::default()
        };
        assert_eq!(
            acl.check(None),
            AccessResult::Forbidden("authentication required".to_string())
        );

        let acl2 = AccessControl {
            required_scopes_any: Some(vec!["read".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            acl2.check(None),
            AccessResult::Forbidden("authentication required".to_string())
        );

        let acl3 = AccessControl {
            resource_type: Some("service".to_string()),
            ..Default::default()
        };
        assert_eq!(
            acl3.check(None),
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
            acl.check(Some(&id_missing)),
            AccessResult::Forbidden(_)
        ));
        let id_ok = identity(&["a", "b", "c"], &[]);
        assert_eq!(acl.check(Some(&id_ok)), AccessResult::Allowed);
    }

    #[test]
    fn required_scopes_any_or_checked() {
        let acl = AccessControl {
            required_scopes_any: Some(vec!["x".to_string(), "y".to_string()]),
            ..Default::default()
        };
        let id_x = identity(&["x"], &[]);
        assert_eq!(acl.check(Some(&id_x)), AccessResult::Allowed);
        let id_y = identity(&["y"], &[]);
        assert_eq!(acl.check(Some(&id_y)), AccessResult::Allowed);
        let id_none = identity(&["z"], &[]);
        assert!(matches!(
            acl.check(Some(&id_none)),
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
        assert_eq!(acl.check(Some(&id_ok)), AccessResult::Allowed);
        let id_missing_action = identity(&[], &[("service", &["write"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_action)),
            AccessResult::Forbidden(_)
        ));
        let id_missing_type = identity(&[], &[("other", &["read"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_type)),
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
        assert_eq!(acl.check(Some(&id_ok)), AccessResult::Allowed);
        let id_missing_scope = identity(&["user"], &[("service", &["read"])]);
        assert!(matches!(
            acl.check(Some(&id_missing_scope)),
            AccessResult::Forbidden(_)
        ));
    }
}

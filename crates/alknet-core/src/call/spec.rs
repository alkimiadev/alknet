//! Operation specifications (type, access control) for the call protocol.
//!
//! See [ADR-025](docs/architecture/decisions/025-operation-spec.md) and
//! [ADR-033](docs/architecture/decisions/033-call-protocol-extensions.md).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum OperationType {
    Query,
    Mutation,
    Subscription,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControl {
    pub required_scopes: Vec<String>,
    pub required_scopes_any: Option<Vec<String>>,
    pub resource_type: Option<String>,
    pub resource_action: Option<String>,
}

impl AccessControl {
    pub fn check(&self, identity: &crate::auth::Identity) -> bool {
        for scope in &self.required_scopes {
            if !identity.scopes.contains(scope) {
                return false;
            }
        }
        if let Some(any) = &self.required_scopes_any {
            if !any.iter().any(|s| identity.scopes.contains(s)) {
                return false;
            }
        }
        if let Some(res_type) = &self.resource_type {
            if let Some(actions) = identity.resources.get(res_type) {
                if let Some(action) = &self.resource_action {
                    if !actions.contains(action) {
                        return false;
                    }
                }
            } else {
                return false;
            }
        }
        true
    }

    pub fn has_restrictions(&self) -> bool {
        !self.required_scopes.is_empty()
            || self.required_scopes_any.is_some()
            || self.resource_type.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSpec {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub input_schema: Value,
    pub output_schema: Value,
    pub access_control: AccessControl,
}

impl OperationSpec {
    pub fn path(&self) -> String {
        format!("/{}", self.name)
    }

    pub fn namespace_from_name(name: &str) -> String {
        let trimmed = name.trim_start_matches('/');
        let parts: Vec<&str> = trimmed.split('/').collect();
        match parts.len() {
            n if n >= 3 => parts[1].to_string(),
            n if n >= 2 => parts[0].to_string(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_identity(
        scopes: Vec<String>,
        resources: HashMap<String, Vec<String>>,
    ) -> crate::auth::Identity {
        crate::auth::Identity {
            id: "test".to_string(),
            scopes,
            resources,
        }
    }

    #[test]
    fn access_control_allows_matching_scopes() {
        let ac = AccessControl {
            required_scopes: vec!["read".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let id = make_identity(vec!["read".to_string()], HashMap::new());
        assert!(ac.check(&id));
    }

    #[test]
    fn access_control_rejects_missing_scopes() {
        let ac = AccessControl {
            required_scopes: vec!["admin".to_string()],
            required_scopes_any: None,
            resource_type: None,
            resource_action: None,
        };
        let id = make_identity(vec!["read".to_string()], HashMap::new());
        assert!(!ac.check(&id));
    }

    #[test]
    fn access_control_required_scopes_any_matches() {
        let ac = AccessControl {
            required_scopes: vec![],
            required_scopes_any: Some(vec!["admin".to_string(), "read".to_string()]),
            resource_type: None,
            resource_action: None,
        };
        let id = make_identity(vec!["read".to_string()], HashMap::new());
        assert!(ac.check(&id));
    }

    #[test]
    fn access_control_required_scopes_any_rejects() {
        let ac = AccessControl {
            required_scopes: vec![],
            required_scopes_any: Some(vec!["admin".to_string()]),
            resource_type: None,
            resource_action: None,
        };
        let id = make_identity(vec!["read".to_string()], HashMap::new());
        assert!(!ac.check(&id));
    }

    #[test]
    fn access_control_resource_check_matches() {
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["read".to_string()]);
        let ac = AccessControl {
            required_scopes: vec![],
            required_scopes_any: None,
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
        };
        let id = make_identity(vec![], resources);
        assert!(ac.check(&id));
    }

    #[test]
    fn access_control_resource_check_missing_resource_type() {
        let ac = AccessControl {
            required_scopes: vec![],
            required_scopes_any: None,
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
        };
        let id = make_identity(vec![], HashMap::new());
        assert!(!ac.check(&id));
    }

    #[test]
    fn access_control_resource_check_missing_action() {
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["write".to_string()]);
        let ac = AccessControl {
            required_scopes: vec![],
            required_scopes_any: None,
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
        };
        let id = make_identity(vec![], resources);
        assert!(!ac.check(&id));
    }

    #[test]
    fn access_control_combined_scopes_and_resources() {
        let mut resources = HashMap::new();
        resources.insert("service".to_string(), vec!["read".to_string()]);
        let ac = AccessControl {
            required_scopes: vec!["relay:connect".to_string()],
            required_scopes_any: Some(vec!["admin".to_string()]),
            resource_type: Some("service".to_string()),
            resource_action: Some("read".to_string()),
        };
        let id = make_identity(
            vec!["relay:connect".to_string(), "admin".to_string()],
            resources,
        );
        assert!(ac.check(&id));
    }

    #[test]
    fn operation_type_variants() {
        assert_eq!(OperationType::Query, OperationType::Query);
        assert_ne!(OperationType::Query, OperationType::Mutation);
        assert_ne!(OperationType::Mutation, OperationType::Subscription);
    }

    #[test]
    fn operation_spec_namespace_from_name() {
        assert_eq!(OperationSpec::namespace_from_name("/auth/verify"), "auth");
        assert_eq!(OperationSpec::namespace_from_name("/fs/readFile"), "fs");
        assert_eq!(
            OperationSpec::namespace_from_name("/head/agent/chat"),
            "agent"
        );
    }

    #[test]
    fn operation_spec_path() {
        let spec = OperationSpec {
            name: "auth/verify".to_string(),
            namespace: "auth".to_string(),
            op_type: OperationType::Query,
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            access_control: AccessControl {
                required_scopes: vec![],
                required_scopes_any: None,
                resource_type: None,
                resource_action: None,
            },
        };
        assert_eq!(spec.path(), "/auth/verify");
    }
}

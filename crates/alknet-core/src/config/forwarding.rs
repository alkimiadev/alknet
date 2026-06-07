use std::net::IpAddr;
use std::ops::Range;
use std::str::FromStr;

use ipnetwork::IpNetwork;

use crate::auth::identity::Identity;
use crate::transport::TransportKind;

#[derive(Debug, Clone, PartialEq)]
pub enum ForwardingAction {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetPattern {
    Any,
    Host(String),
    Cidr(IpNetwork),
    PortRange(String, Range<u16>),
    AlknetPrefix,
}

impl TargetPattern {
    pub fn matches(&self, target: &str, port: u16) -> bool {
        match self {
            TargetPattern::Any => true,
            TargetPattern::Host(pattern) => match_host_pattern(pattern, target),
            TargetPattern::Cidr(network) => match_cidr(network, target),
            TargetPattern::PortRange(host_pattern, port_range) => {
                match_host_pattern(host_pattern, target) && port_range.contains(&port)
            }
            TargetPattern::AlknetPrefix => {
                target.starts_with(crate::server::control_channel::ALKNET_PREFIX)
            }
        }
    }
}

fn match_host_pattern(pattern: &str, target: &str) -> bool {
    if pattern == target {
        return true;
    }
    if pattern.contains('*') {
        if let Some(pos) = pattern.find('*') {
            let prefix = &pattern[..pos];
            let suffix = &pattern[pos + 1..];
            return target.starts_with(prefix)
                && target.ends_with(suffix)
                && target.len() >= prefix.len() + suffix.len();
        }
    }
    false
}

fn match_cidr(network: &IpNetwork, target: &str) -> bool {
    let Ok(addr) = IpAddr::from_str(target) else {
        return false;
    };
    network.contains(addr)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForwardingRule {
    pub target: TargetPattern,
    pub action: ForwardingAction,
    pub principals: Vec<String>,
    pub transports: Vec<TransportKind>,
}

impl ForwardingRule {
    fn matches_principal(&self, identity: &Identity) -> bool {
        if self.principals.is_empty() {
            return true;
        }
        self.principals
            .iter()
            .any(|p| p == &identity.id || identity.scopes.contains(p))
    }

    fn matches_transport(&self, transport: &TransportKind) -> bool {
        if self.transports.is_empty() {
            return true;
        }
        self.transports.contains(transport)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForwardingPolicy {
    pub default: ForwardingAction,
    pub rules: Vec<ForwardingRule>,
}

impl ForwardingPolicy {
    pub fn allow_all() -> Self {
        Self {
            default: ForwardingAction::Allow,
            rules: Vec::new(),
        }
    }

    pub fn deny_all() -> Self {
        Self {
            default: ForwardingAction::Deny,
            rules: Vec::new(),
        }
    }

    pub fn check(
        &self,
        target: &str,
        port: u16,
        identity: &Identity,
        transport: TransportKind,
    ) -> bool {
        for rule in &self.rules {
            if rule.target.matches(target, port)
                && rule.matches_principal(identity)
                && rule.matches_transport(&transport)
            {
                return rule.action == ForwardingAction::Allow;
            }
        }
        self.default == ForwardingAction::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_identity(id: &str, scopes: Vec<&str>) -> Identity {
        Identity {
            id: id.to_string(),
            scopes: scopes.into_iter().map(|s| s.to_string()).collect(),
            resources: HashMap::new(),
        }
    }

    #[test]
    fn forwarding_action_equality() {
        assert_eq!(ForwardingAction::Allow, ForwardingAction::Allow);
        assert_eq!(ForwardingAction::Deny, ForwardingAction::Deny);
        assert_ne!(ForwardingAction::Allow, ForwardingAction::Deny);
    }

    #[test]
    fn allow_all_allows_everything() {
        let policy = ForwardingPolicy::allow_all();
        let identity = make_identity("user1", vec![]);
        assert!(policy.check("example.com", 80, &identity, TransportKind::Tcp));
        assert!(policy.check(
            "10.0.0.1",
            22,
            &identity,
            TransportKind::Tls { server_name: None }
        ));
    }

    #[test]
    fn deny_all_denies_everything() {
        let policy = ForwardingPolicy::deny_all();
        let identity = make_identity("user1", vec![]);
        assert!(!policy.check("example.com", 80, &identity, TransportKind::Tcp));
        assert!(!policy.check(
            "10.0.0.1",
            22,
            &identity,
            TransportKind::Tls { server_name: None }
        ));
    }

    #[test]
    fn first_match_wins_allowlist() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Host("allowed.example.com".to_string()),
                action: ForwardingAction::Allow,
                principals: vec![],
                transports: vec![],
            }],
        };
        let identity = make_identity("user1", vec![]);
        assert!(policy.check("allowed.example.com", 80, &identity, TransportKind::Tcp));
        assert!(!policy.check("denied.example.com", 80, &identity, TransportKind::Tcp));
    }

    #[test]
    fn first_match_wins_blocklist() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Allow,
            rules: vec![ForwardingRule {
                target: TargetPattern::Host("blocked.example.com".to_string()),
                action: ForwardingAction::Deny,
                principals: vec![],
                transports: vec![],
            }],
        };
        let identity = make_identity("user1", vec![]);
        assert!(!policy.check("blocked.example.com", 80, &identity, TransportKind::Tcp));
        assert!(policy.check("allowed.example.com", 80, &identity, TransportKind::Tcp));
    }

    #[test]
    fn first_match_wins_ordering() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![
                ForwardingRule {
                    target: TargetPattern::Any,
                    action: ForwardingAction::Allow,
                    principals: vec![],
                    transports: vec![],
                },
                ForwardingRule {
                    target: TargetPattern::Host("blocked.example.com".to_string()),
                    action: ForwardingAction::Deny,
                    principals: vec![],
                    transports: vec![],
                },
            ],
        };
        let identity = make_identity("user1", vec![]);
        assert!(policy.check("blocked.example.com", 80, &identity, TransportKind::Tcp));
    }

    #[test]
    fn empty_principals_matches_all() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Any,
                action: ForwardingAction::Allow,
                principals: vec![],
                transports: vec![],
            }],
        };
        let identity1 = make_identity("user1", vec![]);
        let identity2 = make_identity("user2", vec![]);
        assert!(policy.check("example.com", 80, &identity1, TransportKind::Tcp));
        assert!(policy.check("example.com", 80, &identity2, TransportKind::Tcp));
    }

    #[test]
    fn principal_matching_by_id() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Any,
                action: ForwardingAction::Allow,
                principals: vec!["SHA256:abc123".to_string()],
                transports: vec![],
            }],
        };
        let allowed = make_identity("SHA256:abc123", vec![]);
        let denied = make_identity("SHA256:other", vec![]);
        assert!(policy.check("example.com", 80, &allowed, TransportKind::Tcp));
        assert!(!policy.check("example.com", 80, &denied, TransportKind::Tcp));
    }

    #[test]
    fn principal_matching_by_scope() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Any,
                action: ForwardingAction::Allow,
                principals: vec!["admin".to_string()],
                transports: vec![],
            }],
        };
        let allowed = make_identity("user1", vec!["admin"]);
        let denied = make_identity("user2", vec!["viewer"]);
        assert!(policy.check("example.com", 80, &allowed, TransportKind::Tcp));
        assert!(!policy.check("example.com", 80, &denied, TransportKind::Tcp));
    }

    #[test]
    fn empty_transports_matches_all() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Any,
                action: ForwardingAction::Allow,
                principals: vec![],
                transports: vec![],
            }],
        };
        let identity = make_identity("user1", vec![]);
        assert!(policy.check("example.com", 80, &identity, TransportKind::Tcp));
        assert!(policy.check(
            "example.com",
            80,
            &identity,
            TransportKind::Tls { server_name: None }
        ));
        assert!(policy.check(
            "example.com",
            80,
            &identity,
            TransportKind::Iroh {
                endpoint_id: String::new()
            }
        ));
    }

    #[test]
    fn transport_matching() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Any,
                action: ForwardingAction::Allow,
                principals: vec![],
                transports: vec![TransportKind::Tls { server_name: None }],
            }],
        };
        let identity = make_identity("user1", vec![]);
        assert!(!policy.check("example.com", 443, &identity, TransportKind::Tcp));
        assert!(policy.check(
            "example.com",
            443,
            &identity,
            TransportKind::Tls { server_name: None }
        ));
    }

    #[test]
    fn target_pattern_any_matches_all() {
        let pattern = TargetPattern::Any;
        assert!(pattern.matches("example.com", 80));
        assert!(pattern.matches("10.0.0.1", 22));
        assert!(pattern.matches("alknet-control", 0));
    }

    #[test]
    fn target_pattern_host_exact_match() {
        let pattern = TargetPattern::Host("example.com".to_string());
        assert!(pattern.matches("example.com", 80));
        assert!(!pattern.matches("other.com", 80));
        assert!(!pattern.matches("sub.example.com", 80));
    }

    #[test]
    fn target_pattern_host_glob_match() {
        let pattern = TargetPattern::Host("*.example.com".to_string());
        assert!(pattern.matches("sub.example.com", 80));
        assert!(pattern.matches("a.example.com", 443));
        assert!(!pattern.matches("example.com", 80));
        assert!(!pattern.matches("xsub.example.com.org", 80));
    }

    #[test]
    fn target_pattern_host_glob_prefix() {
        let pattern = TargetPattern::Host("db-*".to_string());
        assert!(pattern.matches("db-primary", 5432));
        assert!(pattern.matches("db-replica", 5432));
        assert!(!pattern.matches("web-primary", 5432));
    }

    #[test]
    fn target_pattern_host_glob_suffix() {
        let pattern = TargetPattern::Host("*.internal".to_string());
        assert!(pattern.matches("app.internal", 8080));
        assert!(pattern.matches("db.internal", 5432));
        assert!(!pattern.matches("app.external", 80));
    }

    #[test]
    fn target_pattern_cidr_matches_ip() {
        let network: IpNetwork = "10.0.0.0/8".parse().unwrap();
        let pattern = TargetPattern::Cidr(network);
        assert!(pattern.matches("10.0.0.1", 22));
        assert!(pattern.matches("10.255.255.255", 22));
        assert!(!pattern.matches("192.168.1.1", 22));
        assert!(!pattern.matches("not-an-ip", 22));
    }

    #[test]
    fn target_pattern_cidr_ipv6() {
        let network: IpNetwork = "fd00::/8".parse().unwrap();
        let pattern = TargetPattern::Cidr(network);
        assert!(pattern.matches("fd00::1", 22));
        assert!(!pattern.matches("10.0.0.1", 22));
    }

    #[test]
    fn target_pattern_port_range_matches() {
        let pattern = TargetPattern::PortRange("localhost".to_string(), 8080..8090);
        assert!(pattern.matches("localhost", 8080));
        assert!(pattern.matches("localhost", 8085));
        assert!(pattern.matches("localhost", 8089));
        assert!(!pattern.matches("localhost", 8079));
        assert!(!pattern.matches("localhost", 8090));
        assert!(!pattern.matches("otherhost", 8080));
    }

    #[test]
    fn target_pattern_port_range_with_glob() {
        let pattern = TargetPattern::PortRange("*.internal".to_string(), 3000..4000);
        assert!(pattern.matches("app.internal", 3000));
        assert!(pattern.matches("app.internal", 3999));
        assert!(!pattern.matches("app.internal", 2999));
        assert!(!pattern.matches("app.internal", 4000));
        assert!(!pattern.matches("app.external", 3000));
    }

    #[test]
    fn target_pattern_alknet_prefix() {
        let pattern = TargetPattern::AlknetPrefix;
        assert!(pattern.matches("alknet-control", 0));
        assert!(pattern.matches("alknet-status", 0));
        assert!(pattern.matches("alknet-", 0));
        assert!(!pattern.matches("example.com", 0));
        assert!(!pattern.matches("alknet.example.com", 0));
    }

    #[test]
    fn default_fallthrough_allow() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Allow,
            rules: vec![],
        };
        let identity = make_identity("user1", vec![]);
        assert!(policy.check("anything", 80, &identity, TransportKind::Tcp));
    }

    #[test]
    fn default_fallthrough_deny() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![],
        };
        let identity = make_identity("user1", vec![]);
        assert!(!policy.check("anything", 80, &identity, TransportKind::Tcp));
    }

    #[test]
    fn combined_principal_and_transport_matching() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Deny,
            rules: vec![ForwardingRule {
                target: TargetPattern::Host("restricted.example.com".to_string()),
                action: ForwardingAction::Allow,
                principals: vec!["admin".to_string()],
                transports: vec![TransportKind::Tls { server_name: None }],
            }],
        };
        let admin = make_identity("admin-user", vec!["admin"]);
        let viewer = make_identity("viewer-user", vec!["viewer"]);
        assert!(policy.check(
            "restricted.example.com",
            443,
            &admin,
            TransportKind::Tls { server_name: None }
        ));
        assert!(!policy.check("restricted.example.com", 443, &admin, TransportKind::Tcp));
        assert!(!policy.check(
            "restricted.example.com",
            443,
            &viewer,
            TransportKind::Tls { server_name: None }
        ));
    }

    #[test]
    fn webtransport_restricted_to_alknet() {
        let policy = ForwardingPolicy {
            default: ForwardingAction::Allow,
            rules: vec![
                ForwardingRule {
                    target: TargetPattern::AlknetPrefix,
                    action: ForwardingAction::Allow,
                    principals: vec![],
                    transports: vec![TransportKind::WebTransport {
                        host: String::new(),
                    }],
                },
                ForwardingRule {
                    target: TargetPattern::Any,
                    action: ForwardingAction::Deny,
                    principals: vec![],
                    transports: vec![TransportKind::WebTransport {
                        host: String::new(),
                    }],
                },
            ],
        };
        let identity = make_identity("user1", vec![]);
        assert!(policy.check(
            "alknet-control",
            0,
            &identity,
            TransportKind::WebTransport {
                host: String::new()
            }
        ));
        assert!(!policy.check(
            "example.com",
            443,
            &identity,
            TransportKind::WebTransport {
                host: String::new()
            }
        ));
        assert!(policy.check("example.com", 443, &identity, TransportKind::Tcp));
    }

    #[test]
    fn cidr_does_not_match_hostname() {
        let network: IpNetwork = "10.0.0.0/8".parse().unwrap();
        let pattern = TargetPattern::Cidr(network);
        assert!(!pattern.matches("example.com", 22));
    }
}

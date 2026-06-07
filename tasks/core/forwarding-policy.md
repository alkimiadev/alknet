---
id: core/forwarding-policy
name: Implement ForwardingPolicy with rule-based allow/deny
status: pending
depends_on:
  - core/identity-type-provider
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Implement `ForwardingPolicy` with rule-based allow/deny for `channel_open_direct_tcpip` targets, per ADR-031 and configuration.md.

Currently, any authenticated client can open a `direct-tcpip` channel to any destination. `ForwardingPolicy` adds access control: rules are evaluated in order, first match wins, and a default action handles the fallthrough case.

**Key additions**:
- `ForwardingPolicy` struct: `{ default: ForwardingAction, rules: Vec<ForwardingRule> }`
- `ForwardingAction` enum: `Allow` | `Deny`
- `ForwardingRule` struct: `{ target: TargetPattern, action: ForwardingAction, principals: Vec<String>, transports: Vec<TransportKind> }`
- `TargetPattern` enum: `Any`, `Host(String)`, `Cidr(IpNetwork)`, `PortRange(String, Range<u16>)`
- Policy evaluation method: `ForwardingPolicy::check(&self, target: &str, port: u16, identity: &Identity, transport: TransportKind) -> bool`

**Key changes**:
- `ServerHandler::channel_open_direct_tcpip()` currently spawns a proxy task for any non-reserved destination. After this task, it evaluates `ForwardingPolicy::check()` before proxying.
- `DynamicConfig` gains a `forwarding` field of type `Arc<ForwardingPolicy>` (already defined in config task, initially `ForwardingPolicy::allow_all()`)
- Default `ForwardingPolicy::allow_all()` preserves current behavior (migration compatibility per ADR-031)
- `ForwardingPolicy::deny_all()` for production deployments

**Depends on identity-type-provider** because `ForwardingPolicy::check()` takes `&Identity` to match against `principals` (which maps to `Identity.id`).

## Acceptance Criteria

- [ ] `ForwardingPolicy`, `ForwardingAction`, `ForwardingRule`, `TargetPattern` types defined in `crates/alknet-core/src/config/forwarding.rs`
- [ ] `ForwardingPolicy::allow_all()` and `ForwardingPolicy::deny_all()` constructors
- [ ] `ForwardingPolicy::check()` evaluates rules in order, first match wins, falls through to default
- [ ] Empty `principals` field matches all identities (no principal filter)
- [ ] Empty `transports` field matches all transport kinds
- [ ] `TargetPattern::Host` supports glob matching (e.g., `*.example.com`)
- [ ] `TargetPattern::Cidr` matches IP addresses within CIDR ranges
- [ ] `TargetPattern::PortRange` matches hosts with port ranges
- [ ] `ServerHandler::channel_open_direct_tcpip()` calls `ForwardingPolicy::check()` before proxying; denies with log message if policy rejects
- [ ] Reserved `alknet-*` destinations bypass forwarding policy (internal routing, per ADR-018)
- [ ] All existing tests pass (default `allow_all()` preserves current behavior)
- [ ] New tests: policy evaluation with various rules, principal matching, transport matching, default fallthrough

## References

- docs/architecture/decisions/031-forwarding-policy.md — ADR-031, type definitions, evaluation order
- docs/architecture/configuration.md — ForwardingPolicy in DynamicConfig
- docs/architecture/identity.md — Identity.scopes used by ForwardingPolicy
- crates/alknet-core/src/server/handler.rs — channel_open_direct_tcpip() where policy check goes

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
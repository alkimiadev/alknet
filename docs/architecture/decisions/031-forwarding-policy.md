# ADR-031: Forwarding Policy

## Status

Accepted

## Context

Currently, any authenticated client can open a `direct-tcpip` SSH channel to
any destination. The only gate is authentication — once authenticated, a client
has unrestricted network access through the tunnel. This is a security gap: a
compromised key grants unrestricted access.

Operators need the ability to:
- Restrict which hosts and ports authenticated clients can access
- Apply different rules to different principals (key fingerprints, accounts)
- Restrict WebTransport clients to alknet control channels only
- Set a default policy (allow-all for migration compatibility, deny-all for
  production)

## Decision

**Add `ForwardingPolicy` as part of `DynamicConfig` (reloadable without
restart).**

### Type Definitions

```rust
pub struct ForwardingPolicy {
    pub default: ForwardingAction,
    pub rules: Vec<ForwardingRule>,
}

pub struct ForwardingRule {
    pub target: TargetPattern,
    pub action: ForwardingAction,
    pub principals: Vec<String>,   // Empty = matches all
    pub transports: Vec<TransportKind>,  // Empty = matches all
}

pub enum ForwardingAction {
    Allow,
    Deny,
}

pub enum TargetPattern {
    Any,
    Host(String),          // "localhost", "*.example.com"
    Cidr(IpNetwork),       // "10.0.0.0/8"
    PortRange(String, Range<u16>),  // "localhost", ports 8080-8090
    AlknetPrefix,          // Matches alknet-* control channels
}
```

### Rule Evaluation

Rules are evaluated in order. First match wins. If no rule matches, the default
applies. This supports both allowlist and blocklist semantics:

- **Allowlist**: `default: Deny`, then explicit Allow rules for permitted
  destinations.
- **Blocklist**: `default: Allow`, then explicit Deny rules for blocked
  destinations.

### Principals

Each rule can specify which principals it applies to. A principal is an
`Identity.id` (fingerprint or UUID) or a scope from `Identity.scopes`. When the
rule's `principals` field is empty, it matches all identities.

This connects to the `IdentityProvider` trait (ADR-029): when a client
authenticates, the `Identity` is resolved, and the forwarding policy checks
rules against `Identity.id` and `Identity.scopes`.

### TransportKind-Aware Rules

Each rule can specify which `TransportKind` it applies to. This enables
transport-specific restrictions — for example, WebTransport clients can be
restricted to `alknet-*` control channels only:

```rust
ForwardingRule {
    target: TargetPattern::AlknetPrefix,
    action: ForwardingAction::Allow,
    principals: vec![],
    transports: vec![TransportKind::WebTransport { host: "*".into() }],
}
```

### Where the Policy Check Happens

The forwarding policy check occurs in `channel_open_direct_tcpip` before the
proxy task is spawned. The current behavior (no check) is equivalent to
`ForwardingPolicy::allow_all()` — default Allow, no rules. This preserves
backward compatibility during migration.

### DynamicConfig Integration

`ForwardingPolicy` is part of `DynamicConfig` and reloadable via
`ConfigReloadHandle::reload()` or NAPI's `reloadForwarding()`. Changes take
effect on the next channel open — existing connections continue with their
current policy.

### OQ Resolutions

- **OQ-12** (Per-user forwarding scope vs global rules): Resolved. Start with
  global rules + principal matching from `Identity.scopes`. Per-user scope
  from `peer_credentials.metadata.scopes` via `IdentityProvider`.
- **OQ-16** (Transport-specific forwarding): Resolved. Add `TransportKind`
  match in `ForwardingRule`. WebTransport clients can be restricted.
- **OQ-18** (Source of Identity.scopes): Resolved by ADR-029 and this ADR.
  `IdentityProvider` owns scopes. `ForwardingPolicy` consumes them.

## Consequences

- **Positive**: Operators can restrict access per identity, per destination, per
  transport. A compromised key no longer grants unrestricted network access.
- **Positive**: Default-allow preserves current behavior during migration. Switch
  to default-deny for production deployments.
- **Positive**: Policy is reloadable without restart. Adding a rule via
  `reloadForwarding()` takes effect on the next channel open.
- **Positive**: `TransportKind`-aware rules enable transport-specific
  restrictions (e.g., WebTransport clients restricted to alknet-* channels).
- **Negative**: Another check in the hot path (every `channel_open_direct_tcpip`
  call). The cost is a linear scan of rules — acceptable for small rule sets.
  Large rule sets should use compiled matchers (future optimization).
- **Negative**: `TargetPattern` string matching is lenient. Host patterns like
  `*.example.com` require careful implementation to prevent bypasses. The
  `glob` or `globset` crate can handle this correctly.

## References

- [research/configuration.md](../../research/configuration.md) — ForwardingPolicy section
- [auth.md](../auth.md) — Identity.scopes and IdentityProvider
- [open-questions.md](../open-questions.md) — OQ-12, OQ-16, OQ-18
- [ADR-029](029-identity-core-type.md) — Identity as core type
- [ADR-030](030-static-dynamic-config-split.md) — DynamicConfig (ForwardingPolicy is part of it)
- [integration-plan.md](../../research/integration-plan.md) — Phase 1.3
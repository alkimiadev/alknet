---
id: architecture/adr-031-forwarding-policy
name: Write ADR-031 — Forwarding policy
status: completed
depends_on: []
scope: narrow
risk: low
impact: phase
level: implementation
---

## Description

Write ADR-031 establishing the forwarding policy model for `channel_open_direct_tcpip` access control.

Currently any authenticated client can open a channel to any destination. This ADR defines `ForwardingPolicy`, `ForwardingRule`, and `TargetPattern` as part of `DynamicConfig` (reloadable without restart).

Key design decisions from the research:
- Default-allow for migration compatibility (preserves current behavior)
- Default-deny is recommended for production
- Rules are evaluated per-channel-open, matched against the authenticated `Identity` from `IdentityProvider`
- `TransportKind` match in rules enables transport-specific restrictions (e.g., WebTransport clients restricted to alknet-* channels)
- OQ-12 resolved: start with global rules + principal matching from Identity.scopes; per-user scope from peer_credentials.metadata.scopes via IdentityProvider
- OQ-16 resolved: add TransportKind match in ForwardingRule; WebTransport clients can be scoped
- OQ-18 resolved: IdentityProvider owns scopes, ForwardingPolicy consumes them

## Acceptance Criteria

- [ ] `docs/architecture/decisions/031-forwarding-policy.md` exists
- [ ] ADR follows established format
- [ ] Context explains the security gap: any authenticated client gets unrestricted access
- [ ] Decision states: ForwardingPolicy with allow/deny rules, TargetPattern matching, default-allow for migration, TransportKind-aware rules, ForwardingPolicy is part of DynamicConfig (reloadable), Identity.scopes consumed by policy
- [ ] Includes ForwardingRule and TargetPattern type signatures
- [ ] Consequences: operators can restrict access per identity, per destination, per transport; default-allow preserves backward compatibility
- [ ] Resolves OQ-12, OQ-16, OQ-18 (reference in ADR)
- [ ] References: research/configuration.md, auth.md, open-questions.md

## References

- docs/research/configuration.md — ForwardingPolicy section
- docs/architecture/auth.md — Identity.scopes and IdentityProvider
- docs/architecture/open-questions.md — OQ-12, OQ-16, OQ-18
- docs/research/integration-plan.md — ADR 031 entry, Phase 1.3

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
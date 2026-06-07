---
id: architecture/spec-update-open-questions
name: Update open-questions.md — resolve questions per ADR decisions
status: pending
depends_on:
  - architecture/adr-031-forwarding-policy
  - architecture/adr-029-identity-core-type
  - architecture/adr-028-auth-irpc-service
  - architecture/adr-030-static-dynamic-config-split
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Update `docs/architecture/open-questions.md` to record resolutions for the open questions that the new ADRs address.

**Questions to resolve**:
- **OQ-12** (Per-user forwarding scope vs global rules): Resolved per ADR-031 — start with global rules + principal matching. Per-user scope from peer_credentials.metadata.scopes via IdentityProvider.
- **OQ-16** (Transport-specific forwarding policy): Resolved per ADR-031 — add TransportKind match in ForwardingRule. WebTransport clients can be restricted to alknet-* channels.
- **OQ-18** (Source of Identity.scopes): Resolved per ADR-029 and ADR-031 — IdentityProvider owns scopes, ForwardingPolicy uses scopes from Identity.
- **OQ-22** (Client streaming in call protocol): Resolved per integration plan — defer. Current model (single request, optional streaming response) covers all identified use cases.
- **New** (irpc dependency: always or feature flag?): Resolved per ADR-027 — feature flag. Nodes that only do SSH tunneling don't need the service layer.
- **New** (DNS control channel scope): Resolved per ADR-026 — call protocol frames only (no SSH over DNS).
- **New** (alknet-storage and alknet-secret irpc dependency): Resolved per ADR-027 — independently.

**Questions that remain open** (deferred):
- **OQ-15** (TLS + WebTransport + iroh QUIC coexistence): Deferred to Phase 4 per integration plan.
- **OQ-19** (Separate TLS identity for WebTransport): Deferred to Phase 4.
- **OQ-20** (Worker registration and discovery): Still open per integration plan. Register on connect, cleanup on disconnect is the leading approach but needs spec in call-protocol.md.

## Acceptance Criteria

- [ ] OQ-12 marked as resolved with ADR-031 reference
- [ ] OQ-16 marked as resolved with ADR-031 reference
- [ ] OQ-18 marked as resolved with ADR-029/ADR-031 reference
- [ ] OQ-22 marked as resolved (deferred) with note
- [ ] New OQ (irpc feature flag) added and resolved with ADR-027 reference
- [ ] New OQ (DNS control channel scope) added and resolved with ADR-026 reference
- [ ] New OQ (storage/secret irpc dep) added and resolved with ADR-027 reference
- [ ] OQ-15, OQ-19, OQ-20 remain open with notes on deferral
- [ ] `last_updated` in YAML frontmatter updated
- [ ] Format consistent with existing resolved entries (strikethrough priority, ADR reference)

## References

- docs/architecture/open-questions.md — current content
- docs/research/integration-plan.md — "Open Questions to Resolve Before Phase 1" section

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
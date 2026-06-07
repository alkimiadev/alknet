---
id: architecture/spec-update-napi
name: Update napi-and-pubsub.md — add reload API and call protocol references
status: pending
depends_on:
  - architecture/spec-configuration
  - architecture/adr-033-operationenv-irpc-call-protocol
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Update `docs/architecture/napi-and-pubsub.md` to add the NAPI reload API (reloadAuth, reloadForwarding, reloadAll) and references to the call protocol integration.

The current spec is reviewed and stable. Changes are additive:

1. Add `reloadAuth()`, `reloadForwarding()`, `reloadAll()` methods to `AlknetServer` interface
2. Add note about call protocol integration: NAPI consumers can register operation handlers
3. Add note about irpc service creation for NAPI consumers (behind feature flag)
4. Reference configuration.md and services.md for details

**What stays the same**: Everything else. The NAPI wrapper, pubsub adapter, connect vs serve semantics, stream handling, constraints — all unchanged.

## Acceptance Criteria

- [ ] `AlknetServer` interface in NAPI section updated with reloadAuth(), reloadForwarding(), reloadAll()
- [ ] TypeScript interface signature for reload methods matches research/configuration.md NAPI section
- [ ] Note added: call protocol integration — NAPI consumers can register operation handlers
- [ ] Note added: irpc service creation available for NAPI consumers (feature-gated)
- [ ] References to configuration.md and services.md added
- [ ] `last_updated` in YAML frontmatter updated
- [ ] Existing content unchanged except where additions are made
- [ ] ADR references remain correct

## References

- docs/architecture/napi-and-pubsub.md — current content
- docs/research/configuration.md — NAPI Reload API section
- docs/research/integration-plan.md — Phase 3.4 (NAPI layer updates)

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
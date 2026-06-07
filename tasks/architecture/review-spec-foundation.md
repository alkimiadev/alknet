---
id: architecture/review-spec-foundation
name: Review Phase 0 specs — validate consistency, completeness, and ADR alignment
status: pending
depends_on:
  - architecture/spec-configuration
  - architecture/spec-identity
  - architecture/spec-secret-service
  - architecture/spec-storage
  - architecture/spec-flowgraph
  - architecture/spec-interface
  - architecture/spec-services
  - architecture/spec-update-overview
  - architecture/spec-update-auth
  - architecture/spec-update-call-protocol
  - architecture/spec-update-server
  - architecture/spec-update-napi
  - architecture/spec-update-readme
  - architecture/spec-update-open-questions
scope: broad
risk: low
impact: project
level: review
---

## Description

Review all Phase 0 spec documents after they're written. This is the Phase 0 review checklist from the integration plan, applied against the actual deliverables.

## Acceptance Criteria

- [ ] **No inline decision rationale** — all "why" decisions are in ADRs, specs reference ADR numbers
- [ ] **No inline open questions** — all OQs are in open-questions.md, specs reference OQ numbers
- [ ] **Terminology is consistent** — head/worker everywhere (no hub/spoke in specs, ADRs retain historical language)
- [ ] **Layer boundaries are clear** — every component belongs to exactly one layer (Transport, Interface, Protocol)
- [ ] **Phase boundaries are clear** — specs distinguish what ships in Phase 1 (ConfigIdentityProvider, ArcSwap, local dispatch) from what's contracted for later (StorageIdentityProvider, irpc service layer, application services, multi-node deployment). No spec should imply that alknet-storage, alknet-secret, or the irpc service implementations already exist.
- [ ] **Every spec has YAML frontmatter** with status and last_updated
- [ ] **Identity is consistently defined** — Identity struct is `{id, scopes, resources}` everywhere (identity.md is canonical, auth.md references it)
- [ ] **OperationEnv is consistently described** — three dispatch paths match across services.md, call-protocol.md, and identity.md
- [ ] **irpc positioning is consistent** — always described as one dispatch backend for OperationEnv, never as a replacement for the call protocol
- [ ] **Interface trait is consistent** — SshInterface and RawFramingInterface match across interface.md and server.md
- [ ] **ForwardingPolicy is consistently placed** — in DynamicConfig, checked before proxy spawn, reference in server.md and configuration.md
- [ ] **README.md and ADR table** include all new documents and ADRs
- [ ] **No broken links** between doc references
- [ ] **All specs follow the format**: What, Why, Architecture, Constraints, Open Questions, Design Decisions

## References

- docs/research/integration-plan.md — Phase 0: Review Checklist
- docs/architecture/ — all architecture docs

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
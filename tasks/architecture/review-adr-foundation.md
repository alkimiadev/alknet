---
id: architecture/review-adr-foundation
name: Review Phase 0a ADRs — foundation decisions before spec writing
status: completed
depends_on:
  - architecture/adr-034-head-worker-terminology
  - architecture/adr-032-event-boundary-discipline
  - architecture/adr-029-identity-core-type
  - architecture/adr-030-static-dynamic-config-split
  - architecture/adr-031-forwarding-policy
  - architecture/adr-028-auth-irpc-service
  - architecture/adr-027-crate-decomposition
  - architecture/adr-026-transport-interface-separation
  - architecture/adr-033-operationenv-irpc-call-protocol
scope: broad
risk: low
impact: project
level: review
---

## Description

Review all Phase 0a ADRs (026-034) before proceeding to spec writing (Phase 0b). This is the critical checkpoint where we validate that the architectural decisions are consistent and complete before downstream specs reference them.

This review should happen before any spec documents are created or updated, since specs will reference ADR numbers.

## Acceptance Criteria

- [ ] All 9 ADRs (026-034) are written and follow the established format
- [ ] ADRs cross-reference each other correctly (e.g., ADR-028 references ADR-029, ADR-027 references ADR-029)
- [ ] No ADR contradicts another (e.g., ADR-026's Interface trait must be compatible with ADR-033's OperationEnv)
- [ ] Crate dependency graph from ADR-027 is acyclic (core depends on nothing heavy)
- [ ] Layer boundaries from ADR-026 cleanly separate Transport, Interface, and Protocol
- [ ] OperationEnv dispatch model from ADR-033 is consistent with ADR-028 (auth service) and ADR-027 (crate decomposition)
- [ ] Terminology is consistent (head/worker everywhere per ADR-034)
- [ ] Open questions referenced in ADRs have proposed resolutions consistent with the decision
- [ ] Each ADR has clear consequences (both positive and negative)
- [ ] ADRs are added to the README.md ADR table

## References

- docs/architecture/decisions/ — all existing ADRs 001-025 for format reference
- docs/research/integration-plan.md — Phase 0 review checklist

## Notes

ADRs reviewed for cross-reference consistency, terminology alignment, and acyclic dependency graph. Two fixes applied: ADR-027 dependency graph diagram clarified and missing cross-reference to ADR-028 added; ADR-033 missing cross-references to ADR-026 and ADR-028 added.

## Summary

All 9 Phase 0a ADRs (026-034) reviewed. No contradictions found between ADRs. Minor fixes: ADR-027 dependency graph made clearer, ADR-027 and ADR-033 cross-references completed. ADR numbering gaps (020-022) noted in README.md footnote.
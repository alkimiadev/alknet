---
id: architecture/adr-034-head-worker-terminology
name: Write ADR-034 — Head/worker terminology
status: pending
depends_on: []
scope: single
risk: trivial
impact: project
level: implementation
---

## Description

Write ADR-034 formalizing the decision to use head/worker terminology instead of hub/spoke throughout the project.

This decision has already been applied in practice — `call-protocol.md`, `auth.md`, `open-questions.md`, and `napi-and-pubsub.md` were all updated to head/worker. The existing ADRs (024, 025) retain their original hub/spoke language because ADRs are historical records. ADR-018 is noted as superseded/extended by ADR-024 and the three-layer model.

The ADR exists to formally record the decision so future contributors understand why and can reference it.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/034-head-worker-terminology.md` exists
- [ ] ADR follows the established format (Status, Context, Decision, Consequences, References)
- [ ] Context explains why hub/spoke is being replaced (mesh topologies, a head is also a worker)
- [ ] Decision states: head/worker everywhere in new specs and code; ADRs retain original language as historical records
- [ ] Consequences note: natural mesh formation, consistency with integration plan terminology
- [ ] References: integration-plan.md, ADR-024, ADR-025

## References

- docs/research/integration-plan.md — Phase 0 ADR 034 entry, inconsistencies section item 1
- docs/architecture/decisions/024-bidirectional-call-protocol.md — uses hub/spoke historically
- docs/architecture/decisions/025-handler-spec-separation.md — uses hub/spoke historically

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
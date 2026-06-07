---
id: architecture/adr-032-event-boundary-discipline
name: Write ADR-032 — Event boundary discipline
status: pending
depends_on: []
scope: single
risk: low
impact: project
level: implementation
---

## Description

Write ADR-032 establishing event boundary discipline as a hard architectural constraint.

The research (services.md, storage.md) identifies three distinct communication patterns with clear boundaries:

1. **Domain events** (Honker streams) — internal to the owning service, for state reconstruction. Never cross service boundaries without projection.
2. **irpc service calls** — synchronous request-response, within a node or cluster. Internal to the system.
3. **Call protocol events** (EventEnvelope) — cross-node, cross-language integration events. These are what cross boundaries.

The ADR must state this as a hard constraint, not a suggestion. Conflating these three patterns is an anti-pattern that leads to leaky event stores and coupling.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/032-event-boundary-discipline.md` exists
- [ ] ADR follows established format (Status, Context, Decision, Consequences, References)
- [ ] Context explains the three patterns and why conflating them is harmful
- [ ] Decision states: domain events stay within the owning service; irpc calls are synchronous internal boundaries; call protocol events are the only events that cross node boundaries; projection from domain events to integration events is required when crossing boundaries
- [ ] Consequences include: prevents leaky event stores, services are independently deployable, Honker and irpc are implementation details not exposed across boundaries
- [ ] References: research/services.md, research/storage.md, integration-plan.md

## References

- docs/research/services.md — event boundary discipline section
- docs/research/storage.md — Honker integration, event boundaries
- docs/research/integration-plan.md — ADR 032 entry

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
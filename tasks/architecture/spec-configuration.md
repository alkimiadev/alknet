---
id: architecture/spec-configuration
name: Promote configuration.md from research to architecture spec
status: completed
depends_on:
  - architecture/adr-030-static-dynamic-config-split
  - architecture/adr-031-forwarding-policy
  - architecture/adr-028-auth-irpc-service
scope: moderate
risk: medium
impact: phase
level: implementation
---

## Description

Promote `docs/research/configuration.md` to `docs/architecture/configuration.md` as a proper architecture spec document. The research doc is nearly spec-ready — this task is primarily cleanup, restructuring, and aligning with ADR decisions.

**Source**: `docs/research/configuration.md` (651 lines, well-analyzed)

**Key cleanup items**:
1. Remove duplicate "## Problem" heading (lines 20-21 both say `## Problem`)
2. Resolve open questions per ADRs: OQ-12 (global rules + principal matching via IdentityProvider), OQ-13 (no file watching, confirmed), OQ-14 (ArcSwap, confirmed), OQ-16 (TransportKind match in ForwardingRule), OQ-18 (IdentityProvider owns scopes)
3. Remove inline decision rationale — reference ADR-030, ADR-031, ADR-028
4. Remove inline open questions — reference open-questions.md OQ numbers
5. Add YAML frontmatter: `status: draft`, `last_updated: <date>`
6. Restructure to follow established spec format (What, Why, Architecture, Constraints, Open Questions, Design Decisions)
7. Update terminology: head/worker (already done in research doc)
8. Reconcile ADR-011: TOML config file amends ADR-011 (convenience layer), doesn't supersede it
9. Remove research-only sections that are exploration/analysis — keep only the decisions and their architecture

**What stays**: StaticConfig/DynamicConfig split, ArcSwap model, ForwardingPolicy design, multi-transport listeners, ConfigService, NAPI reload API, TOML format, CLI vs programmatic behavior table

## Acceptance Criteria

- [ ] `docs/architecture/configuration.md` exists with YAML frontmatter (`status: draft`)
- [ ] No duplicate "## Problem" heading
- [ ] All inline decision rationale replaced with ADR references (030, 031, 028)
- [ ] All inline open questions replaced with OQ references
- [ ] OQ-12 resolved: global rules + principal matching, reference ADR-031
- [ ] OQ-16 resolved: TransportKind match, reference ADR-031
- [ ] OQ-18 resolved: IdentityProvider owns scopes, reference ADR-029
- [ ] TOML config file positioned as amending ADR-011, not replacing programmatic API
- [ ] Follows spec format: What, Why, Architecture, Constraints, Open Questions, Design Decisions
- [ ] Consistent head/worker terminology throughout
- [ ] `docs/architecture/README.md` updated to include configuration.md in architecture docs table
- [ ] `docs/research/configuration.md` retains its content (not deleted — it's research source material)

## References

- docs/research/configuration.md — source material to promote
- docs/architecture/decisions/030-static-dynamic-config-split.md — ADR to reference
- docs/architecture/decisions/031-forwarding-policy.md — ADR to reference
- docs/architecture/decisions/028-auth-irpc-service.md — ADR to reference
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — amended by TOML config

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
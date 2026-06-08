---
id: cleanup/adr-doc-comments
name: Add ADR number references to doc comments in new modules
status: pending
depends_on:
  - review/phase1-core-modifications
scope: narrow
risk: trivial
impact: isolated
level: implementation
---

## Description

The existing codebase has a pattern of referencing ADR numbers in doc comments (e.g., `transport/mod.rs` references ADR-001). The new Phase 1 modules should follow the same pattern for discoverability.

**Modules needing ADR references**:
- `auth/identity.rs` → ADR-029, ADR-028
- `config/forwarding.rs` → ADR-031
- `config/static_config.rs` → ADR-030
- `config/dynamic_config.rs` → ADR-030
- `config/config_service.rs` → ADR-030
- `call/mod.rs` → ADR-024, ADR-033
- `call/spec.rs` → ADR-025, ADR-033
- `interface/mod.rs` → ADR-026

## Acceptance Criteria

- [ ] Module-level doc comments reference relevant ADR numbers
- [ ] Style matches existing ADR references in the codebase (e.g., `//! See ADR-029.`)

## References

- crates/alknet-core/src/transport/mod.rs — existing pattern to follow

## Notes

> Suggested during Phase 1 review (S2)

## Summary

> To be filled on completion
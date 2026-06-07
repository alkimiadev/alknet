---
id: architecture/spec-update-readme
name: Update architecture README.md — add new docs and ADRs to tables
status: pending
depends_on:
  - architecture/spec-configuration
  - architecture/spec-identity
  - architecture/spec-secret-service
  - architecture/spec-storage
  - architecture/spec-flowgraph
  - architecture/spec-interface
  - architecture/spec-services
  - architecture/review-adr-foundation
scope: narrow
risk: trivial
impact: project
level: implementation
---

## Description

Update `docs/architecture/README.md` to add all new architecture documents and ADRs (026-034) to their respective tables. This is the final documentation assembly task — it runs after all new spec docs are created.

**Changes to Architecture Documents table**:
Add rows for: identity.md, services.md, interface.md, configuration.md, secret-service.md, storage.md, flowgraph.md

**Changes to Research Documents table**:
No changes needed — research docs stay as-is.

**Changes to ADR Table**:
Add ADRs 026-034 with correct titles and status (Accepted).

**Changes to Current State section**:
Update to reflect the new scope: services, identity, interface layer, configuration architecture.

## Acceptance Criteria

- [ ] Architecture Documents table includes all new docs (identity.md, services.md, interface.md, configuration.md, secret-service.md, storage.md, flowgraph.md) with correct status and descriptions
- [ ] ADR table includes ADRs 026-034 with correct titles and "Accepted" status
- [ ] Current State section updated to reflect expanded scope
- [ ] `last_updated` in YAML frontmatter updated
- [ ] No broken links (all new doc references point to files that exist)

## References

- docs/architecture/README.md — current content to update

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion
# OQ-39: `to_openapi` Published-Spec Versioning

- **Origin**: [ADR-017](decisions/017-call-protocol-client-and-adapter-contract.md)
  Consequences, [http-adapters.md](crates/http/http-adapters.md)
- **Status**: **resolved** (2026-06-30 by ADR-045)
- **Door type**: One-way (after first publication), two-way (before)
- **Priority**: medium → resolved
- **Resolution**: **[ADR-045](decisions/045-to-openapi-gateway-spec-versioning.md)
  commits the versioning scheme.** The gateway pattern (ADR-042)
  dissolved most of the original concern: the published doc describes
  **5 fixed gateway endpoints** (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe`), not the per-operation surface. Per-caller
  operation changes (add/remove/modify an operation, change an
  operation's schema) do **not** change the published doc — the
  operation set is discovered at runtime via `AccessControl`-filtered
  `/search`, not preloaded into the doc. So the version does not churn
  on every operation change (the original OQ-39 worry, framed under the
  pre-ADR-042 per-operation-paths model).

  What remains is narrow: how the published gateway doc signals its
  version. The decision:

  1. **`to_openapi` emits `info.version` as semver.** Standard OpenAPI
     field, standard semver interpretation — no alknet-specific
     detection mechanism.
  2. **The version tracks the gateway endpoint contract, not the
     operation set.** Major = breaking change to the gateway (endpoint
     removed/renamed, required request field added, response shape
     changed, error-mapping semantics changed per ADR-023); Minor =
     additive (new endpoint, new optional field); Patch = wording/docs.
     Per-caller operation changes do **not** bump the version.
  3. **Bump on change to the gateway shape, not on regeneration.**
     A restart that regenerates the same gateway shape yields the same
     version.
  4. **Consumers detect breaking changes via the major version.** A
     client compares `info.version`'s major component to the version it
     built against; a major bump signals "re-read the doc, something
     broke." Minor/patch are informational.
  5. **The additive traditional per-operation-paths projection
     (ADR-042 §5) versions independently** on its own schedule — its
     surface *does* change with the operation set, so its versioning is
     the per-operation churn the original OQ-39 framed. That projection
     is opt-in and out of scope for ADR-045; the gateway doc is the
     default published contract and the one ADR-045 governs.

  The original "version marker emitted so consumers can detect mapping
  changes" constraint (from ADR-017 Consequences) is satisfied by
  `info.version` semver. ADR-045 lifts the "published artifact is a
  contract" blind spot in ADR-009's framework (it classifies doors by
  reversal cost in the codebase, not by compatibility cost for external
  consumers) into its Context and honors the constraint without changing
  ADR-009's framework.
- **Cross-references**: ADR-009, ADR-017, ADR-023, ADR-036, ADR-042,
  ADR-045, [http-adapters.md](crates/http/http-adapters.md)

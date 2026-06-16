---
status: draft
last_updated: 2026-06-16
---

# Alknet Architecture

## Current State

**Pre-implementation.** The project has completed a pivot from a three-layer model to an ALPN-as-service model. The greenfield workspace contains only `alknet-secret` (stable) and research/reference material. Foundational ADRs (001–009) are in place, including the BiStream type definition (ADR-007), secret service integration (ADR-008), and the one-way door decision framework (ADR-009). Architecture specs are ready for Phase 1 implementation planning.

**Next step**: Resolve remaining two-way-door questions during implementation. Start with alknet-core (ProtocolHandler trait, Connection, endpoint, router, auth types, config).

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph, shared types, design principles |
| [open-questions.md](open-questions.md) | draft | Centralized OQ tracker with door-type classifications |

Crate-specific specs will be created when each crate is ready for Phase 1 architecture work, not in advance.

## ADR Table

| ADR | Title | Status |
|-----|-------|--------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Accepted |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | Accepted |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | Accepted |
| [004](decisions/004-auth-as-shared-core.md) | Auth as Shared Core (IdentityProvider) | Accepted |
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | Accepted |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | Accepted |
| [007](decisions/007-bistream-type-definition.md) | BiStream Type Definition | Accepted |
| [008](decisions/008-secret-service-integration.md) | Secret Service Integration Point | Accepted |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Accepted |

## Open Questions

See [open-questions.md](open-questions.md) for the full tracker.

**Resolved one-way doors:**
- **OQ-01**: BiStream type — trait with Connection parameter (ADR-007)
- **OQ-02**: AuthContext timing — hybrid model (ADR-004)
- **OQ-03**: ALPN naming — `alknet/` prefix, no version (ADR-006)
- **OQ-06**: ALPN per connection, not per stream (ADR-006)
- **OQ-08**: Secret service — CLI-embedded via call protocol (ADR-008)

**Two-way doors (deferred to implementation):**
- **OQ-04**: Dynamic handler registration — start static, add ArcSwap later
- **OQ-05**: Multi-transport endpoint — start with quinn, add transport trait later
- **OQ-07**: Call protocol scope — start with one stream per operation

**Deferred (not active):**
- **OQ-09**: WASM target boundaries — design constraint, not deliverable
- **OQ-10**: Git adapter scope — start with smart protocol, add ERC721 later

## Document Lifecycle

| Status | Meaning | Transitions |
|--------|---------|-------------|
| `draft` | Under active development. May change significantly. | → `reviewed` when open questions are resolved |
| `reviewed` | Architecture is final. Implementation may begin. Changes require review. | → `stable` when implementation is complete and verified |
| `stable` | Locked. Changes require review and may warrant an ADR. | → `deprecated` when superseded |
| `deprecated` | Superseded. Kept for reference. | Removed when no longer referenced |

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- Cleanup plan: `docs/research/pivot/cleanup-plan.md`
- SDD process: `docs/sdd_process.md`
- Reference implementation: `/workspace/@alkdev/alknet-main/`
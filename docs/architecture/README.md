---
status: draft
last_updated: 2026-06-16
---

# Alknet Architecture

## Current State

**Pre-implementation.** The project has completed a pivot from a three-layer model to an ALPN-as-service model. The greenfield workspace contains only `alknet-vault` (stable) and research/reference material. Foundational ADRs (001–012) are in place, including the BiStream type definition (ADR-007), vault integration (ADR-008), ALPN router/endpoint (ADR-010), AuthContext structure (ADR-011), and call protocol stream model (ADR-012). The alknet-core and alknet-call crate specs are in draft.

**Next step**: Review alknet-call spec documents, then begin implementation. Two-way-door questions (OQ-11, OQ-13, OQ-14) will be resolved during implementation.

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph, shared types, design principles |
| [open-questions.md](open-questions.md) | draft | Centralized OQ tracker with door-type classifications |
| [crates/core/README.md](crates/core/README.md) | draft | alknet-core crate index |
| [crates/core/core-types.md](crates/core/core-types.md) | draft | ProtocolHandler, HandlerError, Connection, BiStream, StreamError |
| [crates/core/endpoint.md](crates/core/endpoint.md) | draft | ALPN router, HandlerRegistry, accept loop, shutdown |
| [crates/core/auth.md](crates/core/auth.md) | draft | AuthContext, Identity, IdentityProvider, AuthToken, resolution flow |
| [crates/core/config.md](crates/core/config.md) | draft | StaticConfig, DynamicConfig, ArcSwap, ConfigReloadHandle |
| [crates/call/README.md](crates/call/README.md) | draft | alknet-call crate index |
| [crates/call/call-protocol.md](crates/call/call-protocol.md) | draft | CallAdapter, EventEnvelope framing, stream model, PendingRequestMap, bidirectional calls |
| [crates/call/operation-registry.md](crates/call/operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, service discovery, irpc integration |

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
| [008](decisions/008-secret-service-integration.md) | Vault Integration Point | Accepted |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Accepted |
| [010](decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Accepted |
| [011](decisions/011-authcontext-structure.md) | AuthContext Structure and Resolution Flow | Accepted |
| [012](decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Accepted |

## Open Questions

See [open-questions.md](open-questions.md) for the full tracker.

**Resolved one-way doors:**
- **OQ-01**: BiStream type — trait with Connection parameter (ADR-007)
- **OQ-02**: AuthContext timing — hybrid model (ADR-004)
- **OQ-03**: ALPN naming — `alknet/` prefix, no version (ADR-006)
- **OQ-05**: Multi-connectivity endpoint — quinn + iroh, both feature-gated (ADR-010)
- **OQ-06**: ALPN per connection, not per stream (ADR-006)
- **OQ-08**: Vault integration — CLI-embedded via call protocol (ADR-008)

**Resolved two-way doors:**
- **OQ-04**: Dynamic handler registration — static at startup (ADR-010)
- **OQ-07**: Call protocol scope — bidirectional streams, EventEnvelope, ID-based correlation (ADR-012)
- **OQ-12**: TLS certificate provisioning — file paths in StaticConfig, ACME later

**Open two-way doors (resolved during implementation):**
- **OQ-11**: Handler-level auth resolution observability — decide during implementation
- **OQ-13**: Operation path format — `/{service}/{op}` for Phase 1, `/{node}/{service}/{op}` later
- **OQ-14**: Batch operation semantics — client-side pattern for Phase 1, batch event types later

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
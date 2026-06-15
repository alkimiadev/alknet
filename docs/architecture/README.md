---
status: draft
last_updated: 2026-06-15
---

# Alknet Architecture

## Current State

**Pre-implementation.** The project has completed a pivot from a three-layer model (StreamInterface/MessageInterface, ListenerConfig, OperationEnv) to an ALPN-as-service model. The greenfield workspace contains only `alknet-secret` (stable) and research/reference material. Architecture specs are being produced following the SDD process (Phase 1).

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph, shared types |
| [open-questions.md](open-questions.md) | draft | Centralized OQ tracker across all crates |
| [crates/alknet-core/spec.md](crates/alknet-core/spec.md) | planned | Core crate: ProtocolHandler, endpoint, router, auth, config |
| [crates/alknet-ssh/spec.md](crates/alknet-ssh/spec.md) | planned | SSH handler: russh, SOCKS5, port forwarding |
| [crates/alknet-call/spec.md](crates/alknet-call/spec.md) | planned | Call protocol: irpc, operation registry, access control |
| [crates/alknet-secret/spec.md](crates/alknet-secret/spec.md) | planned | Key derivation and encryption (already implemented) |
| [crates/alknet-sftp/spec.md](crates/alknet-sftp/spec.md) | planned | SFTP handler: russh-sftp protocol core |
| [crates/alknet-git/spec.md](crates/alknet-git/spec.md) | planned | Git handler: gix, pkt-line protocol |
| [crates/alknet-http/spec.md](crates/alknet-http/spec.md) | planned | HTTP handler: axum, REST API, MCP |
| [crates/alknet-dns/spec.md](crates/alknet-dns/spec.md) | planned | DNS handler: hickory-proto, pkarr, service discovery |
| [crates/alknet-msg/spec.md](crates/alknet-msg/spec.md) | planned | Messaging: E2E encryption, mixnet |
| [crates/alknet-napi/spec.md](crates/alknet-napi/spec.md) | planned | Node.js native addon: call protocol client |
| [crates/alknet/spec.md](crates/alknet/spec.md) | planned | CLI binary: handler registration, endpoint startup |

## ADR Table

| ADR | Title | Status |
|-----|-------|--------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Accepted |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | Accepted |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | Accepted |
| [004](decisions/004-auth-as-shared-core.md) | Auth as Shared Core (IdentityProvider) | Accepted |
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | Accepted |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | Accepted |

## Open Questions

See [open-questions.md](open-questions.md) for the full tracker.

Key questions affecting current work:
- **OQ-01**: BiStream type definition — what exactly does BiStream expose? (open)
- **OQ-02**: AuthContext resolution timing — hybrid model resolved (see ADR-004) (resolved)
- **OQ-03**: ALPN string naming convention — resolved (see ADR-006) (resolved)
- **OQ-04**: Dynamic handler registration at runtime vs static at startup (open)

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
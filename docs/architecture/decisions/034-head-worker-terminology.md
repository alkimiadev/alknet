# ADR-034: Head/Worker Terminology

## Status

Accepted

## Context

The project previously used hub/spoke terminology for describing node
relationships: a hub node that coordinates connections and spokes that connect to
it. This terminology implies a strict star topology where the hub is
fundamentally different from spokes.

In practice, a coordinating node can also execute operations (run services,
forward traffic). Any node can become a coordinator. The architecture supports
mesh topologies where nodes coordinate in a peer-to-peer fashion.

The research documents (`core.md`, `services.md`) and updated architecture
specs (`call-protocol.md`, `auth.md`, `napi-and-pubsub.md`, `open-questions.md`)
already use head/worker consistently. Existing ADRs (024, 025) retain their
original hub/spoke language because ADRs are historical records.

## Decision

**Use head/worker terminology throughout the project.**

- **Head node**: A node that coordinates — accepts connections, routes
  operations, manages cluster state. A head is also a worker (it can execute
  operations).
- **Worker node**: A node that connects to a head, registers its services, and
  executes operations. Any worker can become a head.
- **Node**: Any participant in the network. Every node has an Ed25519 identity.

The terms hub and spoke are deprecated in all new specs, code, and
documentation. Existing ADRs retain their original language as historical
records — ADRs document what was decided at the time, not what the current
terminology is.

## Consequences

- **Positive**: Natural mesh formation. A head that is also a worker enables
  multi-hop routing, redundancy, and distributed topologies without a
  centralized authority.
- **Positive**: Consistency with integration plan and research documents.
- **Positive**: The terminology better reflects the architecture — there is no
  single "hub" that's fundamentally different from "spokes."
- **Neutral**: Existing ADRs (024, 025) retain hub/spoke in their text. This is
  intentional — ADRs are historical records.

## References

- [research/integration-plan.md](../../research/integration-plan.md) — Phase 0 ADR 034 entry, inconsistencies section
- [ADR-024](024-bidirectional-call-protocol.md) — Uses hub/spoke historically
- [ADR-025](025-handler-spec-separation.md) — Uses hub/spoke historically
- [research/core.md](../../research/core.md) — Head/worker terminology
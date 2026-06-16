# ADR-009: One-Way Door Decision Framework

## Status

Accepted

## Context

Not all architectural decisions carry the same reversal cost. Some decisions are easy to change later — if you pick the wrong data structure, you refactor. Other decisions are nearly impossible to reverse — if you build a type hierarchy that forecloses WASM compatibility, every handler written against that hierarchy must be rewritten.

This distinction matters especially during Phase 0 (exploration) and early Phase 1 (architecture). The project is post-pivot with foundational ADRs in place but no implementation code yet (except alknet-secret). Decisions made now shape the API surface that every handler depends on.

Without an explicit framework, one-way doors can be treated as casually as two-way doors, leading to costly rework. Or conversely, two-way doors can be over-analyzed, blocking progress on decisions that are cheap to reverse.

## Decision

### Classification

Every architectural decision is classified as one of:

**One-way door** — Reversing this decision requires rewriting significant code across multiple crates or permanently closes a capability door. Examples:
- BiStream as a concrete quinn type (closes WASM door permanently)
- alknet-vault pulled into alknet-core as a dependency (loses standalone property permanently)
- ProtocolHandler signature changes (every handler must be rewritten)

**Two-way door** — Reversing this decision is cheap or additive. Examples:
- Static vs dynamic handler registration (can add ArcSwap later)
- Single transport vs multi-transport endpoint (can add transport trait later)
- Call protocol stream model (can add multiplexing later)

### Process

- **One-way doors** require an ADR before implementation. If the right choice is unclear, validate with a POC before writing the ADR. If a POC can't resolve the uncertainty within a reasonable timebox, default to the option that keeps more doors open.
- **Two-way doors** can be decided during implementation. Start with the simplest option and add complexity when needed. Note the decision in a commit message or a brief ADR if the context is worth capturing, but don't block on it.
- When in doubt, classify up. If it's unclear whether a door is one-way or two-way, treat it as one-way until proven otherwise.

### WASM as a design constraint

WASM compatibility is not an immediate implementation goal, but it is a **design constraint on one-way doors**. Decisions that would permanently prevent WASM targets from participating as peers require explicit justification. This means:
- Core types (BiStream, ProtocolHandler, AuthContext) must not assume tokio or quinn
- Protocol parsers that are pure data transformations should remain transport-agnostic
- The cost of keeping the WASM door open is low (trait vs concrete type, abstracted I/O) and the cost of closing it is high (impossible to reverse without rewriting every handler)

This is not "WASM support now." It's "don't close the WASM door accidentally."

## Consequences

**Positive:**
- One-way doors get the deliberation they deserve — ADRs, POCs, explicit justification
- Two-way doors don't block progress — start simple, add complexity when needed
- WASM compatibility is preserved as a constraint, not treated as an active deliverable
- The framework creates a shared vocabulary for discussing decision urgency ("is this a one-way door?")

**Negative:**
- Classification requires judgment — some decisions are genuinely ambiguous (mitigated: classify up when in doubt)
- POC timeboxing can feel constraining on genuine hard problems (mitigated: the timebox is "reasonable," not "arbitrary")
- The framework adds a step to every architectural discussion ("is this one-way or two-way?") — but this step is fast and prevents expensive mistakes

## References

- ADR-007: BiStream type definition (one-way door: WASM compatibility)
- ADR-008: Secret service integration point (one-way door: standalone crate independence)
- SDD process: `docs/sdd_process.md` (Phase 0 exploration, POC specialist)
- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
# ADR-045: to_openapi Gateway-Spec Versioning

## Status

Proposed

## Context

OQ-39 asked how the published `to_openapi` spec is versioned. ADR-017
Consequences established that a published `to_*` spec is a compatibility
contract: once external clients build against it, the mapping semantics
become a de facto contract and changing them breaks every client.

The original framing of OQ-39 assumed `to_openapi` generated a
traditional per-operation-paths OpenAPI doc — one path per `External`
operation, changing whenever an operation is added, removed, or has its
schema modified. Under that model the versioning surface is large and
churns constantly, and the doc is a static full-surface dump (the Gitea
failure mode: admin ops shown to every caller, no per-caller filtering).

ADR-042 replaced that model with the **gateway pattern**: `to_openapi`
generates a doc describing **5 fixed gateway endpoints**
(`/search`, `/schema`, `/call`, `/batch`, `/subscribe`), and the
per-caller operation surface is discovered at runtime through
`AccessControl`-filtered `/search` — not preloaded into the static doc.
This is the same mechanic as the MCP gateway (ADR-041), with `subscribe`
added because OpenAPI/SSE supports streaming where MCP tool calls are
request/response.

The consequence for versioning: the published doc is now a small, stable
surface that changes only when the gateway endpoint set or an endpoint's
request/response shape changes. Per-caller operation changes
(adding/removing/modifying operations, changing an operation's schema)
do **not** change the published doc — those operations are not in the
doc; they are discovered via `/search`. This dissolves most of the
churn the original OQ-39 was concerned about.

What remains is the narrow versioning question: how does the published
gateway doc signal its version so consumers can detect breaking changes?
This is one-way after first publication — once external clients build
against the gateway doc, renaming `/call` or changing its request shape
breaks them.

A note on door-type framing: ADR-009 classifies doors by reversal cost
in the codebase. The "published artifact is a contract" case is a blind
spot in that framework — the published doc's reversal cost is paid by
external consumers, not in the codebase. ADR-017 Consequences captures
this (published `to_*` specs are compatibility contracts); this ADR
honors the constraint without changing ADR-009's framework. The door is
two-way before first publication (the gateway shape can be revised
freely while no external client depends on it) and one-way after
(revising requires a major version bump that signals breakage to
consumers).

## Decision

### 1. The published gateway doc carries a semver `info.version`

`to_openapi` emits `info.version` as a semver string. The version
reflects the **gateway endpoint contract** (the 5 endpoints + their
request/response shapes), not the operation set:

- **Major bump** — breaking change to the gateway contract: an endpoint
  removed or renamed, a required field added to a gateway endpoint's
  request, a response shape changed in a backward-incompatible way
  (including removing or retyping an existing response field, or
  tightening an optional field to required),
  the error-mapping semantics (ADR-023) changed.
- **Minor bump** — additive change: a new gateway endpoint added
  (e.g., a future `/subscribe-batch`), a new optional request field, a
  new response field. Additive changes do not break existing clients.
- **Patch bump** — description/wording changes, documentation, no shape
  change.

Cases not enumerated above follow **standard semver**: a change is a
major bump if it could break a client built against the prior version,
a minor bump if it is purely additive, a patch bump otherwise. The
enumerated triggers above are the common cases, not an exhaustive list.

Per-caller operation changes (registering a new operation, removing one,
changing an operation's input schema) **do not bump the version** — the
operation set is not part of the published doc; it is discovered via
`/search` at runtime. This is the key simplification the gateway pattern
buys: the operation surface can evolve freely without touching the
published contract version.

### 2. The version is bumped on change to the gateway shape, not on regeneration

A deployment that regenerates the doc (e.g., on restart) gets the same
`info.version` unless the gateway shape changed. The version is a
function of the gateway contract, not of when the doc was generated.

### 3. Consumers detect breaking changes via the major version

A client reading the doc compares `info.version`'s major component to
the version it built against. A major bump signals "re-read the doc,
something broke." The minor/patch components are informational. This is
the standard OpenAPI/semver convention — no alknet-specific detection
mechanism.

### 4. The traditional per-operation-paths projection (additive, ADR-042 §5) versions independently

A deployment that builds the additive traditional REST projection
(ADR-042 §5) versions that doc on its own schedule — its surface
*does* change with the operation set, so its versioning is the
per-operation churn OQ-39 originally worried about. That projection is
opt-in and out of scope for this ADR; the gateway doc is the default
published contract and the one this ADR governs.

## Consequences

**Positive:**
- The published contract is a 5-endpoint surface that rarely changes.
  Versioning is bump-on-change, not bump-on-every-operation-change. The
  original OQ-39 concern (constant churn) is dissolved by the gateway
  pattern — the operation set is not in the doc.
- Consumers use standard semver/OpenAPI `info.version` — no
  alknet-specific version-detection mechanism to learn.
- Per-caller operation evolution (the common case) is decoupled from the
  published-contract version. A node can add/remove operations freely
  without bumping the doc version or breaking clients built against the
  gateway doc.
- The Gitea failure mode stays structurally impossible (ADR-042 §3):
  `/search` is `AccessControl`-filtered, so the doc never exposes ops
  the caller can't call. Versioning inherits this — the doc describes
  the gateway, not the operations.

**Negative:**
- A client cannot tell from the doc version alone *which* operations are
  available — it must call `/search`. This is by design (per-caller,
  runtime), but a client expecting a static operation list from the doc
  must learn the gateway pattern.
- The version only signals gateway-contract changes. An operation
  changing its input schema (a breaking change for callers of that
  operation) does not bump the doc version — that change is surfaced via
  `/schema` per-operation, not via the doc version. Clients that cache
  operation schemas must re-fetch `/schema` to detect per-operation
  changes; the doc version does not track them.

## Assumptions

1. **The 5-endpoint gateway set is stable.** ADR-042 Assumption 1. Adding
   endpoints is additive (minor bump); removing/renaming is a major bump.
   The initial 5-endpoint set is the first published contract.

2. **Per-operation schema changes are detected via `/schema`, not the
   doc version.** The doc version tracks the gateway contract only. A
   client that caches an operation's `OperationSpec` re-fetches `/schema`
   to detect changes to that operation. This is the standard
   discovery-then-invoke pattern; the doc version is not a per-operation
   change tracker.

3. **`info.version` is the single source of truth for the published
   contract version.** No separate `x-alknet-version` extension or
   content-hash header. Standard OpenAPI field, standard semver
   interpretation. A content-hash would be more precise but adds an
   alknet-specific mechanism for no real gain over semver-on-shape-
   change.

## References

- [ADR-009](009-one-way-door-decision-framework.md) — door-type
  framework (classifies by codebase reversal cost; the
  published-artifact-as-contract case is the blind spot this ADR honors
  without changing the framework)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) — published
  `to_*` specs are compatibility contracts (the one-way-after-
  publication constraint)
- [ADR-023](023-operation-error-schemas.md) — error-mapping semantics
  are part of the gateway contract (a change to them is a major bump)
- [ADR-036](036-http-to-call-operation-mapping.md) — the SSE projection
  for `/subscribe` (part of the gateway contract)
- [ADR-042](042-openapi-gateway-pattern.md) — the gateway pattern that
  makes the published doc a 5-endpoint surface instead of a per-
  operation surface; §4 explicitly deferred versioning to OQ-39
- OQ-39 — `to_openapi` published-spec versioning (resolved by this ADR)
- `crates/http/http-adapters.md` — the spec that emits `info.version`
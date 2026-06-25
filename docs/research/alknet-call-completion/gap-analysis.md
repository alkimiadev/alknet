---
status: draft
last_updated: 2026-06-25
---

# alknet-call Completion — Gap Analysis

This document captures the gap between the existing alknet-call architecture
(ADRs 005/007/012/014/015/016/017/022/023/024, specs in
`docs/architecture/crates/call/`) and the current implementation
(`crates/alknet-call/src/`), the decisions needed before implementation can
proceed, and the downstream crates this completion unblocks.

Unlike the alknet-ssh phase-0 findings (a true exploration doc for a crate with
no existing architecture), this is a **gap analysis + decision record** for
completing existing architecture. The specs are largely settled; the work is
implementing what's specced and resolving a small number of decisions the specs
left as two-way doors or didn't address.

## Implementation State (Verified)

The call protocol's server-side core is implemented and tested (159 tests,
passing). What's missing is the **client side** and the **adapter contract**.

### Implemented (~5,773 lines, 159 tests)

| Component | File | Lines | Status |
|-----------|------|-------|--------|
| `CallAdapter` (ProtocolHandler for `alknet/call`) | `protocol/adapter.rs` | 1,051 | Done |
| `CallConnection` (Layer 2 overlay, call/subscribe/abort) | `protocol/connection.rs` | 780 | **Partial** — see below |
| Wire framing (`EventEnvelope`, `FrameFramedReader/Writer`) | `protocol/wire.rs` | 544 | Done |
| `PendingRequestMap` (ID-based correlation) | `protocol/pending.rs` | 584 | Done |
| Abort cascade | `protocol/abort.rs` | 393 | Done |
| `OperationRegistry`, `HandlerRegistration`, builder | `registry/registration.rs` | 734 | Done |
| `OperationSpec`, `AccessControl`, `Visibility`, `ErrorDefinition` | `registry/spec.rs` | 321 | Done |
| `OperationContext`, `ScopedOperationEnv`, `AbortPolicy` | `registry/context.rs` | 178 | Done |
| `OperationEnv` trait, `CompositeOperationEnv`, `LocalOperationEnv` | `registry/env.rs` | 598 | Done |
| Service discovery (`services/list`, `services/schema` specs + handlers) | `registry/discovery.rs` | 557 | Done |

### Not implemented (specced in ADR-017, absent from `src/`)

| Component | Spec location | Priority | Unblocks |
|-----------|--------------|----------|---------|
| **`CallClient`** (outbound connection opener) | ADR-017 §1 | **Critical** | Runner pattern, bilateral exchange, every downstream consumer |
| **`from_call`** adapter (discover + register remote ops) | ADR-017 §3 | **Critical** (depends on `CallClient`) | Bilateral registry exchange, container-service pattern |
| **`OperationAdapter` trait** | ADR-017 §5 | **Enabling** | alknet-http's `from_openapi`/`from_mcp` implementations |
| **`from_jsonschema`** (schema-only registration, no handler) | ADR-017 §5 | Medium | Type validation, composition graph construction without runtime |

### Partially implemented

**`CallConnection`** (`protocol/connection.rs:34`) exists and implements the
Layer 2 overlay (`register_imported`, `register_imported_all`, `overlay_env`),
the `call()` / `subscribe()` / `abort()` outbound-call API, and the
`OverlayOperationEnv` trait impl. It is constructed via
`CallConnection::new(connection: Connection)` — meaning it wraps a `Connection`
that was *already established* by the `CallAdapter`'s accept path.

What's **missing** is the path that *opens* a connection and constructs a
`CallConnection` from the client side: `CallClient::connect(addr, credentials)`.
The `CallConnection` type itself is ready; the `CallClient` that produces it is
not. This confirms ADR-017's design: the dispatch loop is shared, and the
client is the connection-establishment half, not a parallel protocol
implementation.

## Decisions Needed

These are the points the specs either left as two-way doors or didn't address.
Each is tagged with door type per ADR-009. Resolving these is the prerequisite
for implementation.

### DC-1: `CallClient` registry scope — share global vs peer-scoped subset
*(One-way door — security dimension; ADR-017 Consequences flags this)*

ADR-017 §1 says `CallClient` "has its own operation registry to dispatch
incoming calls from the remote side." The Consequences section flags the
security dimension explicitly: "Sharing the global registry with a `CallClient`
exposes local capabilities to the remote peer... A peer-scoped subset must
filter by capability remote-safety, not just operation name."

Three options:

- **(a) Share the global registry** — the remote peer can call any `External`
  operation. Simplest. But per ADR-017's Consequences, this exposes the local
  node's `Capabilities` to the remote peer's calls: `OperationContext.capabilities`
  is populated from the local `HandlerRegistration.capabilities`, so the local
  node's API keys get used for the remote peer's call. This is a
  capability-exposure decision, not just a dispatch decision.
- **(b) Peer-scoped subset** — the `CallClient` holds a filtered view of the
  global registry, exposing only operations whose `Capabilities` are marked
  remote-safe. Requires a "remote-safe" flag on `HandlerRegistration` or on
  `Capabilities` entries (which don't exist today).
- **(c) Separate registry per `CallClient`** — the `CallClient` has its own
  registry, populated explicitly at construction. Most restrictive, most
  explicit, most boilerplate.

**Recommendation**: **(b) peer-scoped subset** as the v1 default, with (a) as
an explicit opt-in for trusted peers. Rationale: the runner pattern (worker
connects to hub) and the dispatch pattern (hub connects to worker) both
involve semi-trusted peers where exposing all local capabilities is wrong by
default. The "remote-safe" marking is the new concept this introduces — likely
a `Visibility::External`-adjacent flag or a `Capabilities` entry annotation.
This needs an ADR (likely an amendment to ADR-017 or a new ADR-028) because it
adds a concept to the registration bundle. The exact shape is a two-way door;
the *existence* of the filtering is the one-way door.

### DC-2: `from_call` re-import on reconnection
*(Two-way door — ADR-017 Assumption 4)*

ADR-017 Assumption 4: "If the remote operation changes (new schema, renamed),
the imported spec is stale until re-import. The assumption is that re-import
happens on reconnection or is triggered explicitly. Hot-swapping imported
specs is a two-way door."

The question: does `from_call` run automatically on every (re)connection, or
only on explicit trigger? Auto-re-import on reconnect is simpler for the
runner pattern (worker reconnects → hub re-discovers worker's ops
automatically). Explicit trigger is safer (no surprise registry mutations).

**Recommendation**: **auto-re-import on connection establishment** for the
v1 default. The runner pattern is the primary use case, and runners
reconnecting is the common case — making it explicit adds friction without
clear benefit. The overlay is per-connection (Layer 2, ADR-024), so a
stale overlay dies with the connection; re-import on reconnect is naturally
scoped. Explicit re-import can be added later as a `CallConnection::refresh()`
method if needed. This is a two-way door — record the default, don't spend an
ADR.

### DC-3: `from_call` namespace collision handling
*(Two-way door — ADR-017 §3 mentions `FromCallConfig` prefix)*

ADR-017 §3: `FromCallConfig` includes "An optional namespace prefix (to avoid
collisions when importing from multiple remote nodes)." The question is
whether the prefix is mandatory (always applied) or optional (default no
prefix, collision = last-wins or error).

**Recommendation**: **optional prefix, default no prefix, collision = error**.
A node importing from two remotes that both expose `/container/exec` without
prefixes should fail loudly rather than silently overwrite. The operator adds
prefixes when they know they're importing from multiple sources. This matches
the "default-deny, explicit-allow" posture. Two-way door, no ADR needed.

### DC-4: `OperationAdapter` trait error type
*(Two-way door — ADR-017 §5 says "specific trait signatures... are two-way
doors")*

ADR-017 §5 shows the trait as `async fn import(&self) -> Vec<HandlerRegistration>`,
with no error type. A real implementation needs to handle failures (HTTP fetch
fails for `from_openapi`, remote unreachable for `from_call`, schema parse
error for `from_jsonschema`).

**Recommendation**: the trait returns `Result<Vec<HandlerRegistration>,
AdapterError>` where `AdapterError` is a crate-level enum
(`DiscoveryFailed`, `SchemaParse`, `Transport`, `Unauthorized`). The spec's
omission of the error type was an implementation-detail two-way door; the
implementation fills it in. Record in the spec amendment, not a full ADR.

### DC-5: `from_jsonschema` vs `from_call` separation
*(Confirmed — not a decision, but recorded for clarity)*

These are distinct, not collapsible:

| | `from_jsonschema` | `from_call` |
|---|---|---|
| Schema source | Provided directly (caller fetches, passes in) | Discovered over wire (`services/list` + `services/schema`) |
| Handler at call time | None (schema-only, `FromJsonSchema` provenance) | Forwards over QUIC (`FromCall` provenance, leaf) |
| Use case | Type validation, discovery, composition graph construction | Actually invoking remote operations |

`from_call` = schema import (the `from_jsonschema`-shaped step) + forwarding
handler attachment. Keeping them separate preserves the "schema-only, no
execution" use case (type checking, safe composition planning without runtime).
This is confirmed architecture, not a decision to make.

## Adapter Location Map (Settled)

The decomposition principle: **the adapter trait lives where the types live
(alknet-call); the adapter implementations live where their transport
dependencies live.**

```
alknet-call (lean — no HTTP client, no HTTP server)
├── OperationAdapter trait          (the contract — async, per ADR-017 §5)
├── from_call                     (QUIC — discovers remote ops via call protocol)
├── from_jsonschema               (pure parse — caller fetches the doc, passes it in)
└── CallClient                    (outbound connection opener — the #1 gap)

alknet-http (owns HTTP server + HTTP client — separate crate, separate Phase 0)
├── ProtocolHandler for h2/http1.1/h3   (axum server — inbound HTTP)
├── from_openapi                   (parse OpenAPI doc + reqwest forwarding handler)
├── to_openapi                     (generate OpenAPI doc from local registry)
├── from_mcp  (feature-gated)       (import remote MCP tools over streamable HTTP — reqwest)
└── to_mcp    (feature-gated)       (expose local ops as MCP tools over streamable HTTP — axum)

Not built: MCP stdio transport
  — stdio = spawn arbitrary executable = built-in RCE ("download untrusted MCP servers")
  — streamable HTTP is the only supported MCP transport in alknet
  — recorded as an explicit security position, not a feature gap
```

**Why this works**: alknet-call never sees the HTTP client. The
`from_openapi`/`from_mcp` forwarding handlers are opaque `Arc<dyn Handler>`
from the registry's perspective — constructed by `alknet_http::from_openapi()`
at registration time, stored in `HandlerRegistration`, dispatched by the
`CallAdapter` which doesn't know reqwest is involved. alknet-call stays lean
(no reqwest, no axum); alknet-http owns both HTTP directions.

**ADR-003 dependency note**: alknet-http implementing `from_openapi`/`from_mcp`
means alknet-http depends on alknet-call (for `OperationSpec`, `Handler`,
`HandlerRegistration`, `OperationAdapter`). ADR-003's rule is "no handler crate
depends on another handler crate" — but alknet-call is both a handler *and* the
protocol foundation that alknet-agent and alknet-napi already consume. alknet-http
depending on alknet-call is "HTTP uses the call protocol types," not "HTTP depends
on SSH." This is within the spirit of ADR-003 (alknet-call is protocol-foundation,
not a peer handler), but should be noted explicitly in the alknet-http spec
and possibly as a one-line amendment to ADR-003 clarifying that alknet-call is a
protocol-foundation crate.

## The No-Env-Vars Invariant (Architectural Mechanism)

This is the architectural fix for the env-var problem in downstream consumers
like aisdk (the Rust port of Vercel's AI SDK at `/workspace/aisdk/`, 75
providers all reading `std::env::var("OPENAI_API_KEY")` in their `Default`
impls). The fix is **not** to modify aisdk — it's that the env-var path is
never taken because the assembly layer never calls `Default::default()`.

The credential injection path:

```
vault (seed)
  → assembly layer (derive + decrypt at startup, per ADR-014/019/025)
    → Capabilities (non-serializable, zeroized, immutable — ADR-014)
      → HandlerRegistration.capabilities (ADR-022, the registration bundle)
        → OperationContext.capabilities (per-request, populated by dispatch
          path from the bundle — ADR-022 §6)
          → from_openapi handler reads context.capabilities.get("openai")
            → injects into HTTP Authorization header
              → reqwest request goes out with vault-derived credential
```

The `from_openapi`/`from_mcp` forwarding handler (living in alknet-http) is the
**credential injection point**. It reads from `context.capabilities`, not from
`std::env::var`. aisdk's `Default` impls reading env vars are simply never
called — the assembly layer constructs providers with vault-derived
credentials through the builder API, or the provider's HTTP calls are routed
through `from_openapi` operations that carry the credential in `Capabilities`.

**This must be a spec-level invariant in alknet-call, not a runtime convention.**
The dispatch path (`build_root_context` and `OperationEnv::invoke()` per
ADR-022 §5) populates `OperationContext.capabilities` from the registration
bundle. The invariant is: *no handler reads outbound credentials from any
source other than `OperationContext.capabilities`.* This is already the
architectural intent of ADR-014; the completion work should make it an explicit,
documented invariant that the `from_openapi`/`from_mcp` handler implementations
(in alknet-http) are verified against.

## The "Exchange of Operations" Pattern (Runner / Container Service)

This is the canonical downstream pattern alknet-call completion unblocks, made
explicit here so Phase 1 specs can reference it. Concrete example: the
container service at `/workspace/@alkdev/dispatch` (axum + russh SSH client for
"reverse git runner" over Docker/vast.ai) gets rewritten as a call-protocol
service.

### Bilateral exchange

```
Container service (runs on a vast.ai/docker instance):
  Defines Local ops: /container/exec, /container/list, /container/logs...
  (real handlers — calls bollard or vast.ai API)
  Connects to hub as a CallClient (outbound connection — runner pattern)

Hub (central server):
  Runs CallAdapter (server) on alknet/call (already implemented)
  When the container service connects:
    hub runs from_call → discovers /container/* via services/list + services/schema
    registers them as FromCall provenance (leaf, forwarding handlers) in the
    connection's Layer 2 overlay (ADR-024)
  Now the hub (or anything connected to the hub) can call /container/exec
  The from_call handler forwards over the connection back to the container service

Bilateral: the container service ALSO runs from_call against the hub,
  discovers the hub's External ops, and can call them.
  Connection direction (container → hub) is independent of call direction
  (both can call each other) per ADR-017 §2.
```

### What this requires

1. **`CallClient`** — the container service uses it to open the outbound
   connection to the hub. This is the #1 gap.
2. **`from_call`** — both sides run it to populate their Layer 2 overlays with
   the other side's `External` ops. This is the #2 gap.
3. **`OperationAdapter` trait** — `from_call` implements it. This is the #3 gap
   (enabling, not blocking — `from_call` can be built as a free function before
   the trait exists, but the trait is needed for alknet-http's adapters).

### Why the container service doesn't need alknet-ssh

The current dispatch uses SSH (`channel_open_direct_tcpip`) as the transport
for the "connect back to hub" pattern. Under the call protocol, the container
service is a `CallClient` that dials the hub's `alknet/call` ALPN directly over
QUIC — no SSH in the loop. SSH port forwarding becomes the *transitional*
mechanism for targets that can't run a call-protocol client (the alknet-ssh
phase-0 findings document this transition). Once the container service runs a
`CallClient`, SSH is out of the path entirely.

This is the "dev runner" pattern: a call-protocol client that connects back to
a hub and exposes the core dev tools (bash, fs, etc.) as operations. The other
tools (web search, etc.) plug into the call protocol as additional operations.
The agent service (alknet-agent, downstream) is the consumer that orchestrates
these via `env.invoke()`.

## Implementation Priority Order

Based on the gap analysis and the downstream unblock chain:

1. **`CallClient`** (critical) — outbound connection opener. Without it, no
   runner, no container service, no bilateral exchange. Reuses the existing
   `CallConnection` (which is already implemented) for the dispatch loop; adds
   only the connection-establishment + credential-handling half. This is the
   single highest-value piece of work in the entire alknet-call completion.

2. **`from_call`** (critical, depends on `CallClient`) — discovers remote ops
   via `services/list` + `services/schema`, constructs `HandlerRegistration`
   bundles with `FromCall` provenance, registers them in the connection's
   Layer 2 overlay via `CallConnection::register_imported_all()`. The
   discovery mechanism (`services/list` / `services/schema` specs + handlers)
   is already implemented in `registry/discovery.rs`; `from_call` is the
   client-side consumer of that discovery API.

3. **`OperationAdapter` trait** (enabling) — the async trait
   (`async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>`)
   that `from_call`, `from_openapi`, `from_mcp`, `from_jsonschema` all
   implement. Needed before alknet-http's adapter implementations can be built.
   Small, standalone, unblocks alknet-http Phase 1.

4. **`from_jsonschema`** (medium, standalone) — schema-only registration, no
   handler. Useful for validation/discovery without execution. Distinct from
   `from_call` (no forwarding behavior). Small.

5. **DC-1 resolution** (peer-scoped registry filtering) — the security
   dimension of `CallClient`'s registry. Can be addressed in parallel with #1
   (it's a filtering layer on the registry the `CallClient` exposes, not a
   blocker for the connection-establishment work). Needs an ADR.

## What This Completion Unblocks

| Downstream crate | What it needs from alknet-call | Status without completion |
|-------------------|-------------------------------|--------------------------|
| alknet-http | `OperationAdapter` trait (to implement `from_openapi`/`from_mcp`) | Blocked — can't define HTTP-backed adapters without the trait |
| alknet-ssh | Stable alknet-call types (no adapter dependency) | Not blocked — ssh depends on alknet-core, not alknet-call's adapters. Can proceed in parallel. |
| alknet-agent | `CallClient` (tool dispatch), `from_call` (remote tool import), `OperationAdapter` (provider adapters) | Blocked on `CallClient` + `from_call` |
| Container service (dispatch rewrite) | `CallClient` + `from_call` | Blocked — this is the primary consumer |
| Runner pattern (dev runner, opencode runner) | `CallClient` + `from_call` | Blocked — the runner IS a `CallClient` |
| alknet-napi | `CallClient` (Node.js calls remote ops) | Blocked — NAPI projects `CallClient` to JS |

## Open Questions to Carry into Phase 1

- **OQ-CALL-01 (peer-scoped registry filtering shape)**: the exact mechanism
  for marking `Capabilities` entries or `HandlerRegistration`s as remote-safe
  (DC-1). Needs an ADR. The *existence* of filtering is one-way; the shape is
  two-way.
- **OQ-CALL-02 (`OperationAdapter` error type)**: `AdapterError` enum shape
  (DC-4). Two-way door; record in spec amendment.
- **OQ-CALL-03 (`from_call` re-import trigger)**: auto-on-reconnect vs
  explicit (DC-2). Two-way door; recommend auto-on-reconnect as default.
- **OQ-CALL-04 (namespace collision behavior)**: error on collision (DC-3).
  Two-way door; recommend error as default.

## Next Steps

1. **Resolve DC-1** (peer-scoped registry filtering) — this is the one decision
   that needs an ADR before `CallClient` can be implemented correctly. The
   others (DC-2, DC-3, DC-4) are two-way-door defaults that can be set in the
   spec amendment and revisited during implementation.
2. **Amend the call spec** (`call-protocol.md`, `operation-registry.md`) to
   capture: the `CallClient` gap, the adapter location map, the no-env-vars
   invariant, the exchange-of-operations pattern, and the DC-2/3/4 defaults.
3. **Implement `CallClient`** — the highest-value piece. Reuses `CallConnection`
   for the dispatch loop; adds connection establishment + credentials.
4. **Implement `from_call`** — consumes the already-implemented
   `services/list` + `services/schema` discovery API.
5. **Implement `OperationAdapter` trait** — small, unblocks alknet-http.
6. **Implement `from_jsonschema`** — small, standalone.

## References

- `docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md` —
  the client/adapter contract (specced, partially unimplemented)
- `docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md` —
  registration bundle, provenance, composition authority
- `docs/architecture/decisions/024-operation-registry-layering.md` —
  Layer 0/1/2 overlay model
- `docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md` —
  the no-env-vars invariant's foundation
- `docs/architecture/crates/call/call-protocol.md` — `CallConnection`, Layer 2
  overlay, `compose_root_env`
- `docs/architecture/crates/call/operation-registry.md` — adapter provenance,
  `Capabilities` injection
- `crates/alknet-call/src/` — implementation (verified state above)
- `/workspace/@alkdev/operations/` — TypeScript prior art (`from_openapi.ts`,
  `from_mcp.ts`, `from_schema.ts`, `scanner.ts`)
- `/workspace/@alkdev/dispatch/` — concrete downstream consumer (container
  service / "reverse git runner") this completion unblocks
- `/workspace/aisdk/` — downstream consumer (Rust port of Vercel AI SDK); the
  no-env-vars invariant makes its `std::env::var` reads unreachable
- `/workspace/rust-sdk/` — MCP Rust SDK (rmcp); streamable HTTP transport for
  alknet-http's `from_mcp`/`to_mcp` (separate crate, separate Phase 0)
- `docs/research/alknet-ssh/phase-0-findings.md` — alknet-ssh Phase 0; confirms
  ssh depends on alknet-core not alknet-call's adapters, so it proceeds in
  parallel with this completion
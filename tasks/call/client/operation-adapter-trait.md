---
id: call/client/operation-adapter-trait
name: Define OperationAdapter async trait + AdapterError enum (ADR-017 §5, DC-4/OQ-26)
status: pending
depends_on: [call/registry/handler-registration]
scope: narrow
risk: low
impact: project
level: implementation
---

## Description

Define the `OperationAdapter` async trait and the `AdapterError` crate-level
enum in `src/client/adapter.rs` (or `src/registry/adapter.rs` — pick the
module that keeps the trait near the types it produces). This is the #3 gap
(enabling, not blocking) — `from_call` can be built as a free function before
the trait exists, but the trait is needed before `alknet-http`'s
`from_openapi`/`from_mcp` adapters can be built. Small, standalone, unblocks
`alknet-http` Phase 1.

### The trait (ADR-017 §5)

```rust
#[async_trait]
pub trait OperationAdapter: Send + Sync {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}
```

The trait is **async** because `from_call` requires async discovery
(`services/list` + `services/schema` over a QUIC connection). Sync adapters
(`from_openapi`, `from_mcp` reading a static spec) trivially satisfy an async
trait — their `import()` bodies contain no `.await` points. This is locked by
ADR-017 §5; the async/sync question is decided.

The return type is `Vec<HandlerRegistration>` (not `(OperationSpec, Handler)`
pairs) — ADR-022 changed the registration API to the bundle shape, and
adapters must produce bundles. Adapter convenience methods construct bundles
with `composition_authority: None` and `scoped_env: None` for the leaf ops they
produce.

The `to_*` adapters (`to_openapi`, `to_mcp`) are outbound projections, not
`OperationAdapter` implementations — they consume the registry, they don't
produce entries for it (ADR-017 §5). Do not implement `to_*` here.

### AdapterError (DC-4, OQ-26)

ADR-017 §5 showed `async fn import(&self) -> Vec<HandlerRegistration>` with no
error type. A real implementation needs to handle failures. The trait returns
`Result<Vec<HandlerRegistration>, AdapterError>` where `AdapterError` is a
crate-level enum covering the failure modes real implementations hit:

- `DiscoveryFailed` — `from_call` remote unreachable / `services/list` failed
- `SchemaParse` — `from_openapi` / `from_jsonschema` couldn't parse the spec
- `Transport` — underlying transport error (QUIC for `from_call`, HTTP for
  `from_openapi`/`from_mcp`)
- `Unauthorized` — HTTP 401 for `from_openapi`/`from_mcp`, auth rejected for
  `from_call`
- `Conflict` — namespace collision in `from_call` (DC-3); reuse for other
  adapter collisions

The exact variant set is the two-way-door remainder (OQ-26); the *presence* of
an error type is recorded in `client-and-adapters.md`. Pick the variants
above as the v1 set; add a `#[non_exhaustive]` so `alknet-http`'s adapters can
extend without breaking match arms. Use `thiserror::Error` for the derive
(consistent with the crate's existing error types).

### Where the trait lives

The trait lives in **alknet-call** (where the types — `HandlerRegistration`,
`OperationSpec`, `Handler` — live). The *implementations* live where their
transport dependencies live (the adapter location map, client-and-adapters.md):

- `FromCall` — QUIC-backed (in `alknet-call`, task `call/client/from_call`)
- `FromJsonSchema` — pure parse, no transport (in `alknet-call`, task
  `call/client/from-jsonschema`)
- `FromOpenAPI` — HTTP-backed (in `alknet-http`, separate Phase 0)
- `FromMCP` — MCP streamable-HTTP-backed (in `alknet-http`, feature-gated,
  separate Phase 0)

Do not implement `FromOpenAPI`/`FromMCP` here — those are `alknet-http` tasks.
This task defines the trait + error; `from_call` and `from_jsonschema`
implement it (in their tasks).

### Implementations registered in this task

Optionally implement a trivial `FromJsonSchema` adapter in this task if it
falls out naturally (it's a pure-parse adapter with no transport — see
`call/client/from-jsonschema`). If it doesn't fall out naturally, leave it for
the `from-jsonschema` task; the trait + error alone satisfy this task's
acceptance criteria.

## Acceptance Criteria

- [ ] `OperationAdapter` trait defined: `async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>`
- [ ] Trait is `#[async_trait]` (async — ADR-017 §5, locked)
- [ ] `AdapterError` enum defined with `#[non_exhaustive]` and `thiserror::Error`
- [ ] `AdapterError` variants: `DiscoveryFailed`, `SchemaParse`, `Transport`, `Unauthorized`, `Conflict`
- [ ] Trait + error are `pub` and re-exported from `lib.rs`
- [ ] Trait is located in alknet-call (where the types live), not alknet-http
- [ ] Doc comments link to ADR-017 §5 and client-and-adapters.md
- [ ] Unit test: a trivial test adapter implementing the trait compiles and returns Ok
- [ ] Unit test: a test adapter returning `Err(AdapterError::SchemaParse)` compiles
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — OperationAdapter trait §, Adapter Location Map §
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (the trait contract), Amendments (DC-4 resolution)
- docs/architecture/open-questions.md — OQ-26 (AdapterError variants, two-way-door remainder)
- docs/research/alknet-call-completion/gap-analysis.md — DC-4, implementation priority #3

## Notes

> The trait is async because from_call needs async discovery; sync adapters
> (from_openapi reading a static spec) trivially satisfy it. The trait lives
> in alknet-call (where the types live); implementations live with their
> transport deps (from_call/from_jsonschema here, from_openapi/from_mcp in
> alknet-http). The AdapterError variants are the two-way-door remainder
> (OQ-26) — `#[non_exhaustive]` lets alknet-http extend without breaking. This
> task is small and standalone; it unblocks alknet-http Phase 1's adapter
> implementations. The to_* adapters are projections, not OperationAdapter
> impls — don't implement them here.
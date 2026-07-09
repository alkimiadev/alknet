---
id: call/client/from-jsonschema
name: Implement from_jsonschema adapter (schema-only registration, FromJsonSchema provenance, no handler)
status: completed
depends_on: [call/client/operation-adapter-trait]
scope: narrow
risk: low
impact: isolated
level: implementation
superseded_by: [http/adapters/from-jsonschema]
---

## Description

Implement `from_jsonschema` in `src/client/from_jsonschema.rs`. This is the #4
gap — schema-only registration: produces `HandlerRegistration` bundles with
no handler (`FromJsonSchema` provenance). Used for validation, discovery, and
composition-graph construction without a runtime — type-checking a
composition plan without executing it, building a UI of available operations
without standing up the transports, etc.

### Distinct from from_call (gap analysis DC-5 — confirmed, not a decision)

| | `from_jsonschema` | `from_call` |
|---|---|---|
| Schema source | Provided directly (caller fetches, passes in) | Discovered over wire (`services/list` + `services/schema`) |
| Handler at call time | None (schema-only, `FromJsonSchema` provenance) | Forwards over QUIC (`FromCall` provenance, leaf) |
| Use case | Type validation, discovery, composition graph construction | Actually invoking remote operations |

Keeping them separate preserves the "schema-only, no execution" use case (type
checking, safe composition planning without runtime).

### API

```rust
/// Schema-only registration: produce a HandlerRegistration bundle with
/// FromJsonSchema provenance and no handler. The caller fetches the JSON
/// Schema doc and passes it in; this adapter does no network I/O.
pub fn from_jsonschema(
    spec: OperationSpec,
    schema: serde_json::Value,
) -> HandlerRegistration;
```

The bundle:
- `provenance: FromJsonSchema`
- `composition_authority: None` (no composition — it's schema-only)
- `scoped_env: None` (leaf-equivalent — no reachability)
- `capabilities: Capabilities::new()` (empty — no outbound credentials, no handler to use them)
- `remote_safe: false` (default — ADR-028 §4; provenance-aware default)
- `handler`: a placeholder that returns a `NOT_FOUND`-style or
  `INVALID_INPUT`-style error if ever invoked. Since `FromJsonSchema` ops are
  `Internal`/not-remote-safe by default and have no composition authority, they
  should never be dispatched; the placeholder makes the type-level constraint
  hold (the `Handler` type requires a closure) and fails loudly if a bug routes
  a call to it.

### OperationAdapter impl

`from_jsonschema` implements the `OperationAdapter` trait (from
`operation-adapter-trait`). Because it does no I/O, the `import()` body
contains no `.await` points — it trivially satisfies the async trait.

```rust
pub struct FromJsonSchema {
    spec: OperationSpec,
    schema: serde_json::Value,
}

#[async_trait]
impl OperationAdapter for FromJsonSchema {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError> {
        // No .await — pure parse. Validates schema shape if useful, returns bundle.
        Ok(vec![from_jsonschema(self.spec.clone(), self.schema.clone())])
    }
}
```

If the schema is malformed, return `AdapterError::SchemaParse`.

### Why this is standalone (medium priority)

`from_jsonschema` doesn't depend on `CallClient` or `from_call` — it's pure
parse with no transport. It's sequenced after `operation-adapter-trait` only
because it implements the trait; if the trait lands first, this can proceed
in parallel with `call-client`/`from-call`. It's medium priority because the
primary consumers (runner, container service, agent) need `from_call`, not
`from_jsonschema`; the schema-only use case is validation/discovery tooling.

## Acceptance Criteria

- [ ] `src/client/from_jsonschema.rs` exists with `from_jsonschema` fn + `FromJsonSchema` struct
- [ ] `from_jsonschema` produces a `HandlerRegistration` with `provenance: FromJsonSchema`
- [ ] `composition_authority: None`, `scoped_env: None`, empty `capabilities`
- [ ] `remote_safe: false` (provenance-aware default, ADR-028 §4)
- [ ] Handler placeholder returns an error if invoked (no real handler)
- [ ] `FromJsonSchema` implements `OperationAdapter` (async, no .await in import)
- [ ] Malformed schema returns `AdapterError::SchemaParse`
- [ ] No network I/O (pure parse — caller fetches the doc)
- [ ] Unit test: from_jsonschema produces a bundle with correct provenance + None fields
- [ ] Unit test: placeholder handler returns error when invoked
- [ ] Unit test: OperationAdapter impl returns Ok with one bundle
- [ ] Unit test: malformed schema returns SchemaParse error
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/call/client-and-adapters.md — from_jsonschema §, from_jsonschema vs from_call table
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md — ADR-017 §5 (FromJsonSchema impl)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md — ADR-022 (FromJsonSchema provenance, leaf-equivalent None fields)
- docs/architecture/decisions/028-callclient-peer-scoped-registry-filtering.md — ADR-028 §4 (remote_safe default false)
- tasks/call/client/operation-adapter-trait.md — prerequisite (the trait + AdapterError)
- docs/research/alknet-call-completion/gap-analysis.md — DC-5, implementation priority #4

## Notes

> from_jsonschema is distinct from from_call (DC-5): schema source is
> provided directly (caller fetches), there's no handler at call time, and
> the use case is validation/discovery/composition-graph construction without
> a runtime. It's pure parse with no transport, so it can proceed in parallel
> with call-client/from-call once the trait lands. The placeholder handler
> fails loudly if a bug ever routes a call to a schema-only op — they're
> Internal + not-remote-safe + no composition authority, so dispatch should
> never reach them.
---
id: http/adapters/from-jsonschema
name: Move from_jsonschema to alknet-http as a real HTTP-backed single-endpoint adapter (ADR-066); remove broken placeholder from alknet-call
status: pending
depends_on: [http/adapters/from-openapi]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

`from_jsonschema` was originally implemented in `alknet-call`
(`crates/alknet-call/src/client/from_jsonschema.rs`) as a schema-only
adapter with a `NOT_FOUND`-returning placeholder handler. That is broken:
an operation in the `OperationRegistry` needs a real handler, and a
placeholder that returns `NOT_FOUND` does not work with how the registry
is supposed to function. It was also in the wrong crate — `alknet-call`
is supposed to stay lean (no HTTP client), but a useful `from_jsonschema`
needs reqwest, exactly like `from_openapi`.

ADR-066 moves `from_jsonschema` to `alknet-http` as a real HTTP-backed
single-endpoint adapter, functionally similar to `from_openapi` but
registering one endpoint at a time instead of parsing a full OpenAPI
document. The use case is non-standard, non-OpenAPI, or basic REST
endpoints that don't have a full OpenAPI document — the caller supplies
the schema directly.

This task implements the move: adds the real adapter in `alknet-http`
and removes the broken placeholder from `alknet-call`. The
`FromJsonSchema` provenance variant stays in `alknet-call`
(`OperationProvenance` in `registry/registration.rs`) — only the adapter
implementation moves.

### The adapter (http-adapters.md §"from_jsonschema", ADR-066)

```rust
pub struct FromJsonSchema {
    spec: OperationSpec,
    config: HttpServiceConfig,
    path_template: String,
    method: String,
    http_client: Arc<SharedHttpClient>,
}

impl FromJsonSchema {
    pub fn new(
        spec: OperationSpec,
        config: HttpServiceConfig,
        path_template: String,
        method: String,
        http_client: Arc<SharedHttpClient>,
    ) -> Self;
}

#[async_trait]
impl OperationAdapter for FromJsonSchema {
    async fn import(&self) -> Result<Vec<HandlerRegistration>, AdapterError>;
}
```

The caller supplies:
- An `OperationSpec` (name, op type, input/output JSON Schema,
  `error_schemas`, `access_control`, `visibility`) — the caller already
  has the JSON Schemas; no parsing needed.
- An `HttpServiceConfig` (base URL, auth scheme, default headers — the
  same config type `from_openapi` uses, re-exported from
  `crate::adapters::from_openapi`).
- A path template (e.g. `/users/{id}/posts`) and an HTTP method (e.g.
  `GET`).

### The import flow

The adapter builds one `HandlerRegistration`:
- `spec` = the caller-supplied `OperationSpec` (no parsing, no
  `operationId` normalization — the caller named the op).
- `handler` = a reqwest forwarding handler, **identical in shape to
  `from_openapi`'s**. Reuse `from_openapi`'s `build_request`, `forward`,
  and `forward_stream` functions (or factor the shared logic if those
  are not already reusable as-is — they should be, since the handler
  shape is the same; the only thing that differs is where the path
  template / method / error status codes come from). See
  "Implementation note" below.
- `provenance` = `FromJsonSchema` (leaf, `composition_authority: None`,
  `scoped_env: None` — ADR-022).
- `capabilities` = the credentials the forwarding handler needs (same
  no-env-vars path as `from_openapi` — injected at registration, read
  from `context.capabilities` at call time).

For a `Subscription` op type, register a `StreamingHandler`
(`HandlerKind::Stream`) expecting `text/event-stream`, same as
`from_openapi` (ADR-049). For `Query`/`Mutation`, register
`HandlerKind::Once`.

Returns the single bundle. The caller registers it in the
`OperationRegistry`.

### Implementation note — reuse from_openapi's forwarding logic

`from_openapi` (in `crates/alknet-http/src/adapters/from_openapi.rs`)
already implements the full forwarding handler: `build_request` (path
template substitution, query params, body, auth header injection from
`context.capabilities`), `forward` (the `Once` handler — sends via
`SharedHttpClient`, content-type branching JSON/text/binary, error
mapping), and `forward_stream` (the `Stream` handler — SSE parsing).

These functions are currently free functions in `from_openapi.rs`. They
take `base_url`, `path_template`, `method`, `auth_scheme`,
`default_headers`, `namespace`, `error_status_codes`, `op_type`, `input`,
`context` — exactly the parameters a single-endpoint adapter has. The
cleanest implementation is one of:

1. **Call them directly** if they're `pub(crate)` — `from_jsonschema` is
   in the same crate (`alknet-http`), same module tree
   (`adapters/`). Make `build_request`, `forward`, `forward_stream` (and
   the helpers `value_to_path_segment`, `value_to_query`,
   `parse_sse_frames`) `pub(crate)` and call them from
   `from_jsonschema.rs`.
2. **Factor a small shared module** (e.g. `adapters/forwarding.rs`) if
   you prefer the dependency to be explicit rather than reaching into
   `from_openapi`'s module. This is cleaner if you anticipate a third
   HTTP-backed adapter, but not required for this task.

Either is fine — pick whichever is less code churn. The handler logic
is identical; do not duplicate it. The point of ADR-066 is that
`from_jsonschema` shares `from_openapi`'s forwarding implementation.

### Error fidelity (ADR-023)

Same rule as `from_openapi`: error codes prefixed `HTTP_<status>` to
avoid collision with protocol-level codes. The `error_schemas` come
from the caller-supplied `OperationSpec` (the caller declares them);
the handler maps non-2xx HTTP responses to the declared
`ErrorDefinition` by status code, same as `from_openapi`. If the
`OperationSpec`'s `error_schemas` is empty, fall back to
`HTTP_<status>` (same fallback `from_openapi` uses).

### No-env-vars invariant (ADR-014)

The forwarding handler reads
`context.capabilities.get(config.namespace)`, never `std::env::var`.
Same invariant as `from_openapi`. The handler implementation is verified
against this.

### Removal of the broken placeholder (alknet-call cleanup)

Remove the old broken implementation from `alknet-call`:
- Delete `crates/alknet-call/src/client/from_jsonschema.rs`.
- Remove `mod from_jsonschema;` and the
  `pub use from_jsonschema::{from_jsonschema, FromJsonSchema};` re-export
  from `crates/alknet-call/src/client/mod.rs`.
- Do **not** remove the `FromJsonSchema` variant from
  `OperationProvenance` in `registry/registration.rs` — it stays. It is
  now a handler-bearing leaf (the handler lives in `alknet-http`, but
  the provenance type lives in `alknet-call` where the registry types
  live).

The old task `call/client/from-jsonschema` (the one that built the
broken placeholder) is already marked `status: completed`; this task
supersedes that work. No downstream consumer depends on the old
`from_jsonschema` / `FromJsonSchema` exports from `alknet-call` (the
only consumer was the call crate's own tests, which are removed with
the file).

## Acceptance Criteria

### alknet-http (new adapter)

- [ ] `crates/alknet-http/src/adapters/from_jsonschema.rs` exists with
      `FromJsonSchema` struct + `new()` constructor
- [ ] `FromJsonSchema` holds `spec`, `config`, `path_template`,
      `method`, `http_client`
- [ ] `FromJsonSchema` implements `OperationAdapter` (`import()` returns
      one `HandlerRegistration`)
- [ ] The `HandlerRegistration` has `provenance: FromJsonSchema`,
      `composition_authority: None`, `scoped_env: None`
- [ ] The handler is a real reqwest forwarding handler (not a
      placeholder) — reuses `from_openapi`'s forwarding logic
      (`build_request` / `forward` / `forward_stream`)
- [ ] `Query`/`Mutation` → `HandlerKind::Once`; `Subscription` →
      `HandlerKind::Stream` (ADR-049)
- [ ] Path-template substitution (`{id}` → input value), query params
      from non-path fields, `body` field for the request body
- [ ] Credential injection from `context.capabilities` (Bearer / ApiKey
      / Basic), never `std::env::var` (ADR-014)
- [ ] Response parsing: JSON / text / binary (same content-type
      branching as `from_openapi`)
- [ ] SSE streaming for `Subscription` ops (same `parse_sse_frames` as
      `from_openapi`)
- [ ] Error fidelity: non-2xx mapped to declared `ErrorDefinition` by
      status code, `HTTP_<status>` prefix (ADR-023)
- [ ] `HttpServiceConfig` and `HttpAuthScheme` reused from
      `from_openapi` (not redefined)
- [ ] Exported from `crates/alknet-http/src/adapters/mod.rs`
      (`pub use from_jsonschema::FromJsonSchema;`)
- [ ] Unit test: `import()` produces one `HandlerRegistration` with
      `FromJsonSchema` provenance + `None` authority/env
- [ ] Unit test: forwarding handler builds the correct URL (path
      substitution + query)
- [ ] Unit test: forwarding handler injects Bearer token from
      `context.capabilities`
- [ ] Unit test: `Query` op → `HandlerKind::Once`, `Subscription` op →
      `HandlerKind::Stream`
- [ ] Integration test: forwarding handler calls an external endpoint
      via `SharedHttpClient` and returns the response (use the
      `spawn_echo_server` pattern from `from_openapi`'s tests)
- [ ] Integration test: non-2xx response → declared error code
      (`HTTP_<status>`)
- [ ] Integration test: SSE subscription streams `call.responded` events
- [ ] No `std::env::var` reads in the forwarding handler

### alknet-call (cleanup)

- [ ] `crates/alknet-call/src/client/from_jsonschema.rs` deleted
- [ ] `mod from_jsonschema;` removed from
      `crates/alknet-call/src/client/mod.rs`
- [ ] `pub use from_jsonschema::{from_jsonschema, FromJsonSchema};`
      removed from `crates/alknet-call/src/client/mod.rs`
- [ ] `FromJsonSchema` variant **kept** in `OperationProvenance`
      (`registry/registration.rs`) — do not remove
- [ ] `AdapterError::SchemaParse` doc comment in
      `crates/alknet-call/src/client/mod.rs` — update the
      "`from_openapi` / `from_jsonschema` couldn't parse the spec"
      wording if it now reads oddly (the variant stays; `from_jsonschema`
      in `alknet-http` can still return `SchemaParse` if the
      caller-supplied `OperationSpec` is somehow invalid, though that's
      unlikely since the caller constructs it — use judgment)

### Build / lint

- [ ] `cargo build -p alknet-call -p alknet-http` succeeds
- [ ] `cargo test -p alknet-call -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-call -p alknet-http --all-targets` succeeds
      with no warnings
- [ ] `cargo fmt --check` succeeds

## References

- docs/architecture/decisions/066-from-jsonschema-as-http-adapter.md —
  ADR-066 (the decision this task implements)
- docs/architecture/crates/http/http-adapters.md — §"from_jsonschema"
  (the spec: API, flow, relationship to from_openapi, origin)
- docs/architecture/crates/call/client-and-adapters.md — §"from_jsonschema"
  (the move note + pointer to the http spec)
- docs/architecture/decisions/017-call-protocol-client-and-adapter-contract.md —
  ADR-017 §5 (amended — `from_jsonschema` clause superseded by ADR-066)
- docs/architecture/decisions/022-handler-registration-provenance-and-composition-authority.md —
  ADR-022 (amended — `FromJsonSchema` row now handler-bearing leaf)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023
  (`HTTP_<status>` prefix, error fidelity)
- docs/architecture/decisions/049-streaming-handler-for-subscriptions.md —
  ADR-049 (`StreamingHandler` for `Subscription` ops)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md —
  ADR-014 (no-env-vars invariant)
- tasks/http/adapters/from-openapi.md — the `from_openapi` task
  (completed; the forwarding logic to reuse lives in its implementation)
- tasks/http/client/shared-http-client.md — `SharedHttpClient` (the
  shared reqwest client both adapters use)
- tasks/call/client/from-jsonschema.md — the old task that built the
  broken placeholder (superseded by this task)

## Notes

> This is a contained move, not new architecture. ADR-066 already
> landed the architecture; the spec docs already reflect the move.
> The implementation is: (1) write the real adapter in `alknet-http`
> reusing `from_openapi`'s forwarding logic (`build_request` /
> `forward` / `forward_stream` — make them `pub(crate)` or factor a
> shared `adapters/forwarding.rs`, whichever is less churn), (2) delete
> the broken placeholder from `alknet-call`, (3) keep the
> `FromJsonSchema` provenance variant in `registration.rs`. The handler
> shape is identical to `from_openapi` — do not duplicate the
> forwarding logic. The difference between the two adapters is purely
> the input shape: `from_openapi` parses a full OpenAPI doc and
> iterates `(path, method)` pairs; `from_jsonschema` takes one
> `(path_template, method)` from the caller and produces one bundle.
> Risk is low because the forwarding logic is already tested in
> `from_openapi`; the new code is the thin `FromJsonSchema` adapter
> struct + `import()` + wiring.
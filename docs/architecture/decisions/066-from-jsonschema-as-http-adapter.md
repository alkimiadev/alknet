# ADR-066: `from_jsonschema` as an HTTP-Backed Single-Endpoint Adapter in alknet-http

## Status

Accepted (supersedes the `from_jsonschema` clause of ADR-017 §5 and the
`FromJsonSchema` provenance row of ADR-022 — both described a schema-only,
no-handler adapter in `alknet-call`)

## Context

`from_jsonschema` was originally specified (ADR-017 §5) as a schema-only
adapter living in `alknet-call`: it produced `HandlerRegistration` bundles
with a `NOT_FOUND`-returning placeholder handler and `FromJsonSchema`
provenance. The stated use case was validation, discovery, and
composition-graph construction without a runtime — type-checking a
composition plan without executing it, building a UI of available
operations without standing up the transports.

This is broken. An operation in the `OperationRegistry` needs a real
handler. A placeholder that returns `NOT_FOUND` does not work with how
the registry is supposed to function: an `Internal` op registered with
a dead handler is a trap, not a feature. The "schema-only, no handler"
concept conflated two things — schema *validation* (a compile-time /
planning activity that doesn't need a registry entry at all) and
operation *registration* (which always needs a handler). Validation
against a JSON Schema does not require a `HandlerRegistration`; it
requires the schema and a validator. Registering an operation requires
a handler. The old `from_jsonschema` tried to do the former by abusing
the latter, and produced something that works for neither.

The misplacement was compounded by a location error: the adapter lived
in `alknet-call` (which is supposed to stay lean — no HTTP client), but
a `from_jsonschema` that is actually useful for calling non-standard
endpoints needs reqwest, exactly like `from_openapi` and `from_mcp`.
The adapter location map in ADR-017 / `client-and-adapters.md` already
establishes that HTTP-backed adapters live in `alknet-http`; the old
`from_jsonschema` violated its own stated principle by living in
`alknet-call`.

A concrete use case now forces the decision: composing a non-standard,
non-OpenAPI, basic REST endpoint that does not have a full OpenAPI
document. The endpoint has a method, a URL, an input/output JSON Schema,
and an auth scheme — but no `paths` object, no `operationId`, no
`components`. `from_openapi` requires an OpenAPI document; this endpoint
doesn't have one. The gap is: register a single HTTP endpoint as a
call-protocol operation, one at a time, with the caller supplying the
schema directly.

## Decision

`from_jsonschema` becomes an HTTP-backed single-endpoint adapter in
`alknet-http`, functionally similar to `from_openapi` but registering
one endpoint at a time instead of parsing a full OpenAPI document:

1. **Move the adapter implementation to `alknet-http`**
   (`crates/alknet-http/src/adapters/from_jsonschema.rs`). The
   forwarding handler uses the same reqwest-backed `SharedHttpClient`
   and the same no-env-vars credential injection as `from_openapi`. The
   adapter implements `OperationAdapter` (the trait from `alknet-call`,
   ADR-017 §5 — unchanged).

2. **Give it a real forwarding handler.** A `from_jsonschema`-imported
   operation is a leaf with a reqwest forwarding handler, identical in
   shape to a `from_openapi`-imported operation — it builds an HTTP
   request from the input (path/query/body split per a path template),
   injects credentials from `context.capabilities`, sends via the shared
   HTTP client, and parses the response (JSON, text, or binary — same
   content-type branching as `from_openapi`). For a `Subscription`
   op type with `text/event-stream` response, it registers a
   `StreamingHandler` (ADR-049), same as `from_openapi`.

3. **Single-endpoint registration.** The caller supplies:
   - An `OperationSpec` (name, op type, input/output JSON Schema,
     `error_schemas`, `access_control`, `visibility`).
   - An `HttpServiceConfig` (base URL, auth scheme, default headers —
     the same config type `from_openapi` uses).
   - A path template + HTTP method (the one endpoint).

   The adapter builds one `HandlerRegistration` with `FromJsonSchema`
   provenance and a real forwarding handler. The caller registers it in
   the `OperationRegistry`. This is the "one endpoint at a time" shape:
   no `paths` object to iterate, no `operationId` to normalize.

4. **`FromJsonSchema` provenance stays in `alknet-call`** (in the
   `OperationProvenance` enum, `registration.rs`). The provenance type
   lives where the registry types live; only the adapter implementation
   moves. `FromJsonSchema` is now a leaf provenance — it has a handler
   (a reqwest forwarding handler), same trust model as `FromOpenAPI`
   (HTTP endpoint trusted; handler is a forwarding stub).

5. **Remove the "schema-only, no handler" concept.** The placeholder
   handler and the "schema-only ops are `Internal`, so dispatch should
   never reach them" rationale are removed. An op registered with
   `FromJsonSchema` provenance is a real, callable, HTTP-forwarding
   operation — `Internal` by default (adapter-registered ops are
   composition material, ADR-015), but it actually forwards if invoked.

   The schema-validation-without-a-handler use case (type-checking a
   composition plan, building a UI) does not require a
   `HandlerRegistration` at all. That use case is served by consuming
   the `OperationSpec` directly (the spec already carries the input/
   output JSON Schemas); no adapter, no registry entry, no handler is
   needed. If a future use case requires registering a schema-only op
   for discovery purposes, that is a separate feature and would warrant
   its own ADR — it is not what `from_jsonschema` is.

### Relationship to `from_openapi`

| | `from_openapi` | `from_jsonschema` |
|---|---|---|
| Input | A full OpenAPI 3.x document (JSON or YAML) | A single endpoint: `OperationSpec` + `HttpServiceConfig` + path template + method |
| Granularity | One `HandlerRegistration` per `(path, method)` in the doc | One `HandlerRegistration` per call |
| Schema source | Parsed from the OpenAPI doc (parameters, request body, responses) | Supplied directly by the caller |
| Handler | reqwest forwarding handler (shared HTTP client) | Same reqwest forwarding handler |
| Provenance | `FromOpenAPI` | `FromJsonSchema` |
| Location | `alknet-http` | `alknet-http` |
| Use case | Standard OpenAPI APIs (GitHub, OpenAI, Anthropic) | Non-standard, non-OpenAPI, or basic REST endpoints without a full spec |

The two adapters share the forwarding-handler implementation, the
credential injection path, the error-fidelity rule (`HTTP_<status>`
prefix, ADR-023), and the no-env-vars invariant (ADR-014). The
difference is purely the input shape: a full document vs. a single
endpoint.

## Consequences

**Positive**:
- `from_jsonschema` actually works — it has a real handler, not a
  placeholder. A concrete use case (non-standard REST endpoints) is
  served.
- The adapter location is consistent: all HTTP-backed adapters
  (`from_openapi`, `from_mcp`, `from_jsonschema`) live in `alknet-http`,
  where reqwest is. `alknet-call` stays lean.
- The "schema-only, no handler" trap is removed. An op in the registry
  is always callable.
- `FromJsonSchema` provenance becomes a real leaf, consistent with
  `FromOpenAPI`/`FromMCP`/`FromCall`.

**Negative**:
- The schema-validation-without-a-handler use case (the original stated
  purpose) is no longer served by `from_jsonschema`. That use case is
  served by consuming `OperationSpec` directly, but any code that relied
  on the placeholder handler returning `NOT_FOUND` breaks. The only
  existing consumer is the call crate's own tests; no downstream consumer
  depended on this — the placeholder was a trap, not a contract.
- `alknet-call` loses a public export (`from_jsonschema`, `FromJsonSchema`
  the adapter struct). The `FromJsonSchema` provenance variant stays;
  the adapter struct moves. Downstream consumers that referenced the
  adapter (none currently) would need to use `alknet-http`'s re-export.

**Neutral**:
- `FromJsonSchema` provenance is now a leaf (handler-bearing), not a
  "no handler" provenance. The ADR-022 table row updates: it can compose?
  No. Has composition authority? No. Default visibility? Internal. Trust
  model? HTTP endpoint trusted; handler is a forwarding stub. This
  aligns with the other leaves. ADR-017 §5 and ADR-022's provenance
  table/enum-doc are amended (2026-07-09) to point here — the
  supersession is recorded in the superseded ADRs, not only in this one.

## References

- Supersedes the `from_jsonschema` clause of
  [ADR-017](017-call-protocol-client-and-adapter-contract.md) §5
  ("`FromJsonSchema` — imports from a JSON Schema definition (schema-only,
  no handler)") and the operational spec in
  `docs/architecture/crates/call/client-and-adapters.md` §"from_jsonschema".
- Supersedes the `FromJsonSchema` row of
  [ADR-022](022-handler-registration-provenance-and-composition-authority.md)
  (the "no handler — schema only" framing).
- Aligns with the adapter location principle in
  [ADR-017](017-call-protocol-client-and-adapter-contract.md) §5 and
  `client-and-adapters.md` §"Adapter Location Map": HTTP-backed adapters
  live in `alknet-http`.
- Reuses the forwarding handler, credential injection, error fidelity
  (`HTTP_<status>` prefix, [ADR-023](023-operation-error-schemas.md)),
  streaming shape ([ADR-049](049-streaming-handler-for-subscriptions.md)),
  and no-env-vars invariant ([ADR-014](014-secret-material-flow-and-capability-injection.md))
  established by `from_openapi`.
- Reuses `HttpServiceConfig` and `SharedHttpClient` from
  `from_openapi` (in `alknet-http`).
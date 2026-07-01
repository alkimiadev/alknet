---
id: http/gateway/error-mapping
name: Implement CallError-to-HTTP-status error mapping (ADR-023)
status: completed
depends_on: [http/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the `CallError` code → HTTP status code mapping in
`src/gateway/error.rs`. This is the error-mapping table the HTTP server's
gateway endpoints use to translate call-protocol `CallError` codes into
HTTP response status codes (ADR-023). The mapping is a two-way-door
default (the exact status for ambiguous codes can be refined
additively); the one-way constraint is that protocol-level and
operation-level codes are distinct (ADR-023) and `from_openapi`-imported
codes are prefixed `HTTP_<status>` to avoid collision with protocol
codes.

### The mapping table (http-server.md §"Error Mapping")

| Call `code` | HTTP status | Notes |
|-------------|-------------|-------|
| `NOT_FOUND` (operation not registered, or Internal op) | `404` | |
| `FORBIDDEN` (insufficient scopes, or unauthenticated) | `401` (no token) / `403` (token present) | |
| `INVALID_INPUT` (schema mismatch) | `422` | |
| `TIMEOUT` | `504` | `retryable: true` |
| `INTERNAL` | `500` | |
| Operation-level domain code with `http_status` (ADR-023) | the declared `http_status` | `from_openapi`-imported ops carry the original status |
| Operation-level domain code without `http_status` | `500` | |

### The `HTTP_<status>` prefix rule (ADR-023 §5)

`from_openapi` maps OpenAPI non-2xx response status codes to
`ErrorDefinition`s with codes prefixed `HTTP_` + the status number:

```rust
// OpenAPI: 404: { schema: NotFoundError }
// → ErrorDefinition { code: "HTTP_404", http_status: Some(404), schema: NotFoundError }
```

The normative rule (review #002 W20): `from_openapi` must not produce
error codes that collide with the five protocol-level codes (`NOT_FOUND`,
`FORBIDDEN`, `INVALID_INPUT`, `INTERNAL`, `TIMEOUT`). The `HTTP_<status>`
prefix enforces this.

### `retryable` → `Retry-After` hint

The `retryable` field from `CallError` maps to an HTTP `Retry-After` hint
for `503`/`429`-class errors (operation-level codes with `http_status` in
that range). The hint is optional; if the operation-level error does not
carry a retry-after value, no header is added.

### API

```rust
/// Map a CallError to an HTTP status code (ADR-023).
pub fn call_error_to_http_status(error: &CallError) -> u16;

/// Map a CallError to an HTTP response, including the Retry-After hint
/// when applicable. The body is the serialized CallError (or its
/// `details` field).
pub fn call_error_to_http_response(error: &CallError) -> axum::response::Response;
```

The `FORBIDDEN` case needs the caller's identity state to distinguish
`401` (no token) from `403` (token present but insufficient scopes). The
mapping function takes an `Option<Identity>` (or a flag) so the gateway
endpoint can pass the resolved identity through:

```rust
/// Map a CallError to an HTTP status code, considering whether the caller
/// was authenticated (FORBIDDEN → 401 if no identity, 403 if identity
/// present but insufficient scopes).
pub fn call_error_to_http_status_with_identity(
    error: &CallError,
    identity: Option<&Identity>,
) -> u16;
```

### What this task does NOT do

- **No `to_openapi` error projection.** `to_openapi` projects
  `error_schemas` to the gateway endpoint's response definitions (the
  OpenAPI doc's `responses` block). That is the `to-openapi` task, not
  this one. This task is the runtime HTTP response mapping.
- **No `from_openapi` error import.** `from_openapi` builds
  `ErrorDefinition`s from OpenAPI non-2xx responses with the `HTTP_<status>`
  prefix. That is the `from-openapi` task. This task consumes the
  resulting `CallError` codes at runtime.

## Acceptance Criteria

- [ ] `call_error_to_http_status(error: &CallError) -> u16` implemented
- [ ] `NOT_FOUND` → 404
- [ ] `FORBIDDEN` → 401 (no identity) / 403 (identity present)
- [ ] `INVALID_INPUT` → 422
- [ ] `TIMEOUT` → 504
- [ ] `INTERNAL` → 500
- [ ] Operation-level code with `http_status` → declared status
- [ ] Operation-level code without `http_status` → 500
- [ ] `HTTP_<status>`-prefixed codes (from `from_openapi`) → the status number
- [ ] `call_error_to_http_response(error)` builds an `axum::response::Response` with the status + JSON body
- [ ] `retryable: true` on `503`/`429`-class errors → `Retry-After` header (when value present)
- [ ] `call_error_to_http_status_with_identity(error, identity)` for the 401/403 split
- [ ] Unit test: each protocol code maps to the correct status
- [ ] Unit test: operation-level code with `http_status` maps to declared status
- [ ] Unit test: operation-level code without `http_status` maps to 500
- [ ] Unit test: `HTTP_404` code maps to 404 (not collided with protocol `NOT_FOUND`)
- [ ] Unit test: `FORBIDDEN` with `None` identity → 401
- [ ] Unit test: `FORBIDDEN` with `Some(identity)` → 403
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-server.md — Error Mapping table (§"Error Mapping")
- docs/architecture/crates/http/http-adapters.md — Error Fidelity (§"Error Fidelity (ADR-023)")
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (protocol/operation codes distinct, HTTP_<status> prefix)

## Notes

> The mapping is a two-way-door default (the exact status for ambiguous
> codes can be refined additively); the one-way constraint is that
> protocol-level and operation-level codes are distinct (ADR-023) and
> from_openapi-imported codes are prefixed HTTP_<status>. The FORBIDDEN
> case needs the caller's identity state to distinguish 401 (no token)
> from 403 (token present but insufficient scopes). This task is the
> runtime HTTP response mapping; the to_openapi doc-level error
> projection is the to-openapi task, and the from_openapi error import
> is the from-openapi task.

## Summary

> Implemented call_error_to_http_status, call_error_to_http_status_with_identity
> (FORBIDDEN→401 no identity / 403 identity present), and call_error_to_http_response
> (JSON body + Retry-After for retryable 503/429 with details.retry_after) in
> src/gateway/error.rs. Five protocol codes map to fixed statuses. HTTP_<status>-
> prefixed operation-level codes parse status from prefix (no collision). Unknown
> operation-level codes default to 500. 21 unit tests. Build/clippy/test all clean.
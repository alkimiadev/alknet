# ADR-023: Operation Error Schemas

## Status

Accepted (amended by ADR-049 — protocol-level code list extended to six)

## Context

The `OperationSpec` in alknet-call has `input_schema` and `output_schema` but
no `error_schemas`. The `call.error` payload (call-protocol.md L128–134)
carries a `code` and `message`, where `code` is one of six infrastructure
codes: `NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`, `INVALID_OPERATION_TYPE`,
`INTERNAL`, `TIMEOUT`.

These six codes cover **protocol-level failures** — the call protocol
itself can always fail to find an operation, deny access, reject bad input,
reject the wrong dispatch method for the operation type, time out, or hit
an internal error. They are emitted by the dispatch machinery (the registry,
the adapter), not by operation handlers. `INVALID_OPERATION_TYPE` was added
by ADR-049 (streaming handler for subscriptions — `invoke()` called on a
`Subscription`, or `invoke_streaming()` on a `Query`/`Mutation`).

But operations also have **domain-level failures** that are not covered:

- `/fs/readFile` can fail because the file doesn't exist, the path is
  invalid, or the caller lacks OS-level read permission. These are
  operation-specific failures distinct from the protocol-level
  `INVALID_INPUT` (schema mismatch) or `FORBIDDEN` (scope mismatch).
- `/vastai/createMachine` can fail because the account has insufficient
  credits, the machine type is unavailable in the requested region, or the
  upstream API rate-limited the request.
- `/agent/chat` can fail because the LLM provider returned an error, the
  context window overflowed, or the model refused the request.

Today, these failures collapse into `INTERNAL` with a `message` string.
A client calling `/fs/readFile` has no way to know from the schema that it
might return `FILE_NOT_FOUND` vs `PERMISSION_DENIED` vs `INVALID_PATH`. The
caller has to parse `message` strings — the exact anti-pattern that typed
RPC is meant to avoid. This is a **type safety gap**: inputs and outputs are
typed, but errors are untyped strings.

### Why this matters for adapters

OpenAPI specs naturally include error information — response status codes
with schemas (e.g., `404: { schema: NotFoundError }`, `422: { schema:
ValidationError }`). MCP tool definitions carry error descriptions. The
`from_openapi` adapter (ADR-017 L113–124) imports operations and mirrors
"the remote operation's name, namespace, type, schemas, and access control"
— but with no error schema field, error responses from the OpenAPI source
are dropped on import. `to_openapi` has nowhere to project error information
to. The same gap applies to `from_mcp`/`to_mcp`.

An OpenAPI operation that declares:

```yaml
responses:
  '200': { schema: MachineList }
  '401': { schema: AuthError }
  '429': { schema: RateLimitError }
```

cannot be faithfully represented in alknet's `OperationSpec` today. The
adapter would import the `200` output schema and drop the error schemas —
a lossy import that silently discards the operation's failure contract.

### Prior art

The TypeScript reference (`/workspace/@alkdev/operations/src/types.ts`
L38–47, L94, L112) defines `ErrorDefinitionSchema` and an optional
`errorSchemas?: ErrorDefinition[]` on `OperationSpec`:

```typescript
export const ErrorDefinitionSchema = Type.Object({
  code: Type.String({ description: "Error Code e.g., INVALID_INPUT, NOT_FOUND, UNAUTHORIZED" }),
  description: Type.String(),
  schema: Type.Unknown(),
  httpStatus: Type.Optional(Type.Number()),
});
```

The `mapError()` function (`error.ts` L25–51) matches thrown errors against
the declared error schemas by code prefix — if a handler throws an error
whose message starts with a declared code, `mapError` rewrites it to a
typed `CallError` with that code. This is a proven pattern: operations
declare their error contract, the dispatch machinery maps runtime failures
to the declared codes, and clients get typed errors instead of string
parsing.

The translator agent omitted `errorSchemas` from the Rust spec, likely
because it's `Optional` in the TS schema (so dropping it doesn't break the
happy path) and because error schemas are semantically different from
input/output schemas (an operation returns one output but could return any
of several errors). That's a reasonable judgment call for a first
translation pass, but it leaves a real gap for adapters and clients.

### The general principle

This is the same principle as the Safe Exit protocol in the SDD process
(docs/sdd_process.md L19, L423): **make failure a typed, declared thing
rather than an untyped exception that crashes into whoever's listening.**
An operation that declares "I can fail with `FILE_NOT_FOUND`" is the same
shape as an agent that declares "I can fail with `TASK_AMBIGUOUS`" — both
turn an unknown unknown into a known known that the caller can handle
deliberately.

Complex systems survive not because every component is reliable, but
because failure is expected and typed. Cells have apoptosis (a declared
failure mode that protects the organism). Operations have error schemas (a
declared failure mode that lets the caller handle it). The alternative —
components that fail with untyped strings — is how you get brittle clients
that string-match error messages and break when the message wording
changes.

## Decision

### 1. `OperationSpec` gains an optional `error_schemas` field

```rust
pub struct OperationSpec {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub visibility: Visibility,
    pub input_schema: Value,
    pub output_schema: Value,
    pub access_control: AccessControl,
    pub error_schemas: Vec<ErrorDefinition>,  // NEW — empty vec = no declared errors
}

pub struct ErrorDefinition {
    /// Machine-readable error code. e.g., "FILE_NOT_FOUND", "RATE_LIMITED",
    /// "INSUFFICIENT_CREDITS". Distinct from the protocol-level codes
    /// (NOT_FOUND, FORBIDDEN, etc.) — these are operation-level domain codes.
    pub code: String,

    /// Human-readable description of when this error occurs.
    pub description: String,

    /// JSON Schema for the error detail payload. The `call.error` event's
    /// `details` field conforms to this schema when this error code is
    /// returned. `Value` (serde_json::Value) carrying a JSON Schema, same
    /// as input_schema/output_schema.
    pub schema: Value,

    /// HTTP status code for adapter projection. `from_openapi` maps OpenAPI
    /// response status codes to error definitions; `to_openapi` projects
    /// error definitions back to response status codes. Optional — not all
    /// error sources are HTTP-backed.
    pub http_status: Option<u16>,
}
```

`error_schemas` is a `Vec<ErrorDefinition>`, not `Option<Vec<...>>`. An
empty vec means "this operation declares no specific domain errors" (it may
still fail with protocol-level codes like `INTERNAL`). This avoids the
`None` vs `Some([])` ambiguity and matches the TypeScript reference's
optional-array convention.

### 2. The `call.error` payload gains an optional `details` field

```json
{
  "code": "FILE_NOT_FOUND",
  "message": "file not found: /etc/nonexistent",
  "retryable": false,
  "details": { "path": "/etc/nonexistent", "errno": 2 }
}
```

- `code` — the error code. Either a protocol-level code (`NOT_FOUND`,
  `FORBIDDEN`, `INVALID_INPUT`, `INVALID_OPERATION_TYPE`, `INTERNAL`,
  `TIMEOUT`) or an operation-level domain code from `error_schemas` (e.g.,
  `FILE_NOT_FOUND`, `RATE_LIMITED`).
- `message` — human-readable error message. Unstructured — for logging and
  debugging, not for programmatic handling. Clients should switch on
  `code`, not parse `message`.
- `retryable` — whether the caller should retry. `true` for transient
  failures (`TIMEOUT`, `RATE_LIMITED`), `false` for permanent ones
  (`NOT_FOUND`, `FORBIDDEN`, `FILE_NOT_FOUND`).
- `details` — optional. When the error code matches a declared
  `ErrorDefinition`, `details` conforms to that definition's `schema`. When
  the error is protocol-level (`NOT_FOUND`, `FORBIDDEN`, etc.), `details`
  is absent or carries protocol-specific context (e.g., the operation name
  for `NOT_FOUND`). This field is the typed error payload — it's what
  makes errors structured instead of string-matched.

### 3. Protocol-level vs operation-level error codes

The six existing codes are **protocol-level** — emitted by the dispatch
machinery, not by handlers:

| Code | Emitted by | Meaning |
|------|-----------|---------|
| `NOT_FOUND` | Registry | Operation not registered (or Internal op called from wire) |
| `FORBIDDEN` | Registry / ACL | Caller lacks required scopes, or unauthenticated |
| `INVALID_INPUT` | Registry | Input doesn't match `input_schema` |
| `INVALID_OPERATION_TYPE` | Registry / `OperationEnv` | Wrong dispatch path for the operation's type (`invoke()` on a `Subscription`, `invoke_streaming()` on a `Query`/`Mutation`, or `OperationEnv::invoke()` on a `Subscription` during composition — ADR-049) |
| `INTERNAL` | Registry / Adapter | Handler panic, unhandled error, connection failure |
| `TIMEOUT` | Adapter | Request timed out |

Operation-level domain codes are emitted by **handlers** — the operation's
own logic determines what went wrong. They are declared in `error_schemas`
and appear in the `code` field of `call.error`. Examples: `FILE_NOT_FOUND`,
`PERMISSION_DENIED`, `RATE_LIMITED`, `INSUFFICIENT_CREDITS`,
`CONTEXT_OVERFLOW`.

The two namespaces are distinct but share the `code` field. Clients
should handle protocol-level codes uniformly (they mean the same thing
regardless of operation) and operation-level codes per-operation (they
mean what the operation's `error_schemas` says they mean). Unknown codes
— whether a future protocol code or an undeclared operation code — should
be treated as `INTERNAL` with `retryable: false` (same as the current
guidance in call-protocol.md L143).

### 4. Handler error mapping

When a handler returns an error, the dispatch machinery maps it to a
`call.error` event. The mapping:

1. If the handler returns a structured error with a `code` that matches a
   declared `ErrorDefinition.code`, the `call.error` carries that code and
   the error's detail payload (validated against the definition's `schema`).
2. If the handler returns a structured error with a `code` that doesn't
   match any declared `ErrorDefinition`, the `call.error` carries
   `INTERNAL` with the original code in `details`. This is an undeclared
   error — the handler returned a typed error but didn't declare it.
3. If the handler returns an unstructured error (a string, a generic
   `Error`, a panic), the `call.error` carries `INTERNAL` with
   `retryable: false`. This is the current behavior for all handler
   errors.

The TypeScript `mapError()` function (error.ts L25–51) implements case 2
and 3 by matching error messages against declared codes. The Rust
implementation can use a typed error return from the handler (`Result<Value,
CallError>` where `CallError` carries a `code`), which is cleaner than
message-string matching — the handler returns a typed error, the registry
checks whether the code is declared, and the `call.error` is constructed
accordingly.

### 5. `from_openapi` and `to_openapi` error fidelity

`from_openapi` maps OpenAPI response status codes to `ErrorDefinition`s:

```rust
// OpenAPI: 404: { schema: NotFoundError }
// → ErrorDefinition { code: "HTTP_404", http_status: Some(404), schema: NotFoundError }
```

**Normative rule (review #002 W20)**: `from_openapi` must not produce error
codes that collide with the six protocol-level codes (`NOT_FOUND`,
`FORBIDDEN`, `INVALID_INPUT`, `INVALID_OPERATION_TYPE`, `INTERNAL`,
`TIMEOUT`). The adapter prefixes
imported error codes with `HTTP_` and the status number (e.g., `HTTP_404`,
`HTTP_429`) to avoid collision. This is a requirement for the adapter, not
a naming convention — the `from_openapi` example above was previously shown
producing `NOT_FOUND` from a 404, which collided with the protocol-level
`NOT_FOUND` (operation not registered). The `details` field disambiguates
in practice (present for operation-level, absent for protocol-level), but
ADR-023 says "clients should switch on `code`, not parse `message`" — so
the `code` alone must be unambiguous. Operations that hand-write their own
`ErrorDefinition`s should use domain-specific codes (`FILE_NOT_FOUND`,
`RATE_LIMITED`) rather than reusing protocol codes.

The adapter maps the OpenAPI error schema to alknet's JSON Schema format
(same conversion as input/output schemas). The `http_status` field records
the original status code so `to_openapi` can project it back.

`to_openapi` projects `error_schemas` back to OpenAPI response definitions:

```yaml
responses:
  '200': { schema: <output_schema> }
  '404': { schema: <error_schemas[0].schema> }  # where http_status = 404
  '429': { schema: <error_schemas[1].schema> }  # where http_status = 429
```

This makes the adapter contract from ADR-017 faithful on the error axis —
no silent dropping of error contracts.

`from_mcp` and `to_mcp` follow the same pattern: MCP tool definitions carry
error descriptions, and the adapters map them to/from `ErrorDefinition`s.

### 6. `services/schema` exposes error schemas

`services/schema` returns the full `OperationSpec` including `error_schemas`.
A client querying `/services/schema` for `/fs/readFile` gets:

```json
{
  "name": "fs/readFile",
  "namespace": "fs",
  "op_type": "query",
  "input_schema": { ... },
  "output_schema": { ... },
  "error_schemas": [
    { "code": "FILE_NOT_FOUND", "description": "The file does not exist",
      "schema": { "type": "object", "properties": { "path": { "type": "string" } } },
      "http_status": null },
    { "code": "PERMISSION_DENIED", "description": "OS-level read permission denied",
      "schema": { "type": "object", "properties": { "path": { "type": "string" }, "errno": { "type": "integer" } } },
      "http_status": null }
  ]
}
```

This enables client code generation: a TypeScript or Rust client generator
reading the schema can produce a typed `Result<Output, FsReadFileError>`
enum instead of a generic `Result<Output, string>`.

## Consequences

**Positive:**

- Operations declare their failure modes. Clients get typed errors instead
  of string-matched messages. This is the same type-safety property that
  `input_schema` and `output_schema` provide, extended to the error axis.
- `from_openapi` and `to_openapi` are faithful on the error axis. An
  OpenAPI operation's error contract is no longer silently dropped on
  import or absent on export. The adapter contract from ADR-017 is now
  complete.
- Client code generation can produce typed error enums. A client calling
  `/fs/readFile` can match on `FILE_NOT_FOUND` vs `PERMISSION_DENIED`
  instead of parsing `message` strings.
- The protocol-level vs operation-level distinction is explicit. Protocol
  codes (`NOT_FOUND`, `FORBIDDEN`, etc.) mean the same thing regardless of
  operation. Operation codes (`FILE_NOT_FOUND`, `RATE_LIMITED`) mean what
  the operation declares. No conflation.
- The `details` field carries structured error context that conforms to a
  schema — the error payload is typed, not a bare string. This enables
  programmatic error handling (retry logic, user-facing error messages,
  logging) without string parsing.
- The principle generalizes: making failure a typed, declared thing is the
  same pattern as the SDD process's Safe Exit protocol (typed agent
  failure) and the same pattern complex biological systems use (apoptosis
  as a declared cell failure mode). The more components declare their
  failure modes, the more robust the system.

**Negative:**

- `OperationSpec` gains a field. Operations that don't declare errors
  (empty `error_schemas` vec) still work — the field is additive. But
  operations that *should* declare errors and don't will produce `INTERNAL`
  with `retryable: false`, same as today. The gap is visible but not
  enforced — an operation can ship without error schemas and clients get
  untyped errors for it. This is a documentation/guidance issue, not a
  type-system issue.
- The `call.error` payload gains a `details` field. This is a wire-format
  addition. Existing clients that only read `code` and `message` are
  unaffected (they ignore `details`). New clients can read `details` for
  structured error context. This is backward-compatible — `details` is
  optional and absent for protocol-level errors.
- Handler error mapping adds a step to the dispatch path: the registry
  checks whether the handler's error code matches a declared
  `ErrorDefinition`. This is a `HashMap` lookup by code — negligible cost.
- The `http_status` field on `ErrorDefinition` is HTTP-specific. Operations
  that aren't HTTP-backed (local, session, from_mcp) leave it as `None`.
  This is a pragmatic choice: `from_openapi`/`to_openapi` need it, and it's
  optional for everything else. A future non-HTTP adapter that needs a
  different error projection field would add it — but `http_status` covers
  the immediate use case.
- The TypeScript `mapError()` uses message-string matching to map thrown
  errors to codes. The Rust implementation can do better (typed `CallError`
  return from handlers), but this means the `Handler` type's return is
  `Result<Value, CallError>` rather than `Result<Value, Box<dyn Error>>`.
  This is a cleaner API but a slight constraint on handler authors — they
  return typed errors, not generic ones. Mitigated: `CallError::internal()`
  is available for errors that don't fit a declared code.

## Assumptions

1. **Operations can enumerate their meaningful failure modes at
   registration time.** If an operation has failure modes that are only
   discoverable at runtime (e.g., a dynamic API that returns novel error
   codes), those would be `INTERNAL` with `details` carrying the upstream
   error. The assumption is that most operations have a knowable set of
   domain errors.

2. **Error codes are stable per operation.** Once an operation declares
   `FILE_NOT_FOUND`, clients depend on that code. Changing it (renaming to
   `NOT_FOUND_FILE`) is a breaking change for clients that match on it.
   This is the same stability property as `input_schema` and
   `output_schema` — the operation's interface is its contract. Adding new
   error codes is additive (clients that don't know the new code treat it
   as `INTERNAL`); removing or renaming codes is breaking.

3. **Protocol-level codes are distinct from operation-level codes.** If an
   operation declares a code that collides with a protocol code (e.g., an
   operation declares `NOT_FOUND` as a domain error), the protocol code
   takes precedence in the dispatch machinery (the registry's `NOT_FOUND`
   for "operation not registered" is emitted before the handler runs). The
   assumption is that operations use domain-specific codes (`FILE_NOT_FOUND`)
   rather than reusing protocol codes (`NOT_FOUND`). This is a naming
   convention, not a type-system enforcement.

4. **`details` is optional and backward-compatible.** Existing clients that
   ignore `details` continue to work. New clients read `details` for
   structured context. The wire format addition is additive.

## References

- ADR-017: Call protocol client and adapter contract (adapter fidelity —
  this ADR makes `from_openapi`/`to_openapi` faithful on the error axis)
- ADR-014: Secret material flow (the `details` field must not carry secret
  material — same constraint as `metadata`)
- ADR-015: Privilege model (the `FORBIDDEN` protocol code covers ACL
  denial; operation-level `PERMISSION_DENIED` is a distinct domain error
  for OS-level permission issues)
- docs/reviews/001-pre-implementation-architecture-sanity-check.md
  (finding C5, which this ADR resolves)
- ADR-049: Streaming handler for subscriptions (amends this ADR's
  protocol-level code list — `INVALID_OPERATION_TYPE` added as the sixth
  protocol-level code)
- docs/sdd_process.md L19, L423 (Safe Exit protocol — the general principle
  of making failure typed and declared)
- TypeScript reference: `/workspace/@alkdev/operations/src/types.ts`
  L38–47 (`ErrorDefinitionSchema`), L94, L112 (`errorSchemas` on
  `OperationSpec`), `error.ts` L25–51 (`mapError`)
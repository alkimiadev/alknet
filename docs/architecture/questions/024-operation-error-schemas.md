# OQ-24: Operation Error Schemas

- **Origin**: [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md), ADR-017
- **Status**: resolved
- **Door type**: One-way (wire format), two-way (mapping mechanism)
- **Priority**: high
- **Resolution**: `OperationSpec` gains `error_schemas: Vec<ErrorDefinition>` where each `ErrorDefinition` carries a `code`, `description`, `schema` (JSON Schema for the error detail payload), and optional `http_status` (for adapter projection). The `call.error` payload gains an optional `details` field carrying the typed error payload.   Protocol-level codes (`NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`,
  `INVALID_OPERATION_TYPE`, `INTERNAL`, `TIMEOUT`) are distinct from
  operation-level domain codes (`FILE_NOT_FOUND`, `RATE_LIMITED`, etc.) —
  protocol codes are emitted by the dispatch machinery, operation codes by
  handlers. The six-code protocol-level list was extended from five by
  ADR-049 (`INVALID_OPERATION_TYPE`). `from_openapi`/`to_openapi` map OpenAPI response status codes to/from `ErrorDefinition`s, making the adapter contract from ADR-017 faithful on the error axis. `services/schema` exposes `error_schemas` for client code generation. See ADR-023.
- **Cross-references**: ADR-017, ADR-023, docs/reviews/001-pre-implementation-architecture-sanity-check.md (C5), [operation-registry.md](crates/call/operation-registry.md), [call-protocol.md](crates/call/call-protocol.md)

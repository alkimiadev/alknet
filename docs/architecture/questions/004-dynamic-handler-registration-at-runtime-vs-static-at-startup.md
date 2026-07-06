# OQ-04: Dynamic Handler Registration at Runtime vs Static at Startup

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Static registration at startup. `HandlerRegistry` is immutable after construction. ALPN strings in the TLS `ServerConfig` are derived from the registry at startup. See ADR-010.

  **Scope clarification (ADR-024)**: This resolution applies to the
  **`HandlerRegistry`** (ALPN string → `ProtocolHandler`), which is what
  ADR-010 governs. The call protocol's **`OperationRegistry`** (operation
  name → `HandlerRegistration`) is a *separate* registry living inside the
  `CallAdapter`, behind the single ALPN `alknet/call`. Its mutability
  profile is governed by ADR-024, not by this OQ. ADR-024 layers the
  operation registry by trust boundary: curated `Local` ops are immutable
  (same rationale as here — composing ops are privileged, the startup trust
  boundary is where their authority is granted); `Session` and imported
  (`FromCall` etc.) ops are dynamic at their respective trust-boundary
  scopes (session, connection). The pre-ADR-024 blanket immutability claim
  in `operation-registry.md` was inherited by analogy from this OQ and did
  not actually apply — the TLS-config argument that justifies
  `HandlerRegistry` immutability does not touch the `OperationRegistry`.
- **Cross-references**: ADR-001, ADR-010, ADR-024, [endpoint.md](crates/core/endpoint.md), [operation-registry.md](crates/call/operation-registry.md)

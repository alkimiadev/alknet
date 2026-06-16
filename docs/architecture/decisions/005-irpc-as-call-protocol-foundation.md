# ADR-005: irpc as Call Protocol Foundation

## Status

Accepted

## Context

The call protocol (alknet-call) provides structured RPC — operations, request/response, streaming subscriptions, and pub/sub. This is the primary interface for programmatic interaction with an alknet node. It needs to work across platforms: Rust clients, TypeScript/JavaScript clients (via NAPI), WASM targets, and any language that can speak the wire format.

The previous implementation used `irpc` for the call protocol's operation registry, framing, and service patterns. irpc provides:
- An operation registry with schema-based discovery
- Length-prefixed JSON framing (EventEnvelope)
- Request/response and streaming patterns
- Type-safe operation definitions via derive macros

The call protocol is derived from a TypeScript implementation of "operations" and "pub/sub" that can wholesale import OpenAPI schemas, wrap MCP servers, and go the other direction — exposing operations as HTTP endpoints, MCP tools, etc. This bidirectional capability is strategically important.

## Decision

alknet-call uses irpc as its foundation. The `CallAdapter` implements `ProtocolHandler` on ALPN `alknet/call` and delegates to irpc's operation registry, framing, and dispatch.

irpc is not replaced or wrapped in an abstraction layer — it IS the call protocol's core. The relationship is:
- irpc provides: operation registry, schema discovery, frame encoding/decoding, request/response routing, streaming
- alknet-call provides: the ProtocolHandler adapter (BiStream → irpc), AuthContext integration, access control checks, the ALPN registration

This means:
- The wire format is irpc's EventEnvelope framing — length-prefixed JSON
- Operation schemas follow irpc's schema model — JSON Schema compatible
- The TypeScript "operations" and "pub/sub" patterns that can import OpenAPI schemas and expose MCP tools are supported at the protocol level
- Future NAPI and WASM clients speak the same wire format

The `VaultProtocol` in alknet-vault also uses irpc as its service protocol. This is consistent — alknet-vault's irpc service is an independent service that happens to use the same framing, not a dependency on alknet-call.

## Consequences

**Positive:**
- Proven operation registry and framing — irpc is already tested in production (iroh uses it)
- JSON Schema compatible — OpenAPI import, MCP tool exposure, cross-language client generation
- No need to design a custom RPC wire format — irpc's is already battle-tested
- The call protocol inherits irpc's streaming and subscription patterns
- Consistency with alknet-vault's service model — both use irpc

**Negative:**
- alknet-call depends on irpc — if irpc has limitations or bugs, we're affected (mitigated: irpc is lightweight and we can fork if needed)
- JSON framing is not the most compact binary format — for high-throughput scenarios, a binary codec could be added later as an irpc extension
- irpc's derive macros add a compilation dependency — but this is standard for Rust RPC frameworks
- The call protocol's cross-language story depends on irpc's wire format being documented and stable (mitigated: it's length-prefixed JSON, which is inherently cross-language)

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-003: Crate decomposition
- ADR-004: Auth as shared core (IdentityProvider)
- irpc reference: `docs/research/references/iroh/irpc/` (see individual docs in that directory)
- The previous architecture had an equivalent decision in ADR-024 (bidirectional call protocol with EventEnvelope framing), which is archived in the reference implementation at `/workspace/@alkdev/alknet-main/`.
# ADR-005: irpc as Call Protocol Foundation

## Status

~~Accepted~~ → **Superseded** by [ADR-064](064-irpc-never-integrated-hand-rolled-framing.md)

> **Superseded 2026-07-09.** This ADR accepted "irpc as the call protocol
> foundation" based on the previous architecture's use of irpc. When the
> call protocol was implemented, it turned out that **no `.rs` file in the
> workspace ever imported irpc** — the `irpc` / `irpc-derive` workspace deps
> were a Cargo.toml entry with no corresponding import. The wire protocol
> (`crates/alknet-call/src/protocol/wire.rs`) is hand-rolled length-prefixed
> JSON; the `EventEnvelope` shape was derived from the `@alkdev/pubsub`
> TypeScript prior art (ADR-013), not from irpc. ADR-064 supersedes this
> ADR and records the actual state: hand-rolled framing, no irpc
> integration. The architectural properties this ADR sought (proven
> length-prefixed JSON framing, cross-language JSON wire format, streaming)
> are preserved by the hand-rolled implementation. The text below is kept
> as the historical record of the decision that was made (and never
> implemented as stated).

## Context

The call protocol (alknet-call) provides structured RPC — operations, request/response, streaming subscriptions, and pub/sub. This is the primary interface for programmatic interaction with an alknet node. It needs to work across platforms: Rust clients, TypeScript/JavaScript clients (via NAPI), WASM targets, and any language that can speak the wire format.

The previous implementation used `irpc` for the call protocol's operation registry, framing, and service patterns. irpc provides:
- An operation registry with schema-based discovery
- Length-prefixed JSON framing (EventEnvelope)
- Request/response and streaming patterns
- Type-safe operation definitions via derive macros

The call protocol is derived from a TypeScript implementation (`@alkdev/operations`, `@alkdev/pubsub`) that informed the design of the operation registry, EventEnvelope framing, and adapter patterns (from_openapi, from_mcp, from_call). This bidirectional composition capability is strategically important. The TypeScript code is a reference that informed the Rust design — it is not a parallel implementation (see ADR-013).

## Decision

alknet-call uses irpc as its foundation. The `CallAdapter` implements `ProtocolHandler` on ALPN `alknet/call` and delegates to irpc's operation registry, framing, and dispatch.

irpc is not replaced or wrapped in an abstraction layer — it IS the call protocol's core. The relationship is:
- irpc provides: operation registry, schema discovery, frame encoding/decoding, request/response routing, streaming
- alknet-call provides: the ProtocolHandler adapter (BiStream → irpc), AuthContext integration, access control checks, the ALPN registration

This means:
- The wire format is irpc's EventEnvelope framing — length-prefixed JSON
- Operation schemas follow irpc's schema model — JSON Schema compatible
- The TypeScript operation and pub/sub patterns that can import OpenAPI schemas, wrap MCP servers, and expose operations as endpoints are supported at the protocol level — the adapter contract (from_*, to_*) is defined in Rust (see ADR-013)
- Future NAPI and WASM clients speak the same wire format — alknet-napi projects the Rust call protocol client to Node.js; a browser SDK can be adapted from the existing TypeScript code

The `VaultProtocol` in alknet-vault previously used irpc as its service
protocol. ADR-025 dropped irpc from the vault — the vault uses direct method
calls on `VaultServiceHandle`, not irpc dispatch. irpc remains the
foundation for alknet-*call* (the call protocol), not for alknet-*vault*.
See ADR-025 for the rationale (security default inversion: the vault is
local-only by construction, not remote-capable by default).

## Consequences

**Positive:**
- Proven operation registry and framing — irpc is already tested in production (iroh uses it)
- JSON Schema compatible — OpenAPI import, MCP tool exposure, cross-language client generation
- No need to design a custom RPC wire format — irpc's is already battle-tested
- The call protocol inherits irpc's streaming and subscription patterns

**Negative:**
- alknet-call depends on irpc — if irpc has limitations or bugs, we're affected (mitigated: irpc is lightweight and we can fork if needed)
- JSON framing is not the most compact binary format — for high-throughput scenarios, a binary codec could be added later as an irpc extension
- irpc's derive macros add a compilation dependency — but this is standard for Rust RPC frameworks
- The call protocol's cross-language story depends on irpc's wire format being documented and stable (mitigated: it's length-prefixed JSON, which is inherently cross-language)

## References

- **Superseding ADR**: [ADR-064](064-irpc-never-integrated-hand-rolled-framing.md) — irpc was never integrated; hand-rolled framing is the actual state
- ADR-013: Rust as canonical implementation (the `@alkdev/pubsub` prior art the `EventEnvelope` shape was actually derived from)
- ADR-025: Vault local-only dispatch (dropped irpc from the vault; ADR-064 confirms irpc was never in alknet-call either)
- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-003: Crate decomposition
- ADR-004: Auth as shared core (IdentityProvider)
- Call protocol wire format (actual): `crates/alknet-call/src/protocol/wire.rs`
- The previous architecture had an equivalent decision in ADR-024 (bidirectional call protocol with EventEnvelope framing), which is archived in the reference implementation at `/workspace/@alkdev/alknet-main/`.
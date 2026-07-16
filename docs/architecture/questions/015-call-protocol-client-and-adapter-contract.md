# OQ-15: Call Protocol Client and Adapter Contract

- **Origin**: [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md), ADR-013
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: `CallClient` takes over transport connections (`spawn_dispatch` transport-agnostic primary; `connect` removed per ADR-089 §5 — dial extracted to `AlknetClient`) and shares the dispatch loop with `CallAdapter` — both sides can send and receive `call.requested` once connected. Connection direction (who opened the connection) is independent of call direction (who calls whom). `from_call` adapter discovers remote operations via `services/list` + `services/schema` and registers them with forwarding handlers — same pattern as `from_openapi` and `from_mcp`. `to_openapi` and `to_mcp` project local operations to external protocols. Adapter contract trait (`OperationAdapter`) produces `HandlerRegistration` bundles. Cross-node call tree: abort cascade (ADR-016) propagates across node boundaries through `from_call` handlers. Credentials for connections come from capabilities (ADR-014). Adapter-registered operations are `Internal` by default (ADR-015). See ADR-017.
- **Cross-references**: ADR-005, ADR-013, ADR-014, ADR-015, ADR-016, ADR-017, [call-protocol.md](crates/call/call-protocol.md), [operation-registry.md](crates/call/operation-registry.md)

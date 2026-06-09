---
id: stream-interface-message-interface-split
name: Rename Interface → StreamInterface, add MessageInterface trait and restructure config types
status: pending
depends_on: []
scope: moderate
risk: medium
impact: phase
level: implementation
---

## Description

Rename the current `Interface` trait to `StreamInterface` and add a `MessageInterface` trait for request/response interfaces (HTTP, DNS). This is the most impactful structural change in Phase 2 because all subsequent tasks reference the new trait names and config types.

Per ADR-035 and research/phase2/interface-model.md:

- `Interface` → `StreamInterface` (the current trait becomes the stream-specific variant; persistent byte streams)
- `InterfaceSession` stays as-is (it's already stream-specific)
- Add `MessageInterface` trait: `handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>` (stateless request/response)
- Add `InterfaceRequest` and `InterfaceResponse` types that normalize calls across message interfaces
- Add `HttpInterface` stub (struct definition, no route implementations yet)
- Add `DnsInterface` stub (struct definition only)
- Restructure `InterfaceConfig` into `StreamInterfaceConfig` and `MessageInterfaceConfig`
- Update `ListenerConfig` to include `Stream`, `Http`, and `Dns` variants per the research docs
- Add `TransportKind::WebTransport` as a tag-only variant (no acceptor implementation)
- Remove any references to `TransportKind::Dns` (it was never added, so no removal needed — just ensure it's not added)

**Why this must go first**: Every other Phase 2 task imports and references these types. The rename and new traits must land before SshSession bridge, RawFraming implementation, or HTTP scaffold work begins. The integration plan section 2.3 explicitly states: "This task should be done early in Phase 2 because all subsequent tasks reference the new trait names."

**Current state of the code**:
- `Interface` trait in `crates/alknet-core/src/interface/mod.rs` with `accept()` → `Self::Session`
- `InterfaceSession` trait in `crates/alknet-core/src/interface/session.rs` with `recv()` / `send()`
- `InterfaceConfig` enum with `Ssh(SshInterfaceConfig)` and `RawFraming(RawFramingConfig)` variants
- `InterfaceKind` enum with `Ssh` and `RawFraming` variants
- `TransportKind` has `Tcp`, `Tls`, `Iroh` (no `Dns`)
- `SshInterface` implements `Interface`, `SshSession` implements `InterfaceSession`
- `RawFramingInterface`/`RawFramingSession` are stubs (Phase 1 left them)

## Acceptance Criteria

- [ ] `Interface` trait renamed to `StreamInterface` throughout alknet-core (mod.rs, ssh.rs, raw_framing.rs, and all import sites)
- [ ] `MessageInterface` trait defined in `crates/alknet-core/src/interface/mod.rs` with `handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>`
- [ ] `InterfaceRequest` struct defined with `operation_path`, `input`, `auth_token`, `metadata` fields per interface-model.md
- [ ] `InterfaceResponse` struct defined with `result`, `status`, `headers` fields per interface-model.md
- [ ] `HttpInterface` stub struct defined (identity_provider, registry, env fields) — no route implementations
- [ ] `DnsInterface` stub struct defined (domain, identity_provider, registry, env fields) — no implementation
- [ ] `InterfaceConfig` restructured: `StreamInterfaceConfig::Ssh` and `StreamInterfaceConfig::RawFraming` replace current variants; `MessageInterfaceConfig` enum added with `Http` and `Dns` variants (or the variant types are defined)
- [ ] `ListenerConfig` enum updated with `Stream { transport, interface }`, `Http { bind_addr, tls, stealth }`, and `Dns { bind_addr, tls }` variants
- [ ] `TransportKind::WebTransport` added as tag-only variant (no acceptor, no feature flag beyond the enum variant)
- [ ] `is_valid_pair()` / `TransportKindBase` validation updated for StreamInterface pairs only
- [ ] All existing tests pass after the rename (SshInterface and RawFramingInterface still compile and pass)
- [ ] New `StreamInterface` implementors still use `InterfaceSession` for `type Session`
- [ ] `MessageInterface` has at least one compilation test (e.g., a mock struct implements it)
- [ ] `HttpInterface` and `DnsInterface` stubs compile and exist in the type system
- [ ] `lib.rs` re-exports all new types (`StreamInterface`, `MessageInterface`, `InterfaceRequest`, `InterfaceResponse`, `HttpInterface`, `DnsInterface`)

## References

- docs/architecture/decisions/035-streaminterface-messageinterface-split.md — ADR-035
- docs/research/phase2/interface-model.md — Full design rationale
- docs/research/integration-plan.md — Phase 2.3
- crates/alknet-core/src/interface/mod.rs — Current Interface trait
- crates/alknet-core/src/interface/session.rs — InterfaceSession, InterfaceEvent
- crates/alknet-core/src/interface/config.rs — Current InterfaceConfig
- crates/alknet-core/src/interface/pairs.rs — Valid transport-interface pairs

## Notes

> This is the most mechanically invasive change in Phase 2 due to the rename, but it's low-risk behaviorally. The `Interface` → `StreamInterface` rename is a find-and-replace operation. The new `MessageInterface` trait and stubs are purely additive. The `ListenerConfig` restructuring is additive since no existing code uses `ListenerConfig::Http` or `ListenerConfig::Dns` yet.

> The integration plan section 2.3 notes: "Existing `SshInterface` and `RawFramingInterface` become `StreamInterface` implementations. No behavior change for stream-based interfaces."

> Consider using `#[non_exhaustive]` on the new enums (`MessageInterfaceConfig`, `ListenerConfig`) so future variants (WebSocket, etc.) don't break downstream.

## Summary

> To be filled on completion
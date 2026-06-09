---
id: stream-interface-message-interface-split
name: Rename Interface → StreamInterface, add MessageInterface trait, remove TransportKind::Dns, restructure ListenerConfig
status: completed
depends_on: []
scope: moderate
risk: medium
impact: phase
level: implementation
---

## Description

Rename the current `Interface` trait to `StreamInterface`, add a `MessageInterface` trait for request/response interfaces (HTTP, DNS), remove `TransportKind::Dns` from the transport enum (DNS is a `MessageInterface`, not a transport), restructure `ListenerConfig` from a flat struct to the ADR-035 enum form, and align `TransportKind::WebTransport` with the spec. This is the most impactful structural change in Phase 2 because all subsequent tasks reference the new trait names and config types.

Per ADR-035 and research/phase2/interface-model.md:

- `Interface` → `StreamInterface` (the current trait becomes the stream-specific variant; persistent byte streams)
- `InterfaceSession` stays as-is (it's already stream-specific)
- Add `MessageInterface` trait: `handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>` (stateless request/response)
- Add `InterfaceRequest` and `InterfaceResponse` types that normalize calls across message interfaces
- Add `HttpInterface` stub (struct definition, no route implementations yet)
- Add `DnsInterface` stub (struct definition only)
- Restructure `InterfaceConfig` into `StreamInterfaceConfig` and `MessageInterfaceConfig`
- Restructure `ListenerConfig` from its current flat struct form to the ADR-035 enum with `Stream`, `Http`, and `Dns` variants
- **Remove `TransportKind::Dns`** — DNS is a `MessageInterface`, not a transport. This is a residual from Phase 1 that needs cleanup. All code referencing it must be removed: the `ListenerConfig::dns()` constructor, `pairs.rs` valid pairs table, `TransportKindBase::Dns`, and all related tests.
- **Update `TransportKind::WebTransport`** field from `{ host: String }` to `{ server_name: Option<String> }` per ADR-035 (tag-only variant, `server_name` is optional)

**Why this must go first**: Every other Phase 2 task imports and references these types. The rename and new traits must land before SshSession bridge, RawFraming implementation, or HTTP scaffold work begins. The integration plan section 2.3 explicitly states: "This task should be done early in Phase 2 because all subsequent tasks reference the new trait names."

**Current state of the code** (IMPORTANT — these differ from what the research docs assumed):

- `Interface` trait in `crates/alknet-core/src/interface/mod.rs` with `accept()` → `Self::Session`
- `InterfaceSession` trait in `crates/alknet-core/src/interface/session.rs` with `recv()` / `send()`
- `InterfaceConfig` enum with `Ssh(SshInterfaceConfig)` and `RawFraming(RawFramingConfig)` variants
- `InterfaceKind` enum with `Ssh` and `RawFraming` variants
- `TransportKind` has `Tcp`, `Tls { server_name: Option<String> }`, `Iroh { endpoint_id: String }`, `Dns { domain: String }`, and `WebTransport { host: String }`
- **`TransportKind::Dns` exists in the current code** — it must be removed. References exist in `pairs.rs` (`TransportKindBase::Dns`), `serve.rs` (`ListenerConfig::dns()` constructor, Display impl, validate logic), `transport/mod.rs` (Display impl, tests), and `lib.rs` re-exports.
- **`TransportKind::WebTransport` exists but has `{ host: String }`** — must be changed to `{ server_name: Option<String> }` per ADR-035.
- `ListenerConfig` is currently a **flat struct** (fields: `transport_kind`, `interface_kind`, `listen_addr`, `tls_cert`, `tls_key`, `acme_domain`, `stealth`, `iroh_relay`) with builder-pattern constructors (`tcp()`, `tls()`, `iroh()`, `dns()`, `webtransport()`) — NOT an enum. Must be restructured to the ADR-035 enum form.
- `SshInterface` implements `Interface`, `SshSession` implements `InterfaceSession`
- `RawFramingInterface`/`RawFramingSession` are stubs (Phase 1 left them)

## Acceptance Criteria

- [ ] `Interface` trait renamed to `StreamInterface` throughout alknet-core (mod.rs, ssh.rs, raw_framing.rs, and all import sites including `lib.rs`)
- [ ] `MessageInterface` trait defined in `crates/alknet-core/src/interface/mod.rs` with `handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>`
- [ ] `InterfaceRequest` struct defined with `operation_path`, `input`, `auth_token`, `metadata` fields per interface-model.md
- [ ] `InterfaceResponse` struct defined with `result`, `status`, `headers` fields per interface-model.md
- [ ] `HttpInterface` stub struct defined (identity_provider, registry, env fields) — no route implementations
- [ ] `DnsInterface` stub struct defined (domain, identity_provider, registry, env fields) — no implementation
- [ ] `InterfaceConfig` restructured: `StreamInterfaceConfig::Ssh` and `StreamInterfaceConfig::RawFraming` replace current variants; `MessageInterfaceConfig` enum added with `Http` and `Dns` variants
- [ ] `ListenerConfig` restructured from flat struct to enum with `Stream { transport: TransportKind, interface: StreamInterfaceKind }`, `Http { config: HttpListenerConfig }`, and `Dns { config: DnsListenerConfig }` variants per ADR-035
- [ ] `StreamInterfaceKind` enum defined (corresponding to `StreamInterface` implementors: `Ssh`, `RawFraming`)
- [ ] `MessageInterfaceKind` enum defined (corresponding to `MessageInterface` implementors: `Http`, `Dns`)
- [ ] `TransportKind::Dns` **removed** from the enum and all references cleaned up: `TransportKindBase::Dns` in pairs.rs, `ListenerConfig::dns()` constructor, Display impls, validate logic, and all related tests
- [ ] `TransportKind::WebTransport` field changed from `{ host: String }` to `{ server_name: Option<String> }` and all references updated
- [ ] `is_valid_pair()` / `TransportKindBase` validation updated for StreamInterface pairs only (no DNS pairs)
- [ ] All existing tests pass after the changes (SshInterface and RawFramingInterface still compile and pass)
- [ ] New `StreamInterface` implementors still use `InterfaceSession` for `type Session`
- [ ] `MessageInterface` has at least one compilation test (e.g., a mock struct implements it)
- [ ] `HttpInterface` and `DnsInterface` stubs compile and exist in the type system
- [ ] `lib.rs` re-exports all new types (`StreamInterface`, `MessageInterface`, `InterfaceRequest`, `InterfaceResponse`, `HttpInterface`, `DnsInterface`, `StreamInterfaceConfig`, `MessageInterfaceConfig`, `StreamInterfaceKind`, `MessageInterfaceKind`, `HttpListenerConfig`, `DnsListenerConfig`)
- [ ] `HttpListenerConfig` struct defined with `bind_addr: SocketAddr`, `tls: bool`, `stealth: bool`
- [ ] `DnsListenerConfig` struct defined with `bind_addr: SocketAddr`, `tls: bool`
- [ ] `Display` implementations added for all new enums/structs
- [ ] `#[non_exhaustive]` on `ListenerConfig`, `StreamInterfaceKind`, `MessageInterfaceKind`, `MessageInterfaceConfig`
- [ ] Serde `Serialize`/`Deserialize` on all new config types
- [ ] Updated tests cover the new `ListenerConfig` enum form, the removed `Dns` transport, and the `WebTransport` field rename

## References

- docs/architecture/decisions/035-streaminterface-messageinterface-split.md — ADR-035
- docs/research/phase2/interface-model.md — Full design rationale
- docs/research/integration-plan.md — Phase 2.3
- crates/alknet-core/src/interface/mod.rs — Current Interface trait
- crates/alknet-core/src/interface/session.rs — InterfaceSession, InterfaceEvent
- crates/alknet-core/src/interface/config.rs — Current InterfaceConfig
- crates/alknet-core/src/interface/pairs.rs — Valid transport-interface pairs (has TransportKindBase::Dns to remove)
- crates/alknet-core/src/transport/mod.rs — TransportKind enum (has Dns and WebTransport { host } to fix)
- crates/alknet-core/src/server/serve.rs — ListenerConfig flat struct (to restructure to enum)
- crates/alknet-core/src/lib.rs — Re-exports

## Notes

> This is the most mechanically invasive change in Phase 2 due to the rename and the `ListenerConfig` restructuring, but it's low-risk behaviorally. The `Interface` → `StreamInterface` rename is a find-and-replace operation. The new `MessageInterface` trait and stubs are purely additive. The `ListenerConfig` restructuring is the biggest change — it's currently a flat struct with builder-pattern constructors, and needs to become an enum with `Stream`, `Http`, and `Dns` variants per ADR-035. The `Server::new()` code and `StaticConfig::from_serve_options()` code that currently uses the struct form will need to be updated.

> The `TransportKind::Dns` removal is cleanup from Phase 1 — it was incorrectly added as a transport variant when DNS is actually a `MessageInterface`. All references must be removed: the `TransportKindBase::Dns` in `pairs.rs`, the `ListenerConfig::dns()` constructor in `serve.rs`, the `VALID_TRANSPORT_INTERFACE_PAIRS` table entry, and the Display/test code. The DNS functionality will be represented by `ListenerConfig::Dns { config: DnsListenerConfig }` instead.

> The `TransportKind::WebTransport` field change from `{ host: String }` to `{ server_name: Option<String> }` aligns with ADR-035. Since no code currently depends on the `host` field specifically (there's no WebTransport acceptor), this is a safe rename.

> The integration plan section 2.3 notes: "Existing `SshInterface` and `RawFramingInterface` become `StreamInterface` implementations. No behavior change for stream-based interfaces."

> Consider using `#[non_exhaustive]` on the new enums (`MessageInterfaceConfig`, `ListenerConfig`, `StreamInterfaceKind`, `MessageInterfaceKind`) so future variants (WebSocket, etc.) don't break downstream.

> When restructuring `ListenerConfig` from struct to enum, the `Server::new()`, `StaticConfig::from_serve_options()`, and `serve.rs` accept loop code will need updates. The `ServeOptions.listeners` field type changes. The `ListenerConfig::tcp()`, `tls()`, `iroh()` constructors should become associated functions that produce the `ListenerConfig::Stream` variant. New constructors for `Http` and `Dns` variants will be added.

## Summary

> Renamed Interface → StreamInterface throughout alknet-core. Added MessageInterface trait with InterfaceRequest/InterfaceResponse types. Added HttpInterface and DnsInterface stubs. Restructured InterfaceConfig into StreamInterfaceConfig + MessageInterfaceConfig. Added StreamInterfaceKind and MessageInterfaceKind enums. Restructured ListenerConfig from flat struct to enum with Stream/Http/Dns variants per ADR-035. Removed TransportKind::Dns (DNS is a MessageInterface). Changed TransportKind::WebTransport from { host: String } to { server_name: Option<String> }. Updated all import sites, pairs.rs, serve.rs, server/handler.rs, napi/serve.rs, lib.rs, and all tests. 415 tests pass, clippy clean, fmt clean.
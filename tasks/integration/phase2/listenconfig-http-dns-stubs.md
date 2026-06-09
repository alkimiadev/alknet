---
id: listenconfig-http-dns-stubs
name: Update ListenerConfig with Http/Dns variants, add TransportKind::WebTransport tag, restructure InterfaceConfig
status: pending
depends_on: [stream-interface-message-interface-split]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Add `ListenerConfig::Http` and `ListenerConfig::Dns` variants for message-based interfaces, add `TransportKind::WebTransport` as a tag-only variant, and restructure `InterfaceConfig` into `StreamInterfaceConfig` and `MessageInterfaceConfig` to align with the `StreamInterface`/`MessageInterface` split.

Per the integration plan section 2.5 and research/phase2/interface-model.md and tls-transport.md:

**Current state**:
- `ListenerConfig` likely has a single variant or is not yet fully defined (Phase 1 added `TransportKind` variants and `InterfaceConfig` but the `ListenerConfig` may need updating)
- `TransportKind` has `Tcp`, `Tls`, `Iroh` — no `Dns` (correctly), no `WebTransport`
- `InterfaceConfig` has `Ssh(SshInterfaceConfig)` and `RawFraming(RawFramingConfig)` — needs restructuring to `StreamInterfaceConfig`

**Key changes**:
- Add `TransportKind::WebTransport` variant — tag-only, no acceptor implementation. This is a trivial addition that prevents a breaking change later when WebTransport lands in Phase 5.
- Confirm `TransportKind::Dns` is NOT in the enum (DNS is a `MessageInterface`, not a transport). If it somehow got added, remove it. (Research confirms it was never added.)
- Rename `InterfaceConfig` → `StreamInterfaceConfig` (aligned with the trait rename from task 1)
- Add `StreamInterfaceConfig::Ssh` and `StreamInterfaceConfig::RawFraming` variants
- Add `MessageInterfaceConfig` enum with `Http` and `Dns` variants (and their config structs)
- Add `HttpListenerConfig` struct: `bind_addr`, `tls: bool`, `stealth: bool`
- Add `DnsListenerConfig` struct: `bind_addr`, `tls: bool`
- Update `ListenerConfig` to have three variants:
  - `Stream { transport: TransportKind, interface: StreamInterfaceKind }` (existing pattern, renamed)
  - `Http { config: HttpListenerConfig }`
  - `Dns { config: DnsListenerConfig }`
- `TransportKind::WebTransport` is a tag-only enum variant — no `WebTransportAcceptor` implementation, no feature flag, just the variant existing so that config parsing can reference it

**Note on stealth mode**: The `Http` variant's `stealth` field means "if true, do byte-peek protocol detection on incoming TLS connections". This connects to the existing `stealth.rs` protocol detection. The axum router scaffold (task 2.7) handles the routing when stealth mode detects HTTP traffic. This task just defines the config types.

## Acceptance Criteria

- [ ] `TransportKind::WebTransport { server_name: Option<String> }` variant added as tag-only (no acceptor impl, compiles but has no effect on server behavior)
- [ ] `TransportKind::Dns` confirmed absent from the enum (DNS is a `MessageInterface`, not a transport)
- [ ] `InterfaceConfig` renamed to `StreamInterfaceConfig` (or `StreamConfig` — aligned with the trait rename) with `Ssh` and `RawFraming` variants
- [ ] `MessageInterfaceConfig` enum added with `Http` and `Dns` variants
- [ ] `HttpListenerConfig` struct defined with `bind_addr: SocketAddr`, `tls: bool`, `stealth: bool`
- [ ] `DnsListenerConfig` struct defined with `bind_addr: SocketAddr`, `tls: bool`
- [ ] `ListenerConfig` enum has three variants: `Stream { transport, interface }`, `Http { config: HttpListenerConfig }`, `Dns { config: DnsListenerConfig }`
- [ ] `StreamInterfaceKind` enum defined (corresponding to `StreamInterface` implementors: `Ssh`, `RawFraming`)
- [ ] `MessageInterfaceKind` enum defined (corresponding to `MessageInterface` implementors: `Http`, `Dns`)
- [ ] `is_valid_pair()` validation updated for `Stream` listener configs (only valid Transport/StreamInterface combos allowed)
- [ ] `Display` implementations added for all new enums
- [ ] Serialization support (`serde::Serialize`/`Deserialize`) for all new config types
- [ ] All existing server/transport tests pass unchanged
- [ ] Unit test: `TransportKind::WebTransport` variant exists and can be constructed
- [ ] Unit test: `ListenerConfig::Http` variant constructs with `HttpListenerConfig`
- [ ] Unit test: `ListenerConfig::Dns` variant constructs with `DnsListenerConfig`

## References

- docs/research/integration-plan.md — Phase 2.5
- docs/research/phase2/interface-model.md — ListenerConfig, TransportKind, InterfaceKind redesign
- docs/research/phase2/tls-transport.md — HTTP listener config, stealth mode
- crates/alknet-core/src/interface/config.rs — Current InterfaceConfig, InterfaceKind
- crates/alknet-core/src/interface/pairs.rs — Valid transport-interface pairs

## Notes

> Use `#[non_exhaustive]` on `ListenerConfig`, `StreamInterfaceKind`, `MessageInterfaceKind`, and `MessageInterfaceConfig` so future variants (WebSocket, gRPC) don't break downstream.

> The `stealth` field on `HttpListenerConfig` controls whether the server does byte-peek protocol detection (first bytes → SSH vs HTTP). When `stealth: true` on a listener sharing port 443 with SSH, the accept loop routes based on protocol detection. When `stealth: false`, the HTTP listener receives all traffic directly.

> The `tls: bool` field is separate from `stealth`. `tls: true` means "use TLS on this listener". `stealth: true` means "peek first bytes to detect SSH vs HTTP". These are orthogonal: you can have TLS + stealth (port 443), TLS without stealth (port 8443), plain HTTP without stealth (port 8080), etc.

## Summary

> To be filled on completion
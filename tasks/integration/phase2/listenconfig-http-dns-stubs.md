---
id: listenconfig-http-dns-stubs
name: Add HttpListenerConfig/DnsListenerConfig wiring, StreamInterfaceKind/MessageInterfaceKind, and ListenerConfig enum helper methods
status: completed
depends_on: [stream-interface-message-interface-split]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

After the `stream-interface-message-interface-split` task restructures `ListenerConfig` from a flat struct to the ADR-035 enum form (with `Stream`, `Http`, `Dns` variants), removes `TransportKind::Dns`, and adds the `StreamInterfaceConfig`/`MessageInterfaceConfig`/`StreamInterfaceKind`/`MessageInterfaceKind`/`HttpListenerConfig`/`DnsListenerConfig` types, this task wires those new types into the server's accept loop and adds the helper methods, validation, and constructors that make the new `ListenerConfig` enum usable.

Per the integration plan section 2.5 and research/phase2/interface-model.md and tls-transport.md:

**What task 1 (stream-interface-message-interface-split) already did**:
- `ListenerConfig` restructured to enum with `Stream { transport, interface }`, `Http { config: HttpListenerConfig }`, `Dns { config: DnsListenerConfig }` variants
- `TransportKind::Dns` removed
- `TransportKind::WebTransport` updated to `{ server_name: Option<String> }`
- `StreamInterfaceConfig`/`MessageInterfaceConfig` enums defined
- `StreamInterfaceKind`/`MessageInterfaceKind` enums defined
- `HttpListenerConfig` and `DnsListenerConfig` struct types defined
- `is_valid_pair()` updated for StreamInterface pairs only

**What this task adds** (the wiring layer on top):

- **ListenerConfig constructors and helpers**: The enum exists but needs `tcp()`, `tls()`, `iroh()` convenience constructors that produce `ListenerConfig::Stream`, and `http()` / `dns()` constructors that produce their respective variants. These replace the old struct-style builders that task 1 removed.
- **ListenerConfig validation**: `validate()` method on the enum that checks: TLS cert/key requirements for Stream+Tls listeners, stealth-only-on-TLS for Http listeners, no TLS options on non-TLS variants.
- **Server accept loop wiring**: Update `Server::run()` and `handle_connection()` to match on the `ListenerConfig` enum variants. The `Stream` variant runs through the existing SSH/raw-framing accept path. The `Http` and `Dns` variants are stubs for now (Http defers to task 2.7 for the axum router, Dns defers to Phase 5).
- **Display implementations**: Ensure all new types have proper Display impls (task 1 likely added basic ones; add any missing).
- **ServeOptions integration**: Update `ServeOptions` to work with the new `ListenerConfig` enum — the `listeners` field should accept the enum form, and `StaticConfig::from_serve_options()` should produce `ListenerConfig` enum values.

**Note on stealth mode**: The `HttpListenerConfig.stealth` field means "if true, do byte-peek protocol detection on incoming TLS connections". This connects to the existing `stealth.rs` protocol detection. The axum router scaffold (task 2.7) handles the routing when stealth mode detects HTTP traffic. This task just wires the config types into the server.

## Acceptance Criteria

- [ ] `ListenerConfig` enum has convenience constructors: `tcp(addr)` → `ListenerConfig::Stream`, `tls(addr)` → `ListenerConfig::Stream`, `iroh(addr)` → `ListenerConfig::Stream`, `http(config)` → `ListenerConfig::Http`, `dns(config)` → `ListenerConfig::Dns`
- [ ] `HttpListenerConfig` has a builder-pattern API: `HttpListenerConfig::new(addr).tls(true).stealth(true)`
- [ ] `DnsListenerConfig` has a builder-pattern API: `DnsListenerConfig::new(addr).tls(true)`
- [ ] `ListenerConfig::validate()` works for all three variants: Stream checks TLS cert/key, Http checks stealth-only-with-TLS, Dns has minimal validation
- [ ] `Server::run()` updated to match on `ListenerConfig` variants: Stream variant uses existing accept path, Http/Dns variants are stubs that log "not yet implemented" for now
- [ ] `StaticConfig::from_serve_options()` produces `ListenerConfig` enum values correctly
- [ ] `ServeOptions.listeners` field works with the new enum form
- [ ] `is_valid_pair()` called during `ListenerConfig::Stream` validation
- [ ] Serialization support (`serde::Serialize`/`Deserialize`) for all config types verified working
- [ ] All existing server/transport tests pass (updated to use new enum constructors)
- [ ] Unit test: `ListenerConfig::Http` variant constructs with `HttpListenerConfig`
- [ ] Unit test: `ListenerConfig::Dns` variant constructs with `DnsListenerConfig`
- [ ] Unit test: `ListenerConfig::Stream` validates TLS cert/key requirements
- [ ] Unit test: stealth on non-TLS Http listener is rejected by validation

## References

- docs/research/integration-plan.md — Phase 2.5
- docs/research/phase2/interface-model.md — ListenerConfig, TransportKind, InterfaceKind redesign
- docs/research/phase2/tls-transport.md — HTTP listener config, stealth mode
- docs/architecture/decisions/035-streaminterface-messageinterface-split.md — ADR-035 (ListenerConfig enum form)
- crates/alknet-core/src/interface/config.rs — InterfaceConfig (now StreamInterfaceConfig/MessageInterfaceConfig)
- crates/alknet-core/src/interface/pairs.rs — Valid transport-interface pairs
- crates/alknet-core/src/server/serve.rs — ListenerConfig, Server, ServeOptions, StaticConfig

## Notes

> This task depends heavily on what task 1 produces. Before starting, do `git fetch origin && git merge origin/main --no-edit` to get task 1's changes, then read the current state of `ListenerConfig`, `Server`, `ServeOptions`, and `StaticConfig` to understand the enum form they now take.

> The `Http` and `Dns` accept loop stubs should be minimal — just log a message and skip the connection. The full implementations come in task 2.7 (axum scaffold) and Phase 5 (DNS).

> The `stealth` field on `HttpListenerConfig` controls whether the server does byte-peek protocol detection (first bytes → SSH vs HTTP). When `stealth: true` on a listener sharing port 443 with SSH, the accept loop routes based on protocol detection. When `stealth: false`, the HTTP listener receives all traffic directly.

> The `tls: bool` field is separate from `stealth`. `tls: true` means "use TLS on this listener". `stealth: true` means "peek first bytes to detect SSH vs HTTP". These are orthogonal: you can have TLS + stealth (port 443), TLS without stealth (port 8443), plain HTTP without stealth (port 8080), etc.

> Use `#[non_exhaustive]` on `ListenerConfig`, `StreamInterfaceKind`, `MessageInterfaceKind`, and `MessageInterfaceConfig` so future variants (WebSocket, gRPC) don't break downstream.

## Summary

> Added HttpListenerConfig::new().tls().stealth() and DnsListenerConfig::new().tls() builder APIs. Integrated is_valid_pair() into StreamListenerConfig::validate(). Added ListenerConfig::validate() for Http (stealth-requires-TLS) and Dns variants. Refactored Server::run() to dispatch on ListenerConfig variants (Stream→existing accept loop, Http/Dns→warn stubs). 12 new unit tests.
# ADR-035: StreamInterface and MessageInterface Split

## Status
Accepted

## Context

The `Interface` trait (ADR-026) assumes a persistent byte stream from a `Transport`. It produces a `Session` that yields `InterfaceEvent` frames. This works for SSH and raw framing — both run over duplex streams.

However, HTTP and DNS do not fit this model. They handle individual request/response pairs, not persistent sessions. HTTP runs over a TLS connection after byte-peek protocol detection (extending the existing stealth mode pattern). DNS runs its own server on port 53. Both are stateless per-request, not session-oriented.

The three-layer model (Transport, Interface, Protocol) remains correct. The issue is that Layer 2 has two distinct patterns: stream-based (SSH, raw framing) where the transport provides a continuous byte stream, and message-based (HTTP, DNS) where the interface manages its own transport and handles discrete requests.

## Decision

Split the `Interface` trait into two independent traits:

1. **`StreamInterface`** — consumes a `TransportStream`, produces a long-lived `Session` that yields `InterfaceEvent` frames. Existing `SshInterface` and `RawFramingInterface` become `StreamInterface` implementations.

2. **`MessageInterface`** — handles individual `InterfaceRequest` → `InterfaceResponse` pairs. Manages its own transport (HTTP server, DNS server). `HttpInterface` and `DnsInterface` are `MessageInterface` implementations.

The traits are independent. They have different signatures (`accept(stream)` vs `handle_request(req)`), different lifecycles (long-lived session vs stateless per-request), and different transport ownership (provided by caller vs self-managed).

`ListenerConfig` gains variants for both:

```rust
pub enum ListenerConfig {
    Stream {
        transport: TransportKind,
        interface: StreamInterfaceKind,
    },
    Http {
        bind_addr: SocketAddr,
        tls: bool,
        stealth: bool,
    },
    Dns {
        bind_addr: SocketAddr,
        tls: bool,
    },
}
```

`TransportKind::Dns` is removed. DNS is a `MessageInterface` that manages its own transport (UDP/TCP port 53), not a transport variant.

The call protocol handler (Layer 3) is interface-agnostic: it processes `InterfaceEvent` frames from `StreamInterface` sessions and `InterfaceRequest` → `InterfaceResponse` from `MessageInterface` handlers. The dispatch logic is the same — only the framing differs.

## Consequences

**Positive**: HTTP and DNS are first-class interfaces with proper type signatures. No forcing stateless protocols into a session model. The existing stealth mode byte-peek pattern naturally extends to `HttpInterface`. The `InterfaceRequest` / `InterfaceResponse` types normalize calls across message-based interfaces.

**Positive**: Removing `TransportKind::Dns` prevents a breaking change later — code should never depend on DNS as a transport variant.

**Positive**: `ListenerConfig` correctly models the server's accept loop: stream listeners spawn one accept loop per (transport, interface) pair, while HTTP and DNS listeners each manage their own server.

**Negative**: Two traits where there was one. But they serve fundamentally different purposes. A common super-trait would add complexity (`accept_stream` + `handle_request` + `transport_kind`) without practical benefit — implementations satisfy one trait or the other, never both.

**Negative**: The `accept()` method on the current `Interface` trait needs to be renamed. This is a rename of an existing method signature, not a semantic change — `SshInterface` and `RawFramingInterface` implementations become `StreamInterface` implementations with the same `accept()` logic.

## References

- ADR-026 (transport/interface separation — updated by this ADR)
- [interface.md](../interface.md) — Interface layer spec
- [research/phase2/interface-model.md](../../research/phase2/interface-model.md) — Full analysis
- [research/phase2/tls-transport.md](../../research/phase2/tls-transport.md) — HTTP interface, ListenerConfig
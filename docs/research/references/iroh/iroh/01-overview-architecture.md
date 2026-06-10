# Iroh: Overview & Architecture

**Version**: 0.98.1  
**Repository**: https://github.com/n0-computer/iroh  
**License**: MIT OR Apache-2.0  
**Rust Edition**: 2024  
**MSRV**: 1.89

## What is Iroh?

Iroh is a Rust library for establishing **peer-to-peer QUIC connections dialed by public key**. You provide an `EndpointAddr` (which identifies a peer), and iroh finds and maintains the fastest connection route — whether direct (hole-punched) or relayed through a server.

Core value propositions:
- **Dial by public key** — no IP addresses or hostnames needed at the application layer
- **Hole-punching** — automatically attempts direct P2P connectivity
- **Relay fallback** — encrypted relay servers ensure connectivity even behind NATs
- **Built on QUIC** — uses the `noq` QUIC implementation for multiplexed, encrypted streams
- **Address Lookup** — pluggable discovery system to resolve `EndpointId → addressing info`

## Workspace Structure

```
iroh/                # Core library (p2p QUIC connections)
├── iroh-base/       # Fundamental types: SecretKey, PublicKey, EndpointId, RelayUrl, EndpointAddr
├── iroh-dns/        # DNS resolver + endpoint info serialization (pkarr)
├── iroh-dns-server/ # DNS server implementation (powers dns.iroh.link)
├── iroh-relay/      # Relay server + client implementation
└── iroh/bench/      # Benchmarks
```

### Dependency Graph

```
iroh depends on:
  ├── iroh-base        (key types, EndpointAddr, RelayUrl)
  ├── iroh-dns         (DNS resolution, EndpointInfo serialization)
  ├── iroh-relay       (RelayMap, RelayConfig, relay client/server, QUIC client)
  ├── noq              (QUIC implementation)
  ├── noq-proto        (QUIC protocol types)
  ├── noq-udp          (UDP socket abstraction)
  ├── netwatch         (network interface monitoring)
  ├── portmapper       (UPnP/PCP/NAT-PMP port mapping, optional)
  ├── n0-future        (async utilities)
  ├── n0-watcher       (watch/subscribe primitives)
  └── iroh-metrics     (metrics collection)
```

## Key Concepts

### EndpointId / PublicKey
Every iroh endpoint has a unique Ed25519 cryptographic key pair. The public key doubles as the endpoint identifier (`EndpointId`). It's used for both:
- **Identity** — unique addressing in the network
- **Encryption** — TLS authentication (via RFC 7250 Raw Public Keys, no X.509 certificates)

### EndpointAddr
The addressing structure that combines identity with network paths:
```rust
pub struct EndpointAddr {
    pub id: EndpointId,                    // Who to connect to
    pub addrs: BTreeSet<TransportAddr>,    // How to reach them
}

pub enum TransportAddr {
    Relay(RelayUrl),     // Via relay server
    Ip(SocketAddr),      // Direct IP address
    Custom(CustomAddr), // Via custom transport
}
```

### Relay Servers
Relay servers provide:
1. **Reliable connectivity** — always reachable, forward encrypted traffic to the correct endpoint by `EndpointId`
2. **Hole-punching assistance** — QUIC Address Discovery (QAD), STUN-like services
3. **Traffic relay** — fallback when direct connections are impossible

Connections to relays use HTTP/1.1 with TLS, then upgrade to a custom protocol. The relay only sees encrypted traffic.

### Connection Flow
1. Endpoint binds, connects to a "home relay"
2. To connect to peer: resolve `EndpointId` → `EndpointAddr` via Address Lookup
3. Establish initial connection via relay
4. Attempt direct connection (hole-punching if needed)
5. Migrate to direct connection when available (relay becomes backup)

## Crate: `iroh` (Core Library)

### Main Types
| Type | Module | Purpose |
|------|--------|---------|
| `Endpoint` | `endpoint` | Central API — connect, accept, manage connections |
| `Builder` | `endpoint` | Configure and construct an `Endpoint` |
| `Router` | `protocol` | Accept loop that dispatches to `ProtocolHandler`s |
| `ProtocolHandler` | `protocol` | Trait for handling incoming connections by ALPN |
| `Connection` | `endpoint::connection` | QUIC connection wrapper |
| `Incoming` | `endpoint::connection` | Pre-handshake incoming connection |
| `Accepting` | `endpoint::connection` | Post-accept, pre-handshake state |

### Feature Flags
- `default` = `["metrics", "fast-apple-datapath", "portmapper", "tls-ring"]`
- `metrics` — Prometheus-style metrics collection
- `portmapper` — UPnP/PCP/NAT-PMP support
- `test-utils` — Testing utilities
- `platform-verifier` — Use OS TLS trust anchors
- `qlog` — QUIC event logging
- `fast-apple-datapath` — Private Apple APIs for batched sends
- `tls-ring` / `tls-aws-lc-rs` — Choose TLS crypto backend
- `unstable-custom-transports` — Custom transport API (unstable)

### WASM Support
The crate compiles to `wasm32-unknown-unknown` for browser targets. Browser builds:
- Use `PkarrResolver` instead of `DnsAddressLookup` (DNS-over-HTTPS)
- Cannot bind IP sockets (no direct connectivity)
- Use `wasm-bindgen-futures` for async runtime

## Presets

The `presets` module provides common configurations:

| Preset | Description |
|--------|-------------|
| `Empty` | No defaults — you must set all required options yourself |
| `Minimal` | Sets only the crypto provider (ring or aws-lc-rs) |
| `N0` | Full n0 defaults: crypto provider, Pkarr publisher, DNS resolver, n0 relay servers |
| `N0DisableRelay` | N0 defaults but with `RelayMode::Disabled` |

```rust
// Quick start with full n0 infrastructure
let endpoint = Endpoint::bind(presets::N0).await?;

// Minimal — just crypto, no relay or address lookup
let endpoint = Endpoint::bind(presets::Minimal).await?;
```

## Encryption & Authentication

Iroh uses **RFC 7250 Raw Public Keys** for TLS — no X.509 certificates. Each endpoint has:
- `SecretKey` (Ed25519) — used for TLS authentication and signing
- `PublicKey`/`EndpointId` — derived from `SecretKey`, used as identity

The TLS server name is encoded as `<base32-dnssec-encoded-public-key>.iroh.invalid` to ensure 0-RTT session ticket separation per endpoint.

## 0-RTT Support

Iroh supports QUIC 0-RTT connections:
- `Connecting::into_0rtt()` on the client side
- `Accepting::into_0rtt()` on the server side
- TLS session tickets cached per remote endpoint (default 256 tickets = ~150 KiB)
- `max_tls_tickets()` builder option to tune cache size

## Default Infrastructure (n0)

Production relay servers (4 regions):
| Region | Hostname |
|--------|----------|
| NA East | `use1-1.relay.n0.iroh-canary.iroh.link` |
| NA West | `usw1-1.relay.n0.iroh-canary.iroh.link` |
| EU | `euc1-1.relay.n0.iroh-canary.iroh.link` |
| AP | `aps1-1.relay.n0.iroh-canary.iroh.link` |

DNS Address Lookup origin: `dns.iroh.link`
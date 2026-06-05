# Alknet

> **Status: Alpha** — This project is in early development. It depends on solid libraries (russh, tokio, iroh) for core functionality, but the glue code and integration between them has not been fully vetted for production use. Because alknet operates low in the network stack, bugs can cause serious problems downstream (leaked connections, broken tunnels, auth failures). Use with caution and report issues.

A self-hostable SSH-based tunnel tool that provides VPN-like functionality without being a VPN protocol.

## What it does

- **Private tunneling** — Route traffic to internal services (Postgres, Redis, APIs) over SSH
- **Censorship circumvention** — SSH over TLS on port 443 is indistinguishable from HTTPS to DPI
- **NAT traversal** — The iroh transport enables peer-to-peer connections without public IPs or port forwarding
- **Service mesh connectivity** — Lightweight transport layer for event systems via reserved `alknet-*` destinations

The core insight: SSH tunnels work because SSH is fundamental infrastructure. Blocking it breaks the internet. Alknet makes SSH tunneling accessible through a simple CLI with pluggable transports.

## Quick start

### Build

```bash
cargo build --release
```

The default build includes TLS and iroh transports. To build a minimal binary with just TCP:

```bash
cargo build --release --no-default-features -p alknet
```

### Server

```bash
# Generate a host key
ssh-keygen -t ed25519 -f ssh_host_ed25519_key -N ""

# Start the server on port 22 (TCP)
alknet serve --key ssh_host_ed25519_key \
    --authorized-keys ~/.ssh/authorized_keys

# TLS with stealth mode (looks like nginx 404 to scanners)
alknet serve --key ssh_host_ed25519_key \
    --transport tls \
    --acme-domain example.com \
    --stealth

# iroh (no public IP needed)
alknet serve --key ssh_host_ed25519_key \
    --transport iroh
```

### Client

```bash
# Connect via TCP and start a SOCKS5 proxy on 127.0.0.1:1080
alknet connect --server example.com:22 \
    --identity ~/.ssh/id_ed25519

# Connect via TLS
alknet connect --server example.com:443 \
    --transport tls \
    --identity ~/.ssh/id_ed25519

# Connect via iroh (peer-to-peer, no public IP)
alknet connect --peer <endpoint-id> \
    --transport iroh \
    --identity ~/.ssh/id_ed25519

# With port forwarding
alknet connect --server example.com:22 \
    --identity ~/.ssh/id_ed25519 \
    --forward 5432:db.internal:5432 \
    --forward 6379:redis.internal:6379
```

### Use the SOCKS5 proxy

Once connected, point any SOCKS5-aware application at `127.0.0.1:1080`:

```bash
curl --socks5 127.0.0.1:1080 http://internal-api:8080/health
```

For VPN-like "route all traffic" behavior, use [tun2proxy](https://github.com/tun2proxy/tun2proxy) alongside alknet's SOCKS5 proxy (see [ADR-014](docs/architecture/decisions/014-defer-tun-recommend-socks5-proxy.md)).

## Crates

| Crate | Description |
|-------|-------------|
| `alknet-core` | Core library: transport trait, SOCKS5 server, port forwarding, auth, server handler |
| `alknet` | CLI binary (`alknet connect` / `alknet serve`) |
| `alknet-napi` | Node.js native addon via napi-rs (`connect()` / `serve()`) |

## Feature flags

| Feature | Crate | Default | Description |
|---------|-------|---------|-------------|
| `tls` | `alknet-core`, `alknet` | yes | TLS transport (tokio-rustls) |
| `iroh` | `alknet-core`, `alknet` | yes | iroh QUIC P2P transport |
| `acme` | `alknet-core` | no | ACME/Let's Encrypt auto-cert provisioning |
| `testutil` | `alknet-core` | no | Test utilities (for internal use) |

## Transport modes

| Transport | Client | Server | Notes |
|-----------|--------|--------|-------|
| **TCP** | `--transport tcp --server addr:port` | `--transport tcp --listen addr:port` | Direct SSH over TCP. Default. |
| **TLS** | `--transport tls --server addr:port` | `--transport tls --tls-cert/--tls-key or --acme-domain` | SSH wrapped in TLS. Looks like HTTPS. |
| **iroh** | `--transport iroh --peer <id>` | `--transport iroh` | QUIC P2P via iroh. No public IP needed. |

## Authentication

- **Ed25519 public keys** — Default. Load authorized keys from a file via `--authorized-keys`.
- **OpenSSH certificate authority** — Optional. Use `--cert-authority` for multi-user deployments.
- **No password authentication** — Key-based auth only (see [ADR-012](docs/architecture/decisions/012-auth-ed25519-and-cert-authority.md)).

Key formats are OpenSSH throughout (private keys: `-----BEGIN OPENSSH PRIVATE KEY-----`, public keys: `ssh-ed25519 AAAA...`). PEM-encoded keys (PKCS#1, PKCS#8) are not supported.

## Architecture

Alknet's core architectural decision is that SSH never touches the network directly. The transport layer produces a duplex byte stream, and SSH runs over it via `russh::client::connect_stream()` / `russh::server::run_stream()`. This makes transports fully pluggable.

```
Client                                          Server
  │  transport.connect()                          │  transport_acceptor.accept()
  │  ─────────────────────────────────────────────▶│
  │        (duplex byte stream established)        │
  │  russh::client::connect_stream(stream)        │  russh::server::run_stream(stream, handler)
  │  ═══════ SSH session over stream ═════════════ │
  │  channel_open_direct_tcpip(host, port)        │
  │  ─────────────────────────────────────────────▶│
  │  ┌─────── TCP proxy ──────────────────┐       │
  │  │  SSH channel ←→ TcpStream::connect │       │
  │  └────────────────────────────────────┘       │
```

See [docs/architecture/](docs/architecture/) for full specifications and [ADR index](docs/architecture/README.md).

## Node.js API

The `alknet-napi` crate provides a Node.js native addon via napi-rs:

```js
const { connect, serve } = require('alknet-napi');

// Client: open a duplex stream through SSH
const stream = await connect({
  server: "example.com:22",
  transport: "tcp",
  identity: "/path/to/key",
});
const data = await stream.read(1024);
await stream.write(Buffer.from("hello"));
await stream.close();

// Server: accept connections and receive streams
const server = await serve({
  transport: "tcp",
  hostKey: "/path/to/host_key",
  authorizedKeys: "/path/to/authorized_keys",
  listen: "0.0.0.0:22",
});
server.onConnection((event) => {
  const { stream, info } = event;
  // handle stream
});
```

### iroh (peer-to-peer)

iroh transport eliminates the need for public IPs or port forwarding. Both sides discover each other through a relay, then establish a direct QUIC connection. This is ideal for services behind NAT, distributed systems, or any scenario where opening ports is impractical.

```js
// Server: starts an iroh endpoint and prints its peer ID
const server = await serve({
  transport: "iroh",
  hostKey: "/path/to/host_key",
  authorizedKeys: "/path/to/authorized_keys",
  irohRelay: "https://relay.iroh.network/",  // optional, defaults to iroh's relay
  proxy: "socks5://proxy.example.com:1080", // optional, for restrictive networks
});
console.log("iroh endpoint ID:", server.endpointId);
// e.g. iroh endpoint ID: abc23xyz...

// Clients connect using that peer ID
const stream = await connect({
  peer: server.endpointId,
  transport: "iroh",
  identity: "/path/to/key",
  irohRelay: "https://relay.iroh.network/",  // must match the server's relay
  proxy: "socks5://proxy.example.com:1080",   // optional
});
```

The `endpointId` property returns the server's z-base-32 encoded iroh node ID. Share this ID with clients so they can connect — no DNS, no public IP, no port forwarding required.

### TLS

TLS transport wraps SSH in TLS, making the connection indistinguishable from HTTPS traffic to deep packet inspection:

```js
// Server
const server = await serve({
  transport: "tls",
  hostKey: "/path/to/host_key",
  authorizedKeys: "/path/to/authorized_keys",
  listen: "0.0.0.0:443",
  tlsCert: "/path/to/cert.pem",
  tlsKey: "/path/to/key.pem",
});

// Client
const stream = await connect({
  server: "example.com:443",
  transport: "tls",
  identity: "/path/to/key",
  tlsServerName: "example.com",  // optional, SNI hostname
  insecure: true,                 // accept self-signed certs (dev only)
});
```

## Status and stability

This is **alpha software**. While it depends on well-established libraries (russh, tokio, rustls, iroh) for SSH, async I/O, TLS, and QUIC respectively, the integration layer that ties them together has not been battle-tested. Potential concerns include:

- **Connection handling edge cases** — reconnection logic, graceful shutdown, resource cleanup
- **Security review** — the auth layer, rate limiting, and stealth mode should be audited before production use
- **API stability** — the library API (`alknet-core`) and NAPI interface may change between versions
- **Performance** — no load testing or benchmarking has been done yet

Please test thoroughly and [file issues](https://git.alk.dev/alkdev/alknet/issues) for any problems you encounter.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
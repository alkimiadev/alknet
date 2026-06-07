# SSH Tunnel VPN Alternative — Feasibility Assessment

**Date**: 2026-06-01  
**Status**: Feasibility assessment / architecture sketch  
**Updated**: 2026-06-01 — Added iroh transport analysis (§11)  

## 1. Problem Statement

Countries in the "developed west" (UK, CA, etc.) are increasingly banning or restricting VPNs at the protocol level. The valid use case of a VPN — a *virtual private network* for securing traffic on hostile networks, accessing private infrastructure, and tunneling between trusted endpoints — gets caught in the crossfire when VPNs are treated primarily as location-spoofing tools.

SSH-based tunnels cover the same functional ground without being a VPN protocol. Blocking SSH would break the internet in critical ways (infrastructure management, CI/CD, development workflows). The goal is to build a dead-simple, self-hostable Rust client/server that provides VPN-like functionality over SSH, with optional TLS wrapping for traffic obfuscation.

## 2. Reference Codebase Analysis

### 2.1 Dispatch (`/workspace/@alkdev/dispatch`)

Dispatch proves russh usage well within scope. Key takeaways:

- **Pure SSH client** — `client::Handler` is a zero-sized type, auto-accepts server keys. Minimal boilerplate.
- **Arc-wrapped Handle pattern** — `Arc<client::Handle<Client>>` enables sharing across concurrent tasks (port forwarding, SFTP, exec).
- **Port forwarding via `channel_open_direct_tcpip`** — Already implemented. Local TCP listener → `direct-tcpip` SSH channel → `tokio::io::copy_bidirectional`. This is the standard SSH `-L` pattern, implemented programmatically.
- **Channel-per-operation model** — Each operation opens its own SSH channel on a shared session. Multiplexing is handled by russh internally.
- **Channel.into_stream()** — Converts SSH channels to `AsyncRead + AsyncWrite` streams, enabling use with any tokio I/O combinator.

The dispatch codebase is clean and demonstrates that the core SSH mechanics are straightforward. The new project would need both client **and** server sides, but russh's server API mirrors the client API closely.

### 2.2 russh (`/workspace/russh`)

Critical capabilities confirmed:

| Feature | API | Status |
|---------|-----|--------|
| Local port forwarding (client → server → remote) | `Handle::channel_open_direct_tcpip()` | Available, no feature flag |
| Remote port forwarding (server listens, client gets channels) | `Handle::tcpip_forward()` / Handler callback `server_channel_open_forwarded_tcpip()` | Available, no feature flag |
| Unix socket forwarding | `Handle::channel_open_direct_streamlocal()` / `Handle::streamlocal_forward()` | Available, no feature flag |
| Server-side reverse forwarding | `server::Handler::tcpip_forward()` / `server::Handle::forward_tcpip()` | Available, no feature flag |
| Arbitrary stream transport | `client::connect_stream()` / `server::run_stream()` | **Both accept `AsyncRead+AsyncWrite+Unpin+Send`** |
| Channel as bidirectional stream | `Channel::into_stream()` / `split()` | Available |

**The `connect_stream()` and `run_stream()` APIs are the key enabler for TLS wrapping.** They accept any async byte stream, meaning we can layer TLS (via `tokio-rustls`) underneath russh without modifying russh itself. The SSH session runs over a TLS stream, which looks like HTTPS to DPI.

## 3. Architecture Sketch

### 3.1 Components

```
┌─────────────────────────────────┐         ┌─────────────────────────────────┐
│           CLIENT                │         │           SERVER                │
│                                 │         │                                 │
│  ┌──────────┐    ┌───────────┐  │         │  ┌───────────┐    ┌──────────┐ │
│  │  TUN     │    │  SSH      │  │  SSH    │  │  SSH      │    │  Proxy   │ │
│  │ Interface│───▶│  Client   │──┼─ over ──▶│  Server    │───▶│  Handler │ │
│  │  (tun-rs)│◀───│  (russh)  │  │  TLS    │  (russh)   │◀───│          │ │
│  └──────────┘    └─────┬─────┘  │  opt.   │  └─────┬─────┘    └────┬─────┘ │
│                        │        │         │        │                 │       │
│                  ┌─────▼─────┐  │         │  ┌─────▼─────┐    ┌────▼─────┐ │
│                  │ TLS Layer │  │         │  │ TLS Layer │    │ Outbound  │ │
│                  │(tokio-    │  │         │  │(tokio-    │    │ Proxy     │ │
│                  │ rustls)   │  │         │  │ rustls)   │    │(SOCKS5/   │ │
│                  └─────┬─────┘  │         │  └─────┬─────┘    │  HTTP)    │ │
│                        │        │         │        │          └────┬─────┘ │
│                  ┌─────▼─────┐  │         │  ┌─────▼─────┐         │       │
│                  │  TCP      │  │         │  │  TCP      │    ┌────▼─────┐ │
│                  │  Connect  │◀─┼────────▶│  │  Listener │    │ Direct   │ │
│                  └───────────┘  │         │  └───────────┘    │ Forward  │ │
│                                 │         │                    └────┬─────┘ │
└─────────────────────────────────┘         └─────────────────────────────────┘
                                                   │                  │
                                              Proxy Mode        Direct Mode
                                           (outbound via       (outbound
                                            SOCKS5/HTTP)       direct TCP)
```

### 3.2 Data Flow — Client TUN Mode

1. **TUN interface** (created via `tun-rs`) captures IP packets from the OS routing table
2. **Client reads IP packets** from the TUN device, determines destination IP:port
3. **Client opens `direct-tcpip` SSH channel** to destination via `handle.channel_open_direct_tcpip(dest_ip, dest_port, ...)`
4. **Client writes packet payload** to the SSH channel, reads response
5. **Client writes response** back to TUN interface

This is essentially what tun2proxy does, except instead of SOCKS5 upstream, it's an SSH channel.

### 3.3 Data Flow — TLS Obfuscation Mode

When `--tls` or `--https` is specified:

1. **Client establishes TLS connection** to `server:443` using `tokio-rustls::TlsStream`
2. **SSH session runs over the TLS stream** via `client::connect_stream(Arc::new(config), tls_stream, handler)`
3. **Server accepts TLS connection**, then runs `server::run_stream(server_config, tls_stream, handler)`
4. **To DPI, the traffic looks like HTTPS** — standard TLS handshake, then encrypted application data
5. Optional: Server can present a legitimate-looking certificate and serve a fake nginx 404 to non-SSH probes (similar to https_proxy's stealth approach)

### 3.4 Data Flow — Server-Side Proxy Mode

When `--proxy` is specified on the server:

1. Client requests `channel_open_direct_tcpip(target_host, target_port, ...)`
2. Server's `channel_open_direct_tcpip` handler checks ACLs
3. Instead of connecting directly, server routes through a local SOCKS5/HTTP proxy
4. This provides an additional hop for privacy — the SSH server's IP isn't exposed to the destination

### 3.5 CLI Interface Sketch

```bash
# Server — simplest mode (SSH only, port 22)
ghost serve --key /etc/ssh/ssh_host_ed25519_key

# Server — with TLS on port 443
ghost serve --key /etc/ssh/ssh_host_ed25519_key --tls --tls-cert /etc/ssl/cert.pem --tls-key /etc/ssl/key.pem

# Server — with TLS + outbound proxy
ghost serve --key /etc/ssh/ssh_host_ed25519_key --tls --tls-cert /etc/ssl/cert.pem --tls-key /etc/ssl/key.pem --proxy socks5://127.0.0.1:9050

# Client — TUN mode (routes all traffic through SSH tunnel)
ghost connect --server example.com:443 --tls --identity ~/.ssh/id_ed25519 --tun

# Client — Single port forward (like SSH -L)
ghost connect --server example.com:443 --tls --identity ~/.ssh/id_ed25519 --forward 5432:db.internal:5432

# Client — SOCKS5 proxy mode (local SOCKS5 that tunnels through SSH)
ghost connect --server example.com:443 --tls --identity ~/.ssh/id_ed25519 --socks5 1080
```

**Working name: `ghost`** (as in "ghost in the shell" — it's SSH, it's stealthy, it passes through walls). Or `shade`, `wraith`, `spectre`. Pick anything.

## 4. Key Technical Decisions & Unknowns Analysis

### 4.1 TUN Interface — SOLVED

**Library: `tun-rs` (v2, formerly `tun` crate)**

- Supports Linux, macOS, Windows (via wintun.dll), FreeBSD, OpenBSD, NetBSD, Android, iOS
- Async API with `tokio` feature: `DeviceBuilder::new().build_async()`
- Clean `recv()` / `send()` API — read IP packets, write IP packets
- Already used in production by tun2proxy and similar projects
- Supports hardware offload (TSO/GSO) on Linux for performance
- No `CAP_NET_ADMIN` needed on some platforms when using `--unshare` namespace approach (tun2proxy pattern)

**This is a solved problem.** The `tun-rs` crate is mature, cross-platform, and async-native with tokio. The implementation is straightforward:

```rust
let dev = DeviceBuilder::new()
    .ipv4("10.0.0.1", 24, None)
    .mtu(1400)
    .build_async()?;

let mut buf = vec![0u8; 65536];
loop {
    let len = dev.recv(&mut buf).await?;
    // Parse IP header, determine destination
    // Open SSH channel to destination
    // Write response back to TUN
}
```

**Key consideration**: On Linux requires `CAP_NET_ADMIN` or root. The tun2proxy approach of using network namespaces (`--unshare`) is worth adopting for unprivileged operation.

### 4.2 SSH over TLS — SOLVED (architecturally)

**Approach: Layer TLS beneath SSH using russh's `connect_stream` / `run_stream`**

This is the critical insight. russh already decouples transport from protocol:

- `client::connect_stream(config, stream, handler)` — accepts any `AsyncRead + AsyncWrite + Unpin + Send`
- `server::run_stream(config, stream, handler)` — same for server

This means:

```rust
// Client side
let tcp_stream = TcpStream::connect((server_addr, server_port)).await?;
let tls_stream = TlsStream::connect(tls_connector, server_domain, tcp_stream).await?;
let handle = client::connect_stream(config, tls_stream, handler).await?;

// Server side  
let (tcp_stream, addr) = tcp_listener.accept().await?;
let tls_stream = TlsStream::accept(tls_acceptor, tcp_stream).await?;
server::run_stream(config, tls_stream, handler).await?;
```

**No modification to russh is needed.** This is a clean layering.

**For HTTPS stealth**: The server can:
1. Accept connections on port 443
2. Present a valid TLS certificate (self-signed or Let's Encrypt via ACME)
3. Non-SSH clients making HTTP requests get a normal-looking 404 response
4. SSH clients speak SSH protocol directly after TLS handshake
5. DPI sees standard HTTPS traffic since the TLS handshake is normal

The https_proxy project demonstrates this pattern well — stealth proxy returning fake nginx 404s to probes.

### 4.3 IP Packet Handling — NEEDS DESIGN

When using TUN mode, we're receiving raw IP packets. We need to:

1. **Parse IP headers** to determine destination IP and port
2. **Track connection state** — map `(src_ip, src_port, dst_ip, dst_port)` to SSH channels
3. **TCP reassembly** — handle segmentation, retransmission, etc.
4. **ICMP handling** — respond to pings, handle unreachable destinations
5. **DNS interception** — handle DNS queries that arrive at the TUN interface

This is the most complex part. Options:

**Option A: Use a userspace TCP/IP stack (smoltcp)**
- Parse packets, but let a userspace stack handle TCP
- Heavier dependency, but proven approach (what tun2proxy does with its own stack)
- `smoltcp` is well-maintained, used in embedded and networking projects

**Option B: Raw packet forwarding with NAT**
- Simpler conceptually — just NAT the packets, forward them through the SSH channel
- Requires handling TCP state at the IP level (seq/ack manipulation, checksum recalculation)
- More error-prone

**Option C: SOCKS5 proxy mode only (no TUN)**
- Simplest to implement — just a local SOCKS5 server that forwards through SSH
- Browsers, curl, and most apps can use SOCKS5
- No root/CAP_NET_ADMIN needed
- But: doesn't capture all traffic (UDP, DNS leaks, etc.)

**Recommendation**: Start with Option C (SOCKS5 proxy mode) as the minimal viable product. Add TUN mode (Option A with smoltcp) as an advanced feature. This matches how tun2proxy structures their project and is the pragmatic path.

### 4.4 SSH Server Authentication — STRAIGHTFORORD

The server implementation needs:

- **Public key authentication** — primary method, matching standard SSH practices
- **`authorized_keys` file support** — read `~/.ssh/authorized_keys` or a custom path
- **Optional password authentication** — for convenience, but not recommended for production

russh's `server::Handler` trait provides `auth_publickey` and `auth_password` callbacks. Implementation is trivial:

```rust
async fn auth_publickey(&mut self, user: &str, public_key: &PublicKey) -> Auth {
    if self.authorized_keys.iter().any(|k| k == public_key) {
        Auth::Accept
    } else {
        Auth::Reject { proceed_with_methods: None, partial_success: false }
    }
}
```

### 4.5 DNS Handling — DESIGN DECISION NEEDED

In TUN mode, DNS queries need to be routed through the tunnel. Options:

1. **Virtual DNS** (tun2proxy approach) — intercept DNS packets, map query names to fake IPs from a reserved range (198.18.0.0/15), resolve via the SSH tunnel
2. **DNS-over-TCP** — Force DNS through the SSH tunnel
3. **Direct DNS** — Don't handle DNS in the tunnel, rely on system resolver
4. **SOCKS5 mode** — SOCKS5 supports DOMAIN names natively (SOCKS5h), so DNS resolution happens server-side

**Recommendation**: SOCKS5 mode handles DNS naturally via SOCKS5h. For TUN mode, adopt the virtual DNS approach from tun2proxy (their `ip-stack` crate handles this).

### 4.6 Connection Multiplexing — ALREADY SOLVED

russh multiplexes channels over a single SSH connection. No need to manage multiple TCP connections per tunnel. One SSH connection, many channels. This is exactly what we want.

### 4.7 Keep-Alive and Reconnection — NEEDS DESIGN

- **SSH keepalive**: russh `Config` has `keepalive_interval` and `keepalive_max`
- **Auto-reconnect**: Client should detect disconnection (`is_closed()`) and reconnect with exponential backoff
- **TUN continuity**: When SSH reconnects, existing TCP connections through the tunnel will fail, but new ones will work. This is acceptable behavior (same as any VPN).

### 4.8 Server-Side Proxy (Outbound) — STRAIGHTFORORD

When `--proxy` is specified, the server's `channel_open_direct_tcpip` handler forwards through a local proxy:

```rust
async fn channel_open_direct_tcpip(
    &mut self,
    host: &str,
    port: u32,
    ...
) -> Result<Channel<Msg>, Self::Error> {
    // Option 1: Connect directly
    let stream = TcpStream::connect((host, port as u16)).await?;
    
    // Option 2: Connect through SOCKS5 proxy
    let stream = connect_socks5(proxy_addr, host, port).await?;
    
    // Option 3: Connect through HTTP CONNECT proxy
    let stream = connect_http_proxy(proxy_addr, host, port).await?;
    
    // Then bidirectional copy between SSH channel and stream
    Ok(channel)
}
```

SOCKS5 client implementation is simple (5-byte handshake, variable-length connect). HTTP CONNECT is also straightforward. Both can be implemented in a few hundred lines.

## 5. Dependency Assessment

| Dependency | Purpose | Maturity | Risk |
|------------|---------|----------|------|
| `russh` | SSH client & server | High (used in dispatch, well-maintained) | Low — already proven |
| `tun-rs` (v2) | TUN/TAP interface | High (cross-platform, prod-tested, bench'd at 70Gbps) | Low — well-maintained |
| `tokio-rustls` | TLS layer | High (standard Rust TLS) | Low — widely used |
| `rustls` | TLS implementation | High | Low — no ring dependency needed with aws-lc-rs |
| `smoltcp` | Userspace TCP/IP stack (TUN mode) | Medium-High | Medium — complex but well-proven |
| `clap` | CLI args | High | None |
| `tracing` | Structured logging | High | None |
| `anyhow/thiserror` | Error handling | High | None |
| `tokio` | Async runtime | High | None |

**No immature or risky dependencies.** Every crate is well-established with active maintenance.

## 6. Risk Assessment

### 6.1 Technical Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| TUN mode complexity (TCP state, IP parsing) | Medium | Medium | Start with SOCKS5 mode; TUN is advanced feature |
| Cross-platform TUN differences | Medium | Medium | tun-rs handles most; `--unshare` for Linux privilege separation |
| TLS + SSH interaction edge cases | Low | Low | Both are well-tested; russh's `connect_stream` / `run_stream` abstracts transport |
| Performance under load | Low | Medium | russh multiplexes channels; tun-rs has benchmarked 35+ Gbps async |
| DPI detecting SSH banner over TLS | Medium | High | After TLS, the SSH banner ("SSH-2.0-...") is encrypted. But SNI reveals domain. Use `Config { anonymous: true }` to minimize fingerprint, or configure `client_id` to look like a web server. |

### 6.2 Protocol-Level Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| SSH protocol fingerprinting (packet sizes, timing) | Medium | Medium | Pad messages, add random delays. russh doesn't do this natively — would need custom channel wrapping. |
| SNI leaks domain in TLS handshake | High | Low | Use a innocuous domain. Could also explore ECH (Encrypted Client Hello) in rustls if available. |
| Deep packet inspection identifying SSH patterns even over TLS | Low-Medium | Medium | The TLS layer prevents payload inspection. Only traffic analysis (sizes, timing) is possible. Padding and traffic shaping could help. |
| Countries blocking SSH traffic on port 22 | Already happening | N/A | That's the whole point — we run SSH over TLS on port 443 |

### 6.3 Usability Risks

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Requires self-hosted server | By design | Medium | Document simple deployment. Provide Docker image. Consider one-command install script. |
| Root/CAP_NET_ADMIN needed for TUN on Linux | High | Medium | Provide `--unshare` mode. SOCKS5 mode needs no privileges. |
| Certificate management for TLS mode | Medium | Low | Support self-signed certs, ACME (Let's Encrypt), or manual cert paths. |

## 7. Implementation Plan

### Phase 1: MVP (2-3 days)

**SOCKS5 proxy mode only. No TUN. Client + server.**

1. **Server binary** (`ghost serve`)
   - russh server implementation with public key auth
   - `channel_open_direct_tcpip` handler: connect to target directly or via outbound proxy
   - Optional TLS wrapping via `tokio-rustls` + `server::run_stream`
   - Config: listen address, host key path, authorized keys, TLS options, proxy options

2. **Client binary** (`ghost connect`)
   - russh client with public key auth
   - Local SOCKS5 server that forwards connections through SSH `channel_open_direct_tcpip`
   - Optional TLS wrapping via `tokio-rustls` + `client::connect_stream`
   - Config: server address, identity key, TLS options, SOCKS5 listen address

3. **Testing**
   - Integration test: client → server → HTTP target
   - Test with: `curl --socks5-hostname 127.0.0.1:1080 https://example.com`
   - Test TLS mode against DPI-like inspection

### Phase 2: Port Forwarding (1 day)

4. **Client: explicit port forwards** (`--forward local:remote:port`)
   - Direct reimplementation of SSH `-L` and `-R`
   - Uses `channel_open_direct_tcpip` for local forwards
   - Uses `tcpip_forward` / handler callback for remote forwards

5. **Client: SOCKS5 with DNS** (SOCKS5h)
   - Domain names resolved server-side, not client-side

### Phase 3: TUN Mode (2-3 days)

6. **Client: TUN interface mode** (`--tun`)
   - Create TUN device via `tun-rs`
   - IP packet routing through SSH channels
   - Either: raw packet forwarding (simpler, but fragile) or smoltcp integration (robust, but more code)
   - Recommend: use tun2proxy's `ip-stack` crate or similar for TCP reconstruction
   - Virtual DNS for TUN mode

7. **Privilege separation**
   - `--unshare` mode for Linux (create network namespace, unshare)
   - Document CAP_NET_ADMIN requirement

### Phase 4: Hardening & Polish (1-2 days)

8. **Obfuscation improvements**
   - SSH banner customization (`client_id` config)
   - Random padding in channel data
   - Traffic shaping / constant-rate padding (optional, advanced)

9. **Server stealth**
   - Non-SSH connection detection: serve fake nginx 404 on TLS port
   - Dual-protocol listener: HTTPS for browsers, SSH for ghost clients

10. **Auto-reconnect**
    - Exponential backoff reconnect on SSH session drop
    - TUN interface survives reconnect (new connections work, in-flight connections fail gracefully)

### Phase 5: Distribution (1 day)

11. **Build & packaging**
    - Static musl binary for Linux
    - Docker image
    - systemd unit file
    - One-line install script

## 8. Estimated Timeline

| Phase | Duration | Cumulative |
|-------|----------|------------|
| Phase 1: SOCKS5 MVP | 2-3 days | 2-3 days |
| Phase 2: Port Forwarding | 1 day | 3-4 days |
| Phase 3: TUN Mode | 2-3 days | 5-7 days |
| Phase 4: Hardening & Polish | 1-2 days | 6-9 days |
| Phase 5: Distribution | 1 day | 7-10 days |

With LLM-assisted development, the MVP (Phase 1) could realistically be done in 1-2 focused sessions. The full feature set in under a week.

## 9. Open Questions

1. **Project name** — `ghost`, `wraith`, `shade`, `spectre`, something else? Needs to be catchy, not conflict with existing Rust crates, and suggest stealth/mobility.

2. **TUN vs smoltcp** — Should TUN mode integrate smoltcp for a userspace TCP stack, or try the simpler "just forward packets and let the OS handle TCP" approach? Smoltcp is more work but more robust. tun2proxy's approach (which uses their own `ip-stack`) suggests userspace TCP is the way to go for reliability.

3. **TLS certificate story** — Should the server support ACME/Let's Encrypt auto-provisioning (like https_proxy does), or is manual cert management sufficient? Auto-provisioning is more user-friendly but adds significant complexity and a dependency on the ACME protocol.

4. **Mobile support** — Should we target iOS/Android eventually? tun-rs supports both via platform APIs, but mobile is a much bigger scope. Probably Phase 6+.

5. **Multi-user server** — Should the server support multiple simultaneous clients? russh's server model handles this naturally (each connection gets its own Handler instance), but access control (per-user ACLs, bandwidth limits) would add complexity.

6. **Crates structure** — Single binary with subcommands (`ghost serve`, `ghost connect`), or separate binaries? Single crate with `#[tokio::main]` dispatch seems cleanest for MVP.

## 10. Conclusion

**This is feasible and straightforward.** The core mechanics — SSH tunnel via russh, TLS wrapping via tokio-rustls, TUN interface via tun-rs — are all solved problems with mature Rust libraries. The dispatch codebase proves russh is production-ready for this kind of work. The `connect_stream` / `run_stream` API in russh makes TLS wrapping a clean layering, not a hack.

The biggest design decision is TUN mode approach (raw packets vs. userspace TCP), and the recommendation is to start with SOCKS5 mode and add TUN later. This gives a working tool in 2-3 days that covers the primary use case (private tunneling that doesn't look like VPN traffic).

The project is well-scoped, the risk profile is low, and the existing tooling (russh, tun-rs, tokio-rustls) handles the hard parts. This is a "few days of focused work" estimate, not a "few weeks."

## 11. iroh Transport — Feasibility Addendum

### 11.1 The Insight

russh's `connect_stream()` and `server::run_stream()` accept **any** `AsyncRead + AsyncWrite + Unpin + Send` stream. The iroh project provides exactly such a stream — a QUIC bidirectional stream (`open_bi()` / `accept_bi()`) where both `SendStream` and `RecvStream` implement `tokio::io::AsyncWrite` and `tokio::io::AsyncRead` respectively.

This means **iroh can serve as a transport layer beneath SSH**, the same way TLS can. The architecture becomes:

```
┌──────────────────────────────────────────────────┐
│                  APPLICATION                      │
│              (SOCKS5 / TUN / port-forward)        │
├──────────────────────────────────────────────────┤
│              SSH (russh)                          │
│         channel_open_direct_tcpip/etc.           │
├──────────────────────────────────────────────────┤
│           Transport Layer (SWAPPABLE)            │
│                                                  │
│   ┌──────────┐  ┌──────────┐  ┌──────────────┐ │
│   │   TCP    │  │   TLS    │  │    iroh       │ │
│   │(direct)  │  │(obfusc)  │  │  (P2P QUIC)  │ │
│   └──────────┘  └──────────┘  └──────────────┘ │
└──────────────────────────────────────────────────┘
```

### 11.2 Why iroh is Compelling

iroh solves the **biggest deployment problem** with SSH tunnels: the server needs a public IP and open port.

With iroh as transport:

1. **No public IP needed** — Server and client both connect outbound to iroh's relay servers. Hole-punching attempts direct UDP in the background.
2. **No open firewall ports** — The server only needs outbound HTTPS to the relay. No inbound 22 or 443 required.
3. **NAT traversal for free** — iroh's relay + hole-punching means peers behind CGNAT or strict firewalls can still connect.
4. **Ed25519-based addressing** — Peers are identified by public key (EndpointId), no DNS or IP addresses needed.
5. **Built-in address discovery** — pkarr DNS records let you find a peer knowing only their public key.
6. **Still SSH underneath** — All the channel multiplexing, port forwarding, SOCKS5 logic still works. iroh is just the wire.

The use cases multiply:

- **Home server behind NAT**: No reverse proxy, no dynamic DNS, no port forwarding. Just run the server, share the EndpointId.
- **Temporary infrastructure**: Spin up a server anywhere (even behind corporate NAT), connect by public key.
- **Internal services**: Expose Postgres/Redis etc. over an SSH connection that traverses any NAT, no VPN required.
- **Censorship circumvention**: SSH over iroh QUIC to a relay that uses standard HTTPS. The deep packet inspector sees HTTPS traffic to a relay server, not SSH.

### 11.3 How It Works — The Code

The integration is trivially clean because both primitives implement the right traits:

**Client side:**
```rust
// Create iroh endpoint
let endpoint = Endpoint::builder(presets::N0)
    .alpns(vec![b"ghost-ssh/1".to_vec()])
    .bind()
    .await?;

// Connect to peer (no IP needed — just public key)
let addr = EndpointAddr::from_bytes(peer_id_bytes);
let conn = endpoint.connect(addr, b"ghost-ssh/1").await?;

// Open a bidirectional QUIC stream
let (send_stream, recv_stream) = conn.open_bi().await?;

// Combine into a single AsyncRead+AsyncWrite
let iroh_stream = tokio::io::join(recv_stream, send_stream);
// OR use a custom wrapper that implements AsyncRead+AsyncWrite

// Run SSH client over the iroh stream
let handle = client::connect_stream(
    Arc::new(client_config),
    iroh_stream,
    client_handler
).await?;
```

**Server side:**
```rust
// Create iroh endpoint
let endpoint = Endpoint::builder(presets::N0)
    .alpns(vec![b"ghost-ssh/1".to_vec()])
    .bind()
    .await?;

// Accept incoming connections
while let Some(incoming) = endpoint.accept().await {
    let conn = incoming.await?;
    
    // For each connection, accept a bidirectional stream
    let (send_stream, recv_stream) = conn.accept_bi().await?;
    let iroh_stream = tokio::io::join(recv_stream, send_stream);
    
    // Run SSH server over the iroh stream
    server::run_stream(
        Arc::new(server_config),
        iroh_stream,
        server_handler
    ).await?;
}
```

**Or using iroh's Router + ProtocolHandler pattern:**
```rust
struct GhostSshProtocol;

impl ProtocolHandler for GhostSshProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // iroh already handled connection acceptance
        // We can accept bi streams on the connection directly
        // Or: each SSH session could be a new bi stream on the same connection
        
        let (send, recv) = connection.accept_bi().await
            .map_err(AcceptError::from_err)?;
        let stream = join_streams(recv, send);
        
        server::run_stream(server_config, stream, GhostHandler).await
            .map_err(AcceptError::from_err)
    }
}

let endpoint = Endpoint::builder(presets::N0).bind().await?;
let router = Router::builder(endpoint)
    .accept(b"ghost-ssh/1", GhostSshProtocol)
    .spawn();
```

### 11.4 Design Decision: One Stream per Session vs. One Connection with Multiple Streams

There are two ways to layer SSH over iroh:

**Option A: One QUIC bi-stream per SSH session**
- Each SSH session opens a new `open_bi()` stream under a single iroh `Connection`
- The iroh Connection itself persists (one QUIC connection per peer pair)
- Simpler: `open_bi()` gives you a stream, you feed it to `connect_stream()`
- Pro: Connection setup cost amortized. If SSH disconnects, `open_bi()` again is cheap.
- Con: Need to combine `RecvStream` + `SendStream` into a single `AsyncRead+AsyncWrite`

**Option B: One iroh Connection per SSH session (new QUIC connection each time)**
- Each SSH session = one `endpoint.connect()` + the whole connection
- Wasteful: QUIC handshake + iroh relay discovery each time
- Not recommended

**Recommendation: Option A.** One iroh `Connection` per peer pair, one `open_bi()` stream per SSH session. The connection is long-lived; SSH sessions can be re-established cheaply on the same QUIC connection.

### 11.5 Combining `RecvStream + SendStream` into `AsyncRead + AsyncWrite`

QUIC splits streams into separate send and receive halves. russh needs a single duplex stream. Two approaches:

**Approach 1: `tokio::io::join()` (simplest)**
```rust
use tokio::io;

fn join_iroh_stream(
    recv: iroh::endpoint::RecvStream,
    send: iroh::endpoint::SendStream,
) -> impl AsyncRead + AsyncWrite + Unpin + Send {
    io::join(recv, send)
}
```
`tokio::io::join` returns a `Join<A, B>` that implements both `AsyncRead` (from the first) and `AsyncWrite` (from the second). Since `RecvStream: AsyncRead` and `SendStream: AsyncWrite`, this works directly.

**Approach 2: Custom wrapper (more control)**
```rust
struct IrohStream {
    recv: iroh::endpoint::RecvStream,
    send: iroh::endpoint::SendStream,
}

impl AsyncRead for IrohStream { /* delegate to recv */ }
impl AsyncWrite for IrohStream { /* delegate to send */ }
```

**Recommendation: Start with `tokio::io::join`.** It's one line and has the right trait implementations. Only switch to a custom wrapper if profiling shows overhead (unlikely).

### 11.6 Relay Considerations

iroh provides two relay options:

1. **Default n0 relay servers** (`https://use1-1.relay.n0.iroh.network.`) — free, operated by n0. Good for getting started and testing.
2. **Self-hosted relay** (`iroh-relay` crate) — The relay server is part of the iroh project. Can be self-hosted for complete independence.

For this project:

- **Development/quick start**: Use n0 relays (they're free and reliable)
- **Production/privacy**: Self-host the relay server. It's a single binary (`iroh-relay`) that can run on any VPS. The relay sees only encrypted QUIC packets — it cannot read SSH traffic.
- **Paranoid**: Disable relay entirely. Both peers must have direct network connectivity. No third-party dependency.

The `RelayMode` enum handles this:
```rust
// Default n0 relays
let endpoint = Endpoint::builder(presets::N0).bind().await?;

// Self-hosted relay
let relay_map = RelayMap::from([(relay_url, Some(direct_addr))]);
let endpoint = Endpoint::builder(presets::Custom(relay_map)).bind().await?;

// No relay (direct only)
let endpoint = Endpoint::builder(presets::RelayDisabled).bind().await?;
```

### 11.7 Updated Architecture with iroh Transport

```
┌───────────────────────────────────────────────────────────┐
│                      CLIENT                               │
│                                                           │
│  ┌──────────┐    ┌───────────┐    ┌────────────────────┐ │
│  │  TUN /   │    │   SSH     │    │   Transport        │ │
│  │ SOCKS5 / │───▶│  Client   │───▶│   (selectable)     │ │
│  │ Port-    │    │  (russh)  │    │                    │ │
│  │ Forward  │    │           │    │  ┌────────────────┐ │ │
│  └──────────┘    └───────────┘    │  │ TCP direct     │ │ │
│                                   │  │ TLS (rustls)   │ │ │
│                                   │  │ iroh (QUIC)    │ │ │
│                                   │  └────────────────┘ │ │
│                                   └────────────────────┘ │
└───────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────┐
│                     SERVER                                │
│                                                           │
│  ┌──────────┐    ┌───────────┐    ┌────────────────────┐ │
│  │ Outbound │    │   SSH     │    │   Transport        │ │
│  │ Proxy /  │◀───│  Server   │◀───│   (selectable)     │ │
│  │ Direct   │    │  (russh)  │    │                    │ │
│  │ Forward  │    │           │    │  ┌────────────────┐ │ │
│  └──────────┘    └───────────┘    │  │ TCP listener   │ │ │
│                                   │  │ TLS (rustls)   │ │ │
│                                   │  │ iroh (QUIC)    │ │ │
│                                   │  └────────────────┘ │ │
│                                   └────────────────────┘ │
└───────────────────────────────────────────────────────────┘

                    ┌──────────────┐
                    │ iroh Relay   │  (optional, for NAT)
                    │ (self-host   │
                    │  or n0)      │
                    └──────────────┘

Transport modes:
  --transport tcp          Direct TCP (default, simplest)
  --transport tls          TCP + TLS (obfuscation)
  --transport iroh         iroh QUIC (NAT traversal, no public IP)
  --transport iroh+tls     iroh QUIC + TLS (NAT traversal + obfuscation)
```

### 11.8 iroh Transport — Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| iroh API instability (it's v0.x) | Medium | Medium | Pin version; iroh's core stream API is stable (it's just QUIC) |
| Relay dependency for initial connectivity | Low | Low | Self-host relay; or direct-only mode for LAN |
| QUIC stream vs TCP semantics differences | Low | Medium | QUIC streams are reliable ordered byte streams, same semantics as TCP. russh won't know the difference. |
| Performance overhead of QUIC + SSH | Low | Low | QUIC is fast. SSH over QUIC might actually be *faster* than SSH over TCP due to QUIC's multipath and no head-of-line blocking. |
| iroh crate size / compile time | Low | Low | iroh pulls in quinn + rustls + lots of networking. But we already need rustls for TLS mode. The incremental cost is the QUIC stack. |

**Key observation**: QUIC streams have identical reliability and ordering guarantees to TCP. russh's `connect_stream()` / `run_stream()` will work correctly over iroh QUIC streams with no modifications.

### 11.9 Updated CLI Sketch with iroh

```bash
# Server — iroh mode (no public IP needed!)
ghost serve --key ~/.ssh/id_ed25519 --transport iroh
# Prints endpoint ID: e.g., "abc123..."
# Clients connect using this ID

# Server — iroh mode with self-hosted relay
ghost serve --key ~/.ssh/id_ed25519 --transport iroh \
    --iroh-relay https://my-relay.example.com

# Client — connect via iroh (no IP needed!)
ghost connect --peer abc123def456... --transport iroh --socks5 1080

# Client — connect via iroh with TUN
ghost connect --peer abc123def456... --transport iroh --tun

# Client — traditional TCP mode (still works)
ghost connect --server 1.2.3.4:443 --transport tls --socks5 1080
```

### 11.10 Implementation Impact

Adding iroh as a transport option is **incremental** — it doesn't change the SSH layer at all:

1. **Transport trait**: Define a `Transport` trait that produces `Box<dyn AsyncRead + AsyncWrite + Unpin + Send>`:
   ```rust
   trait Transport {
       async fn connect(&self) -> Result<Box<dyn AsyncRead + AsyncWrite + Unpin + Send>>;
   }
   ```

2. **Three implementations**:
   - `TcpTransport` — plain TCP
   - `TlsTransport` — TCP + tokio-rustls
   - `IrohTransport` — iroh endpoint + `open_bi()` + `tokio::io::join(recv, send)`

3. **Server side**: Same trait, different direction:
   ```rust
   trait TransportAcceptor {
       async fn accept(&self) -> Result<Box<dyn AsyncRead + AsyncWrite + Unpin + Send>>;
   }
   ```

4. **The SSH layer never changes.** russh's `connect_stream()` / `run_stream()` takes the transport stream, and everything else stays the same.

### 11.11 Dependency Impact

| Dependency | Added? | Size concern |
|------------|--------|-------------|
| `iroh` (includes iroh-base) | Yes, feature-gated | Yes — pulls in QUIC stack, DNS, relay client |
| `n0-error` | Yes (small) | No |
| `tokio` | Already present | No |
| `rustls` | Already present (for TLS mode) | No |

**Recommendation**: Make iroh a feature flag (`--features iroh`) so the base install stays lean. Users who want P2P capability opt in:

```toml
[features]
default = ["tls"]
tls = ["tokio-rustls", "rustls-pemfile"]
iroh = ["dep:iroh"]
tun = ["dep:tun-rs", "dep:smoltcp"]
```

### 11.12 The Compelling Narrative

With iroh as a transport option, this tool becomes something genuinely new:

- **Not just a VPN alternative** — it's a VPN alternative that doesn't need port forwarding, public IPs, or DNS records.
- **Not just SSH tunneling** — it's SSH tunneling that works between any two machines on the internet, regardless of NAT configuration.
- **Not just for censorship circumvention** — it's how you securely expose internal services (Postgres, Redis, admin panels) from machines behind corporate firewalls or home networks.

The "ghetto VPN" becomes a **zero-config mesh VPN**. Spin up `ghost serve` on any machine, share the public key, connect from anywhere. The relay server is optional (self-host or n0's free tier). And underneath it's just SSH, doing what SSH does best.

This isn't theoretical — the API compatibility is exact. iroh's `RecvStream + SendStream` implement `AsyncRead + AsyncWrite`, and russh's `connect_stream` / `run_stream` accept `AsyncRead + AsyncWrite`. Three lines of `tokio::io::join(recv, send)` and you have a transport stream that russh can use.
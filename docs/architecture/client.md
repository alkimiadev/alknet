---
status: reviewed
last_updated: 2026-06-02
---

# Client

## What

The alknet client establishes an SSH session to a server (via pluggable transport) and exposes a local SOCKS5 proxy for routing traffic through that session. Port forwarding (`-L` / `-R` style) covers specific service access like Postgres or Redis.

## Why

Users need a way to route traffic through the SSH tunnel. SOCKS5 is the primary interface — it's standard, well-supported by browsers and CLI tools, and needs no privileges. Port forwarding covers specific service access. For VPN-like "route all traffic" behavior, users run `tun2proxy` alongside alknet (ADR-014).

## Architecture

### Client Components

```
┌────────────────────────────────────────────────────────┐
│                     alknet connect                      │
│                                                        │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐              │
│  │ SOCKS5   │ │ Port     │ │ Remote   │              │
│  │ Server   │ │ Forward  │ │ Forward  │              │
│  │ :1080    │ │ -L spec  │ │ -R spec  │              │
│  └────┬─────┘ └────┬─────┘ └────┬─────┘              │
│       │             │             │                     │
│       ▼             ▼             ▼                     │
│  ┌─────────────────────────────────┐                   │
│  │        Channel Manager          │                   │
│  │  (opens direct-tcpip,           │                   │
│  │   forwarded-tcpip streams)      │                   │
│  └──────────────┬──────────────────┘                   │
│                  │                                      │
│  ┌──────────────▼──────────────────┐                   │
│  │       SSH Client (russh)        │                   │
│  │   Handle<ClientHandler>         │                   │
│  └──────────────┬──────────────────┘                   │
│                  │                                      │
│  ┌──────────────▼──────────────────┐                   │
│  │         Transport                │                   │
│  │   (Tcp / Tls / Iroh)            │                   │
│  └──────────────────────────────────┘                   │
└────────────────────────────────────────────────────────┘
```

### SOCKS5 Server

The primary client interface. Listens on a local port (default `127.0.0.1:1080`), accepts SOCKS5 connections, and for each connection:

1. Reads the SOCKS5 handshake (auth method negotiation, target address)
2. Opens a `channel_open_direct_tcpip(target_host, target_port, originator_addr, originator_port)` on the SSH session
3. Converts the SSH channel to a stream via `channel.into_stream()`
4. Runs `tokio::io::copy_bidirectional(&mut local_socket, &mut ssh_stream)` to proxy data

Supports SOCKS5h (domain names resolved server-side) by default. This prevents DNS leaks — the client never resolves target hostnames locally, sending them to the server for resolution instead. This is consistent with the project's privacy design (ADR-006).

### Port Forwarding

Local port forwards (`-L local_addr:local_port:remote_host:remote_port`):

1. Bind `TcpListener` on `local_addr:local_port`
2. For each accepted connection, open `channel_open_direct_tcpip(remote_host, remote_port, ...)`
3. Proxy bytes bidirectionally via `copy_bidirectional`

Remote port forwards (`-R remote_addr:remote_port:local_host:local_port`):

1. Send `tcpip_forward(remote_addr, remote_port)` to request the server listen on a port
2. When the handler receives `server_channel_open_forwarded_tcpip`, connect to `local_host:local_port`
3. Proxy bytes bidirectionally

### Channel Manager

The channel manager owns the `Arc<client::Handle<ClientHandler>>` and provides methods:

- `open_direct_tcpip(host, port)` — open a tunnel channel to a remote host
- `open_streamlocal(socket_path)` — open a tunnel to a Unix socket
- `request_tcpip_forward(addr, port)` — request remote listening
- `cancel_tcpip_forward(addr, port)` — cancel remote listening

It also handles reconnection: if `handle.is_closed()` returns true, attempt reconnection with exponential backoff.

### Reconnection

On transport failure:

1. Detect via `handle.is_closed()` or transport read error
2. Exponential backoff reconnect (1s, 2s, 4s, ... max 30s)
3. Re-establish transport connection
4. Re-authenticate SSH session
5. Notify SOCKS5 server and port forwards (in-flight connections fail, new connections work)

Reconnection is always enabled. The backoff caps at 30 seconds and continues indefinitely until the user terminates the process. Existing TCP connections through the tunnel are lost on reconnect — this is acceptable and consistent with how VPN connections behave.

The channel manager orchestrates reconnection: it creates a new transport stream (by calling `transport.connect()` again) and establishes a new SSH session over it (ADR-004). This is a full reconnect — there is no "SSH reconnects over the same transport." Port forward listeners (`-L`, `-R`) are re-registered with the new session after reconnection.

### Programmatic Configuration (ADR-011)

The client uses programmatic configuration — no `~/.ssh/config` parsing, no custom config files. Configuration comes from:

1. **CLI flags**: `--server`, `--identity`, `--transport`, etc.
2. **Library API**: `ConnectOptions` and `ServeOptions` structs in `alknet-core`, constructable programmatically
3. **Environment variables**: `ALKNET_SERVER`, `ALKNET_IDENTITY` as convenience defaults

This approach avoids cross-platform path issues (`~` expansion, Windows `USERPROFILE`) and makes the library API clean for programmatic consumers like the NAPI wrapper. Keys can be provided as file paths or in-memory data.

### Key Material Format

Key inputs (`--identity`, `--authorized-keys`, `--cert-authority`, `--key`) accept either:

- **File path**: A filesystem path to a key file (e.g., `~/.ssh/id_ed25519`, `/etc/alknet/ca.pub`)
- **In-memory data**: Raw key bytes provided programmatically via the library API or NAPI wrapper (as `Vec<u8>` in Rust, `Buffer` in Node.js)

The accepted format is **OpenSSH key format** (the format used by `ssh-keygen` and OpenSSH's `~/.ssh/` files). This includes:
- Private keys: OpenSSH format (begins with `-----BEGIN OPENSSH PRIVATE KEY-----`)
- Public keys: OpenSSH format (e.g., `ssh-ed25519 AAAA... user@host`)
- Certificate authority keys: OpenSSH public key format
- Authorized keys files: Standard OpenSSH `authorized_keys` format

PEM-encoded keys (PKCS#1, PKCS#8) are not supported. Use OpenSSH format keys throughout.

### CLI Interface

```bash
# Basic connection (TCP, default port 22)
alknet connect --server example.com --identity ~/.ssh/id_ed25519

# With TLS
alknet connect --server example.com:443 --transport tls --identity ~/.ssh/id_ed25519

# With TLS + insecure (self-signed certs)
alknet connect --server example.com:443 --transport tls --identity ~/.ssh/id_ed25519 --insecure

# With iroh (no public IP needed)
alknet connect --peer <endpoint-id> --transport iroh --identity ~/.ssh/id_ed25519

# With iroh + custom relay
alknet connect --peer <endpoint-id> --transport iroh --identity ~/.ssh/id_ed25519 --iroh-relay https://relay.example.com

# With iroh + proxy (transport chaining)
alknet connect --peer <endpoint-id> --transport iroh --identity ~/.ssh/id_ed25519 --proxy socks5://127.0.0.1:1080

# SOCKS5 on custom port
alknet connect --server example.com --socks5 127.0.0.1:1080 --identity ~/.ssh/id_ed25519

# With port forwards
alknet connect --server example.com --forward 5432:db.internal:5432 --forward 6379:redis.internal:6379

# All options
alknet connect \
  --server <addr> \          # TCP/TLS server address (required for tcp/tls)
  --peer <endpoint-id> \    # iroh endpoint ID, base58-encoded (required for iroh)
  --transport tcp|tls|iroh \ # Transport mode
  --identity <path-or-buffer> \ # SSH private key (path or in-memory)
  --socks5 <addr:port> \    # SOCKS5 listen address (default: 127.0.0.1:1080)
  --forward <spec> \        # Port forward spec (repeatable)
  --remote-forward <spec> \ # Remote port forward spec (repeatable)
  --proxy <url> \            # Upstream proxy (socks5:// or http://)
  --iroh-relay <url> \      # iroh relay URL (default: n0 relay)
  --tls-server-name <host> \ # SNI hostname for TLS
  --insecure                 # Accept self-signed TLS certs
```

## Constraints

- SOCKS5 is always enabled when `alknet connect` runs (it's the primary interface). Port forwards are optional.
- The client does not log tunnel destinations. The SOCKS5 server connects and proxies — no logging of SOCKS5 request targets.
- Authentication is Ed25519 public key or OpenSSH certificate (ADR-012). No password authentication over SSH.
- Only one SSH session per `alknet connect` process. Multiple sessions = multiple processes (or a future multiplexer).
- No `~/.ssh/config` parsing. Configuration is programmatic via CLI flags, env vars, or library API structs (ADR-011).
- VPN-like "route all traffic" behavior is provided by running `tun2proxy --proxy socks5://127.0.0.1:1080` alongside the client, not by a built-in TUN interface (ADR-014).
- The CLI `alknet connect` command manages a full SSH session with SOCKS5 and port forwarding. The NAPI `connect()` function is a different operation — it opens a single SSH channel as a Duplex stream for programmatic use, with no SOCKS5 server or port forwarding. See napi-and-pubsub.md for details.

## Graceful Shutdown

On SIGTERM or SIGINT:

1. Stop accepting new SOCKS5 connections and port forward connections
2. Send an SSH disconnect message to the server
3. Wait for in-flight channel data to drain (brief timeout, ~2 seconds)
4. Close the transport stream
5. Exit

In-flight connections are not preserved across shutdown — they receive a connection reset. This matches the behavior of standard SSH tunnel tools.

## Error Handling

Error handling follows the project's layered pattern (see overview.md):

- **Transport errors**: Trigger reconnection with exponential backoff (see Reconnection section above). If reconnection fails indefinitely, the process continues retrying until the user terminates it.
- **Auth errors**: Cause reconnection retry. After repeated auth failures, the SOCKS5 server and port-forward listeners remain active but new channel opens fail until reconnection succeeds.
- **Channel-level errors**: Individual channel failures (target unreachable, proxy failure) close that channel without affecting the SSH session or other channels.
- **CLI errors**: Reported to stderr with a non-zero exit code. Fatal errors (invalid flags, key file not found) exit immediately.

## Open Questions

None — all resolved.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [005](decisions/005-socks5-before-tun.md) | SOCKS5 first | SOCKS5 is the primary interface; TUN is external (tun2proxy) |
| [006](decisions/006-no-logging-of-tunnel-destinations.md) | No logging of destinations | Client does not log SOCKS5 request targets (consistent with ADR-006) |
| [011](decisions/011-no-ssh-config-programmatic-api.md) | Programmatic-first API | No file-based config; options are structs, env vars, or CLI flags |
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Key + cert-authority | No password auth; OpenSSH cert-authority for multi-user |
| [019](decisions/019-proxy-dual-semantics.md) | Proxy dual semantics | `--proxy` routes transport on client, data on server |
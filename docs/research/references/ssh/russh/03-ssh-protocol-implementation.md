# Russh: SSH Protocol Implementation

This document covers how russh implements the SSH-2 protocol — key exchange, authentication, channel multiplexing, port forwarding, and extensions.

## Protocol Message Constants (`msg` module)

Russh defines SSH protocol message type codes as constants:

| Constant | Value | RFC Reference |
|----------|-------|---------------|
| `DISCONNECT` | 1 | RFC 4253 §11.1 |
| `IGNORE` | 2 | RFC 4253 §11.2 |
| `UNIMPLEMENTED` | 3 | RFC 4253 §11.3 |
| `DEBUG` | 4 | RFC 4253 §11.4 |
| `SERVICE_REQUEST` | 5 | RFC 4253 §7 |
| `SERVICE_ACCEPT` | 6 | RFC 4253 §7 |
| `EXT_INFO` | 7 | RFC 8308 |
| `KEXINIT` | 20 | RFC 4253 §7.1 |
| `NEWKEYS` | 21 | RFC 4253 §7.3 |
| `KEX_ECDH_INIT` | 30 | RFC 5656 §4 |
| `KEX_ECDH_REPLY` | 31 | RFC 5656 §4 |
| `KEX_DH_GEX_REQUEST` | 34 | RFC 4419 §3 |
| `KEX_DH_GEX_GROUP` | 31 | RFC 4419 §3 |
| `KEX_DH_GEX_INIT` | 32 | RFC 4419 §3 |
| `KEX_DH_GEX_REPLY` | 33 | RFC 4419 §3 |
| `USERAUTH_REQUEST` | 50 | RFC 4252 §5 |
| `USERAUTH_FAILURE` | 51 | RFC 4252 §5 |
| `USERAUTH_SUCCESS` | 52 | RFC 4252 §5 |
| `USERAUTH_BANNER` | 53 | RFC 4252 §5.4 |
| `USERAUTH_INFO_REQUEST` | 60 | RFC 4256 §5 |
| `USERAUTH_INFO_RESPONSE` | 61 | RFC 4256 §5 |
| `GLOBAL_REQUEST` | 80 | RFC 4254 §4 |
| `REQUEST_SUCCESS` | 81 | RFC 4254 §4 |
| `REQUEST_FAILURE` | 82 | RFC 4254 §4 |
| `CHANNEL_OPEN` | 90 | RFC 4254 §5.1 |
| `CHANNEL_OPEN_CONFIRMATION` | 91 | RFC 4254 §5.1 |
| `CHANNEL_OPEN_FAILURE` | 92 | RFC 4254 §5.1 |
| `CHANNEL_WINDOW_ADJUST` | 93 | RFC 4254 §5.2 |
| `CHANNEL_DATA` | 94 | RFC 4254 §5.2 |
| `CHANNEL_EXTENDED_DATA` | 95 | RFC 4254 §5.2 |
| `CHANNEL_EOF` | 96 | RFC 4254 §5.3 |
| `CHANNEL_CLOSE` | 97 | RFC 4254 §5.3 |
| `CHANNEL_REQUEST` | 98 | RFC 4254 §5.4 |
| `CHANNEL_SUCCESS` | 99 | RFC 4254 §5.4 |
| `CHANNEL_FAILURE` | 100 | RFC 4254 §5.4 |

---

## Connection Lifecycle

### 1. Version Exchange

Both sides send their SSH identification string:

```
SSH-2.0-russh_0.60.2\r\n
```

The `SshId` type handles this:

```rust
pub enum SshId {
    Standard(Cow<'static, str>),  // Appends \r\n
    Raw(Cow<'static, str>),        // Sent as-is
}
```

Client sends first, then reads server's ID. Server reads client's ID first, then sends its own.

### 2. Key Exchange (KEXINIT)

The `negotiation` module handles algorithm negotiation. Each side sends a `KEXINIT` packet containing lists of supported algorithms. The first algorithm from the client's list that also appears in the server's list is chosen (for client), or vice versa (for server).

**Algorithm selection order** (per `Preferred::DEFAULT`):
- **KEX**: `mlkem768x25519-sha256` → `curve25519-sha256` → `curve25519-sha256@libssh.org` → `diffie-hellman-group-exchange-sha256` → group18/17/16/15/14
- **Host Key**: `ssh-ed25519` → `ecdsa-sha2-nistp256/384/521` → `rsa-sha2-512/256` → `ssh-rsa`
- **Cipher**: `chacha20-poly1305@openssh.com` → `aes256-gcm@openssh.com` → `aes256-ctr` → `aes192-ctr` → `aes128-ctr`
- **MAC**: `hmac-sha2-512-etm@openssh.com` → `hmac-sha2-256-etm@openssh.com` → `hmac-sha2-512` → `hmac-sha2-256`

**Negotiation flow** (`write_kex` / `read_kex`):
1. Both sides send KEXINIT with their algorithm lists
2. Each side parses the other's KEXINIT using `Client::read_kex()` or `Server::read_kex()`
3. The `Select` trait implements the first-match algorithm
4. `Names` struct is produced with the negotiated algorithms
5. Extension kex names (like `ext-info-c`, `kex-strict-c-v00@openssh.com`) are handled specially — filtered from negotiation but used for capability detection

### 3. Diffie-Hellman Key Exchange

After KEXINIT, the actual key exchange runs. This is managed by `ClientKex` and `ServerKex` state machines.

**Client KEX state machine** (`ClientKex`):
```
Created → (receive KEXINIT) → WaitingForDhReply or WaitingForGexReply → WaitingForNewKeys → Done
```

Steps for a typical Curve25519 exchange:
1. **Client sends KEX_ECDH_INIT**: Contains client's ephemeral public key
2. **Server sends KEX_ECDH_REPLY**: Contains server host key, server's ephemeral public key, and a signature
3. **Client verifies**:
   - Parses the server host key
   - Computes the shared secret from the ephemeral keys
   - Computes the exchange hash (H) from: `V_C || V_S || I_C || I_S || K_S || e || f || K`
   - Verifies the server's signature on H using the server host key
   - Calls `handler.check_server_key()` for application-level trust verification
4. **Both sides send NEWKEYS**: Activate the newly computed encryption keys

**Server KEX** mirrors this, responding to client's init and sending the reply.

**DH-GEX (Group Exchange) flow** has an extra step:
1. Client sends `KEX_DH_GEX_REQUEST` with min/preferred/max group sizes
2. Server sends `KEX_DH_GEX_GROUP` with the prime and generator
3. Client validates group size and sends `KEX_DH_GEX_INIT`
4. Server sends `KEX_DH_GEX_REPLY`
5. Both send `NEWKEYS`

### 4. Key Derivation (`compute_keys`)

After the shared secret and exchange hash are computed, keys are derived per RFC 4253 §7.2:

```
Key = HASH(K || H || X || session_id)
```

Where `X` is a single character:
- `A` = client-to-server IV (nonce)
- `B` = server-to-client IV (nonce)  
- `C` = client-to-server encryption key
- `D` = server-to-client encryption key
- `E` = client-to-server MAC key
- `F` = server-to-client MAC key

If the derived key is shorter than needed, it's extended by hashing `K || H || Key[0..n]` repeatedly.

The `session_id` is set to the exchange hash from the first key exchange and remains constant across rekeys.

### 5. Strict Key Exchange

Russh supports the OpenSSH strict key exchange extension (`kex-strict-c-v00@openssh.com` / `kex-strict-s-v00@openssh.com`), which:
- Resets sequence numbers after NEWKEYS
- Enforces that only KEXINIT, DH init, and NEWKEYS messages appear during initial kex
- Validated by `validate_client_msg_strict_kex` / `validate_server_msg_strict_kex`
- Prevents the Terrapin attack (CVE-2023-48799)

### 6. Authentication

After NEWKEYS, the encrypted state machine transitions through:

```rust
pub enum EncryptedState {
    WaitingAuthServiceRequest { sent: bool, accepted: bool },
    WaitingAuthRequest(auth::AuthRequest),
    InitCompression,
    Authenticated,
}
```

Flow:
1. Client sends `SERVICE_REQUEST` for "ssh-connection"
2. Server responds with `SERVICE_ACCEPT`
3. Client sends authentication requests (password, publickey, keyboard-interactive, none)
4. Server responds with `USERAUTH_SUCCESS`, `USERAUTH_FAILURE`, or `USERAUTH_INFO_REQUEST`
5. Upon success, state transitions to `InitCompression` → `Authenticated`

**Authentication methods** (`auth::Method`):
```rust
pub enum Method {
    None,
    Password { password: String },
    PublicKey { key: PrivateKeyWithHashAlg },
    OpenSshCertificate { key: Arc<PrivateKey>, cert: Certificate },
    FuturePublicKey { key: PublicKey, hash_alg: Option<HashAlg> },
    FutureCertificate { cert: Certificate, hash_alg: Option<HashAlg> },
    KeyboardInteractive { submethods: String },
}
```

The `FuturePublicKey` / `FutureCertificate` variants are used with the `Signer` trait (e.g., SSH agent), where signing is delegated to an external component.

### 7. Rekeying

Rekeying is triggered automatically when:
- Bytes written exceed `Limits::rekey_write_limit` (default 1 GB)
- Bytes read exceed `Limits::rekey_read_limit` (default 1 GB)  
- Time since last rekey exceeds `Limits::rekey_time_limit` (default 1 hour)
- Explicitly via `Handle::rekey_soon()`

During rekey:
- New `KEXINIT` packets are exchanged
- Pending channel data is buffered until rekey completes
- Sequence numbers are reset if strict kex is negotiated

---

## Channel Multiplexing

SSH channels are multiplexed over a single connection. Each channel has:
- A `ChannelId` (local identifier)
- A `recipient_channel` (remote identifier)
- Window size for flow control
- Maximum packet size

### Channel Types

| Type | Open Method | Description |
|------|-------------|-------------|
| `session` | `channel_open_session()` | Standard shell/exec channel |
| `x11` | `channel_open_x11()` | X11 forwarding |
| `direct-tcpip` | `channel_open_direct_tcpip()` | Local TCP port forwarding |
| `forwarded-tcpip` | Server-initiated | Remote TCP port forwarding |
| `direct-streamlocal` | `channel_open_direct_streamlocal()` | Local UNIX socket forwarding |
| `forwarded-streamlocal` | Server-initiated | Remote UNIX socket forwarding |
| `auth-agent@openssh.com` | Server-initiated | Agent forwarding |

### Flow Control

SSH uses a window-based flow control mechanism:
- Each side maintains a `sender_window_size` and `recipient_window_size`
- Data cannot be sent if the recipient's window is exhausted
- `CHANNEL_WINDOW_ADJUST` messages increase the window
- `adjust_window()` is called when data is received to determine the next target window
- Data is queued in `pending_data` when the window is full, and flushed when the window is adjusted

### Channel Request/Response Pattern

Many channel operations follow a request-response pattern:
1. Client sends `CHANNEL_REQUEST` with `want_reply = true`
2. Server processes the request via `Handler` callback
3. Server calls `session.channel_success(channel)` or `session.channel_failure(channel)`
4. Client receives `CHANNEL_SUCCESS` or `CHANNEL_FAILURE`

---

## Port Forwarding

### Local TCP Forwarding (direct-tcpip)

Client opens a channel to forward a TCP connection through the server:

```rust
// Client side
let channel = handle.channel_open_direct_tcpip(
    "target-host", 8080,  // host:port to connect to
    "origin-host", 12345, // originator info
).await?;
```

### Remote TCP Forwarding (forward-tcpip)

**Step 1**: Client requests the server to listen on a port:
```rust
let allocated_port = handle.tcpip_forward("0.0.0.0", 0).await?;
```

**Step 2**: When a connection arrives, the server calls `Handler::channel_open_forwarded_tcpip()`.

**Step 3**: Client receives the channel via `Handler::server_channel_open_forwarded_tcpip()`.

**Step 4**: To stop:
```rust
handle.cancel_tcpip_forward("0.0.0.0", allocated_port).await?;
```

### UNIX Socket Forwarding (streamlocal)

Similar to TCP forwarding but for UNIX domain sockets:

```rust
// Local
handle.channel_open_direct_streamlocal("/tmp/socket").await?;

// Remote
handle.streamlocal_forward("/tmp/socket").await?;
handle.cancel_streamlocal_forward("/tmp/socket").await?;
```

---

## Extensions

### `ext-info-c` / `ext-info-s` (RFC 8308)

Extension info is sent after NEWKEYS. Russh supports:
- **`server-sig-algs`**: Server tells client which signature algorithms it accepts for public key authentication. Used by `Handle::best_supported_rsa_hash()`.

### `kex-strict-c-v00@openssh.com` / `kex-strict-s-v00@openssh.com`

OpenSSH strict key exchange (Terrapin attack mitigation). Negotiated during KEXINIT and enforced during the initial key exchange.

### OpenSSH Agent Forwarding

The server can open an `auth-agent@openssh.com` channel to forward the client's SSH agent. The client handles this via `Handler::server_channel_open_agent_forward()`.

### OpenSSH `no-more-sessions@openssh.com`

Requests that the other side not open any more sessions:
```rust
handle.no_more_sessions(true).await?;
```
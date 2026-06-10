# Russh: Overview & Architecture

**Version**: 0.60.2  
**Repository**: https://github.com/warp-tech/russh  
**License**: Apache-2.0  
**Rust Edition**: 2024  
**MSRV**: 1.85  
**Origin**: Fork of [Thrussh](https://nest.pijul.com/pijul/thrussh) by Pierre-Étienne Meunier

## What is Russh?

Russh is a **low-level, asynchronous SSH2 client and server implementation** for Rust, built on Tokio and Futures. It provides a complete SSH-2 protocol stack — key exchange, authentication, channel multiplexing, port forwarding, and subsystems — exposed through handler traits that users implement for their specific needs.

Core characteristics:
- **Async-native** — built entirely on Tokio and Futures; no blocking I/O
- **Handler-driven** — both client and server are used by implementing trait handlers (`client::Handler`, `server::Handler`)
- **Both client and server** — a single crate supports both sides of the SSH connection
- **Streaming I/O** — channels implement `AsyncRead`/`AsyncWrite` for ergonomic integration
- **Safety-conscious** — `deny(clippy::unwrap_used)`, `deny(clippy::expect_used)`, `deny(clippy::panic)`, `deny(clippy::indexing_slicing)` by default; sensitive data uses `CryptoVec` with `mlock`
- **Two crypto backends** — `aws-lc-rs` (default) or `ring`, at least one required

## Workspace Structure

```
russh/                 # Main SSH library crate
├── russh-util/        # Runtime abstraction utilities (WASM support, time, spawn)
├── russh-config/      # SSH config file parser + ProxyCommand support
├── cryptovec/         # Zeroing-on-drop vector with mlock (CryptoVec)
└── pageant/           # Windows Pageant SSH agent transport client
```

### Dependency Graph

```
russh depends on:
  ├── russh-cryptovec    (CryptoVec: zeroing vector with mlock, ssh-encoding support)
  ├── russh-util         (runtime abstraction: tokio spawn, time, WASM compat)
  ├── ssh-key            (internal-russh-forked-ssh-key: key types, parsing, certificates)
  ├── ssh-encoding       (SSH wire format encode/decode)
  ├── tokio              (async runtime, IO, sync, time)
  ├── futures            (future combinators)
  ├── curve25519-dalek   (Curve25519 DH)
  ├── ed25519-dalek      (Ed25519 signing)
  ├── p256/p384/p521     (NIST ECDSA curves + ECDH)
  ├── aes                (AES cipher implementations)
  ├── flate2             (zlib compression, optional)
  ├── rsa                (RSA signing, optional feature)
  └── aws-lc-rs / ring   (crypto backend for GCM etc.)

russh-config depends on:
  ├── tokio              (async IO, process for ProxyCommand)
  ├── futures            (future utilities)
  └── globset / whoami   (config matching, username detection)

cryptovec depends on:
  ├── nix (Unix)         (mlock/munlock via mmap)
  ├── windows-sys (Win)  (VirtualLock/VirtualUnlock)
  └── ssh-encoding       (optional, for Encode support)
```

## Architecture Overview

Russh implements the SSH-2 protocol as a **state machine** driven by an event loop. The core design separates protocol handling from application logic via handler traits.

```
┌─────────────────────────────────────────────────────────┐
│                    Application Layer                      │
│  ┌──────────────────┐      ┌──────────────────┐         │
│  │ client::Handler   │      │ server::Handler   │         │
│  │ (user implements) │      │ (user implements) │         │
│  └────────┬─────────┘      └────────┬─────────┘         │
├───────────┼──────────────────────────┼───────────────────┤
│           │      Russh Library       │                    │
│  ┌────────▼─────────────────────────▼─────────┐         │
│  │              Event Loop (tokio::select!)       │         │
│  │  ┌──────────┐ ┌─────────┐ ┌──────────────┐  │         │
│  │  │ Reading  │ │ Writing │ │ Handler      │  │         │
│  │  │ packets  │ │ packets │ │ dispatch     │  │         │
│  │  └────┬─────┘ └────┬────┘ └──────┬───────┘  │         │
│  │       │            │             │           │         │
│  │  ┌────▼────────────▼─────────────▼───────┐   │         │
│  │  │          Session State Machine        │   │         │
│  │  │  KEX → Auth → Channels → Forwarding   │   │         │
│  │  └──────────────────────────────────────┘   │         │
│  └─────────────────────────────────────────────┘         │
├──────────────────────────────────────────────────────────┤
│                     Transport Layer                       │
│  ┌────────────────┐  ┌────────────┐  ┌────────────┐     │
│  │ Cipher (enc)   │  │ MAC (auth) │  │ Compression │     │
│  └────────────────┘  └────────────┘  └────────────┘     │
│  ┌────────────────┐  ┌────────────────────────────┐      │
│  │ PacketWriter   │  │ SSHBuffer / SshRead        │      │
│  └────────────────┘  └────────────────────────────┘      │
├──────────────────────────────────────────────────────────┤
│                   TCP Stream (tokio::net)                  │
└──────────────────────────────────────────────────────────┘
```

### Key Design Principle: Buffered Writes

From the library documentation:

> It might seem a little odd that the read/write methods for server or client sessions often return neither `Result` nor `Future`. This is because the data sent to the remote side is **buffered**, because it needs to be encrypted first, and encryption works on buffers, and for many algorithms, not in place.

The event loop works as follows:
1. Wait for incoming packets
2. React by calling the provided `Handler`, which fills some buffers
3. If buffers are non-empty, send them to the socket, flush, empty the buffers
4. For servers, unsolicited messages sent through a `server::Handle` are processed when there is no incoming packet

## Module Map

| Module | Purpose |
|--------|---------|
| `client/` | Client-side SSH: `Handler` trait, `Session`, `Handle`, `Config`, kex, encrypted state |
| `server/` | Server-side SSH: `Handler` trait, `Server` trait, `Session`, `Handle`, `Config` |
| `kex/` | Key exchange algorithms: Curve25519, DH groups, ECDH-NIST, ML-KEM hybrid |
| `cipher/` | Encryption: Chacha20-Poly1305, AES-GCM, AES-CTR, AES-CBC, 3DES-CBC |
| `mac/` | Message authentication: HMAC-SHA2, ETM variants |
| `keys/` | Key loading, parsing, SSH agent protocol, known hosts |
| `channels/` | Channel abstraction, `Channel`/`ChannelMsg`, `AsyncRead`/`AsyncWrite` streams |
| `negotiation` | Algorithm negotiation: `Preferred`, `Names`, `write_kex`, `read_kex` |
| `compression` | zlib and zlib@openssh.com compression |
| `session` | `CommonSession`, `Encrypted` state, `NewKeys`, `Exchange` |
| `sshbuffer` | `SSHBuffer`, `PacketWriter`, `SshId` |
| `auth` | Auth methods: `Method`, `MethodSet`, `Signer`, `AuthResult` |
| `cert` | OpenSSH certificate handling |
| `pty` | PTY terminal modes |
| `msg` | SSH message type constants (RFC 4250/4253/4254) |
| `parsing` | Wire format parsing helpers |

## Crypto Backend Selection

At least one crypto backend feature must be enabled, or compilation fails:

```rust
#[cfg(not(any(feature = "ring", feature = "aws-lc-rs")))]
compile_error!(
    "`russh` requires enabling either the `ring` or `aws-lc-rs` feature as a crypto backend."
);
```

- **`aws-lc-rs`** (default): Used for AES-GCM via the `aws-lc-rs` crate's AEAD interface
- **`ring`**: Alternative backend for AES-GCM via the `ring` crate's AEAD interface

Other features:
- **`rsa`** (default): Enables RSA key support
- **`des`**: Enables insecure 3DES-CBC cipher
- **`dsa`**: Enables insecure DSA key support
- **`flate2`** (default): Enables zlib compression
- **`async-trait`**: Enables `#[async_trait]` attribute on handler traits
- **`legacy-ed25519-pkcs8-parser`**: Enables ASN1-based Ed25519 PKCS#8 parsing
- **`serde`**: Enables serde support for ssh-key types
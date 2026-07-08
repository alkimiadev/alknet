# Transport Generalization — from_stream + iroh 1.0 + dead irpc

**Status:** Findings complete. Three-step clean sweep planned; not yet applied. Two free wins (dead `irpc` dep, iroh 0.35→1.0 migration) and one structural unlock (`Connection::from_stream` — a generic single-stream connection kind that makes every `ProtocolHandler` work over TCP+TLS, SSH channels, WebTransport streams, and wasm without handler code changes).
**Date:** 2026-07-08
**Scope:** Resolves the QUIC-welding problem in `alknet-core/src/types.rs` that blocks `api.alk.dev` (needs standard TCP+TLS HTTP) and `alknet-ssh` (needs per-channel dispatch over russh). Incorporates the iroh 1.0 migration from `../iroh-update/findings.md` so the workspace lands on a single consistent iroh version in the same sweep. Traces the exact `Connection` / `SendStream` / `RecvStream` edits and the downstream test call-site renames so the sweep is one concrete PR series, not a TODO.

---

## TL;DR

1. **`Connection`, `SendStream`, and `RecvStream` are welded to QUIC.** `ConnectionKind::{Quinn, Iroh, Mock}` — both real variants are QUIC. `HttpAdapter::handle` (`alknet-http/src/server/adapter.rs:232`) calls `connection.accept_bi().await` to get one bidi stream and serves HTTP over it. That's "HTTP over QUIC," not HTTP over TCP+TLS. There is no way to serve `api.alk.dev`'s required standard HTTP interface without either bypassing the `HandlerRegistry` (a parallel listener) or generalizing `Connection` to accept a non-QUIC stream.

2. **The generalization is small and additive.** Add `ConnectionKind::Stream` — a single read/write pair behind a `Mutex<Option<...>>` that `accept_bi` yields once (then `ConnectionClosed`). Add `Connection::from_stream(send, recv, alpn, remote_addr)` and `Connection::from_bidi(stream, alpn, remote_addr)`. Rename the existing stream-level `Mock` variants to `Stream` (they're already `Box<dyn AsyncRead/Write>` — the wrong name; `from_mock` → `from_stream`). ~60 lines added to `types.rs`, no new deps, no `Cargo.toml` change. Every existing `ProtocolHandler` (`HttpAdapter`, `TtyAdapter`, call handler) works over the new kind **unchanged**.

3. **The yield-once contract composes.** `accept_bi` on QUIC returns a new stream per call (many). `accept_bi` on `Stream` returns the underlying stream on the first call, then `ConnectionClosed`. Handlers that loop `accept_bi` (tty) get one session per single-stream connection; handlers that call once (http) get the stream directly. Both correct, no branching on transport. This is the contract that makes SSH channels, TCP+TLS, WebTransport streams, and wasm streams all look the same to handlers.

4. **`alknet-ssh` falls out for free.** SSH is a `ProtocolHandler` on `alknet/ssh` that parses SSH bytes via russh and dispatches channels — exactly the pattern `TtyAdapter` already establishes (`alknet-tty/src/adapter.rs:120-133` loops `accept_bi`, dispatches each stream to `drive_session`). The SSH handler parses channels, wraps each channel's read/write as a `Connection` via `from_stream`, and dispatches through the **same** `HandlerRegistry` by channel-type string (channel-type == ALPN string). russh is a byte parser — it doesn't care whether the bytes come from QUIC, TCP+TLS, or wasm. Zero core changes beyond `from_stream`.

5. **The iroh 0.35→1.0.2 migration is a prerequisite for `alknet-blobs` and fits in the same sweep.** `alknet-core`'s `iroh 0.35` dep is load-bearing (`endpoint.rs:676-704` uses ~15 iroh APIs; 3 broke between 0.35 and 1.0). The migration is 3 edits in `build_iroh_endpoint` + a `Cargo.toml` bump. The dead `irpc 0.16` workspace dep (`Cargo.toml:19-20`, never imported by any `.rs` file) dissolves the other half of the version gap with zero code impact. Both are standalone commits that should land before `from_stream` so the workspace is on a single consistent iroh version when the structural change lands.

6. **No trait shape change.** `ProtocolHandler::handle(&self, connection: Connection, &AuthContext)` stays. The multiplexing loop stays in the handler (as `TtyAdapter` already does). The endpoint does not own the loop. SSH's per-channel dispatch happens *inside* the SSH handler, not in the endpoint — same as tty's per-stream dispatch happens inside the tty handler.

---

## 1. The QUIC-welding problem

### 1.1 `Connection` is QUIC-only

`crates/alknet-core/src/types.rs:362-368`:

```rust
enum ConnectionKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::Connection),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::Connection),
    Mock(Arc<dyn MockConnection + Send + Sync>),
}
```

Both real variants are QUIC. `Mock` is a test-only trait stub whose `accept_bi` returns `StreamClosed` (`types.rs:430`) — it cannot serve a real stream.

`SendStreamKind` / `RecvStreamKind` (`types.rs:228-242`) are the same shape — `Quinn` / `Iroh` / `Mock`, where `Mock` is `Box<dyn AsyncWrite>` / `Box<dyn AsyncRead>`. The stream-level `Mock` variants *are* generic stream holders, just misnamed. They're the seed of the generalization.

### 1.2 `HttpAdapter` inherits the welding

`crates/alknet-http/src/server/adapter.rs:227-238`:

```rust
async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
    if let Some(identity) = auth.identity.clone() {
        let _ = connection.set_identity(identity);
    }
    let (send, recv) = connection
        .accept_bi()
        .await
        .map_err(stream_error_to_handler)?;
    let io = QuicStream::new(send, recv);
    self.serve_io(io).await
}
```

`serve_io` (`adapter.rs:241-264`) is already generic over `AsyncRead + AsyncWrite + Send + Unpin + 'static` — the only QUIC-coupled line in the whole HTTP adapter is `connection.accept_bi().await`. `QuicStream` (`adapter.rs:271-314`) just forwards `poll_read`/`poll_write`/`poll_flush`/`poll_shutdown` to the `SendStream`/`RecvStream` pair. It's a thin adapter, not a QUIC dependency.

So `HttpAdapter` is one `accept_bi` call away from being transport-agnostic. The problem is that `accept_bi` only works when the `Connection` is QUIC-kind.

### 1.3 `api.alk.dev` needs standard HTTP

The `api.alk.dev` hub requires:
- A standard TCP+TLS HTTP interface (h2/http1.1) for browser clients, the OpenAI-compatible proxy, the streamable MCP endpoint, and WebSocket upgrade
- A QUIC endpoint for worker/spoke connections (vast.ai GPU instances registering with per-instance tokens, proxying Ollama cloud access without leaking keys)
- Eventually, SSH channel access over TCP/TLS/QUIC for remote terminal sessions

The first bullet is blocked today. The reverse-proxy at `/workspace/@alkdev/reverse-proxy/src/server.rs:56-136` (`serve_https_listener`) is the exact template for what's needed: `tokio_rustls::TlsAcceptor` → check `alpn_protocol()` → `hyper::server::conn::http2::Builder` or `hyper_util::server::conn::auto::Builder` over `TokioIo::new(tls_stream)`. But wiring it through the `HandlerRegistry`/ALPN router (so it's not a parallel listener bypassing the core) requires `Connection` to accept a `TlsStream<TcpStream>`.

### 1.4 `alknet-ssh` is blocked on the same thing

`alknet-tty` already shows the pattern. `TtyAdapter::handle` (`alknet-tty/src/adapter.rs:116-135`):

```rust
async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
    if let Some(identity) = auth.identity.clone() {
        let _ = connection.set_identity(identity);
    }
    loop {
        let (send, recv) = match connection.accept_bi().await {
            Ok(pair) => pair,
            Err(StreamError::ConnectionClosed) => break,
            Err(StreamError::StreamClosed) => break,
            Err(e) => return Err(HandlerError::from(e)),
        };
        let backends = self.backends.clone();
        let ownership = self.ownership.clone();
        let identity = auth.identity.clone();
        tokio::spawn(async move {
            let _ = drive_session(send, recv, backends, ownership, identity).await;
        });
    }
    Ok(())
}
```

And `drive_session` (`adapter.rs:175-187`) is generic over `AsyncRead + AsyncWrite`:

```rust
pub async fn drive_session(
    client_send: impl AsyncWrite + Send + Unpin + 'static,
    client_recv: impl AsyncRead + Send + Unpin + 'static,
    backends: Arc<HashMap<String, Arc<dyn TtyBackend>>>,
    ownership: Option<Arc<dyn OwnershipProvider>>,
    identity: Option<Identity>,
)
```

The session driver is already transport-agnostic. Only `handle()` is QUIC-coupled (it loops `accept_bi`).

SSH is the same pattern, richer: `SshHandler::handle` parses SSH bytes, channels open with a type string, and each channel's read/write is handed to the service for that channel-type. If channel-type == ALPN string, the SSH handler dispatches through the same `HandlerRegistry` every other transport uses — one SSH connection carries heterogeneous channels (`alknet/tty`, `alknet/call`, `h2`, ...). This is SSH's real multiplexing power, and it's the thing QUIC doesn't give you natively (QUIC's ALPN is per-connection, so all bidi streams share one handler).

For the SSH handler to dispatch channels through `HandlerRegistry`, it needs to wrap each channel as a `Connection`. That's `from_stream`.

---

## 2. The generalization: `Connection::from_stream`

### 2.1 The new `ConnectionKind::Stream`

Add a variant that holds a single read/write pair behind a `Mutex<Option<...>>` for the yield-once semantic:

```rust
struct StreamConn {
    stream: Mutex<Option<(SendStream, RecvStream)>>,
    remote_addr: Option<SocketAddr>,
}

enum ConnectionKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::Connection),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::Connection),
    Mock(Arc<dyn MockConnection + Send + Sync>),
    Stream(StreamConn),
}
```

`StreamConn` is always available (no feature gate) — it's generic, no transport deps.

### 2.2 The constructors

```rust
impl Connection {
    /// Construct a Connection from a pre-split read/write pair.
    /// `accept_bi()` yields this pair once, then returns `ConnectionClosed`.
    /// `open_bi()` returns `StreamClosed` (a single stream can't open new streams).
    pub fn from_stream(
        send: impl AsyncWrite + Send + Unpin + 'static,
        recv: impl AsyncRead + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        Self {
            kind: ConnectionKind::Stream(StreamConn {
                stream: Mutex::new(Some((
                    SendStream::from_stream(send),
                    RecvStream::from_stream(recv),
                ))),
                remote_addr,
            }),
            alpn,
            identity: OnceLock::new(),
        }
    }

    /// Convenience for a single bidirectional stream (e.g. TlsStream<TcpStream>).
    /// Splits internally via tokio::io::split.
    pub fn from_bidi(
        stream: impl AsyncRead + AsyncWrite + Send + Unpin + 'static,
        alpn: Vec<u8>,
        remote_addr: Option<SocketAddr>,
    ) -> Self {
        let (recv, send) = tokio::io::split(stream);
        Self::from_stream(send, recv, alpn, remote_addr)
    }
}
```

Note: `tokio::io::split` returns `(ReadHalf, WriteHalf)` — the tuple is `(recv, send)`.

### 2.3 The `accept_bi` contract (made explicit)

```rust
/// Yield the next bidirectional stream this connection provides.
///
/// # Transport semantics
///
/// - **QUIC (quinn/iroh)**: returns a new bidi stream on each call.
///   `ConnectionClosed` when the underlying connection closes.
/// - **TCP+TLS / single-stream**: yields the underlying stream on the
///   first call, then `ConnectionClosed` on all subsequent calls.
///   A single transport stream cannot open new application streams.
///
/// Handlers that loop `accept_bi` (e.g. `TtyAdapter`) get one session
/// per single-stream connection; handlers that call once (e.g.
/// `HttpAdapter`) get the stream directly. Both are correct.
pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> { ... }
```

This is the contract that makes the abstraction compose: QUIC is "many streams," everything else is "one stream," and the handler code doesn't branch on which.

### 2.4 The method arms

```rust
impl Connection {
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => { /* unchanged */ }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => { /* unchanged */ }
            ConnectionKind::Mock(_) => Err(StreamError::StreamClosed),
            ConnectionKind::Stream(sc) => {
                let mut guard = sc.stream.lock().expect("stream mutex poisoned");
                match guard.take() {
                    Some(pair) => Ok(pair),
                    None => Err(StreamError::ConnectionClosed),
                }
            }
        }
    }

    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => { /* unchanged */ }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => { /* unchanged */ }
            ConnectionKind::Mock(_) => Err(StreamError::StreamClosed),
            ConnectionKind::Stream(_) => Err(StreamError::StreamClosed),
        }
    }

    pub fn remote_addr(&self) -> Option<SocketAddr> {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => Some(c.remote_address()),
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(_) => None,
            ConnectionKind::Mock(m) => m.remote_addr(),
            ConnectionKind::Stream(sc) => sc.remote_addr,
        }
    }

    pub fn close(&self, _code: u32, _reason: &str) {
        match &self.kind {
            #[cfg(feature = "quinn")]
            ConnectionKind::Quinn(c) => { /* unchanged */ }
            #[cfg(feature = "iroh")]
            ConnectionKind::Iroh(c) => { /* unchanged */ }
            ConnectionKind::Mock(m) => m.close(_code, _reason),
            ConnectionKind::Stream(sc) => {
                // Drop the stream if still held (accept_bi never called).
                // If already taken, no-op — the holder owns the lifecycle.
                let _ = sc.stream.lock().expect("stream mutex poisoned").take();
            }
        }
    }
}
```

The `code`/`reason` args to `close` are QUIC-specific (application-level close codes). For a raw stream they're ignored — the drop is the close. This is the same best-effort semantic `close` already has for `Mock`.

### 2.5 The rename: stream-level `Mock` → `Stream`

The existing `SendStreamKind::Mock(Box<dyn AsyncWrite>)` and `RecvStreamKind::Mock(Box<dyn AsyncRead>)` (`types.rs:228-242`) are already generic stream holders — the name is wrong. Rename to `Stream`:

```rust
enum SendStreamKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::SendStream),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::SendStream),
    Stream(Box<dyn AsyncWrite + Send + Unpin>),
}

enum RecvStreamKind {
    #[cfg(feature = "quinn")]
    Quinn(quinn::RecvStream),
    #[cfg(feature = "iroh")]
    Iroh(iroh::endpoint::RecvStream),
    Stream(Box<dyn AsyncRead + Send + Unpin>),
}
```

Constructors: `SendStream::from_mock` → `from_stream`, `RecvStream::from_mock` → `from_stream`. Drop the `#[allow(dead_code)]` — `from_stream` is now load-bearing (called by `Connection::from_stream`).

The `poll_*` match arms: `SendStreamKind::Mock(s)` → `SendStreamKind::Stream(s)` (3 sites in `SendStream`'s `AsyncWrite` impl at `types.rs:298-342`), `RecvStreamKind::Mock(s)` → `RecvStreamKind::Stream(s)` (1 site in `RecvStream`'s `AsyncRead` impl at `types.rs:344-360`).

The connection-level `ConnectionKind::Mock` / `MockConnection` trait (`types.rs:370-375`) is a different thing — a test-only full-connection mock. It stays untouched. Only the stream-level `Mock` variants are renamed.

### 2.6 Downstream call-site renames

The stream-level `from_mock` is used by tests in `alknet-call`:

- `crates/alknet-call/src/protocol/dispatch.rs:1200-1201, 1239-1240, 1275-1276, 1306-1307, 1366-1367` — `SendStream::from_mock` / `RecvStream::from_mock`
- `crates/alknet-call/src/protocol/adapter.rs:1213-1214, 1253-1254` — same

These are mechanical renames (`s/from_mock/from_stream/g` for the stream-level calls). The `Connection::from_mock(Arc<StubConnection>)` call sites (`dispatch.rs:483`, `connection.rs:484`, `adapter.rs:312`, `from_call.rs:461`, `call_client.rs:602`) stay unchanged — those use the connection-level `Mock` which is not renamed.

No backwards-compatibility shims. No `#[deprecated]` aliases. Clean rename — no one is using the develop branch yet, and the project that would build against it couldn't proceed because of the HTTP/TCP issue this sweep resolves.

### 2.7 What's NOT changing

- `ProtocolHandler` trait shape — unchanged. `handle(&self, connection: Connection, &AuthContext)` stays.
- `HandlerRegistry` — unchanged.
- `AlknetEndpoint` — unchanged in this commit. The TCP+TLS accept loop that *uses* `from_stream` is a separate follow-up commit.
- `ConnectionKind::Mock` / `MockConnection` trait — stays for test-only full-connection mocks.
- All handler code (`HttpAdapter`, `TtyAdapter`, call handler) — unchanged.
- No `Cargo.toml` changes — `from_stream` is pure code, no new deps. `tokio::io::split` is already available via the existing `tokio` dep.

### 2.8 What this unlocks

Every existing `ProtocolHandler` works over:

| Substrate | How `from_stream` is used | Multiplexing |
|---|---|---|
| **QUIC (quinn/iroh)** | unchanged — `ConnectionKind::Quinn`/`Iroh` | many bidi streams per connection (`accept_bi` loops) |
| **TCP+TLS** | `from_bidi(TlsStream<TcpStream>, alpn, remote_addr)` in a TLS accept loop | one stream per connection (`accept_bi` yields once) |
| **SSH channels** | SSH handler wraps each russh channel via `from_stream`, dispatches by channel-type through `HandlerRegistry` | one connection, many channels, heterogeneous handlers |
| **WebTransport streams** | WT handler wraps each WT stream via `from_stream` | one session, many streams |
| **Wasm** | `from_stream` takes any `AsyncRead + AsyncWrite`, including wasm-compatible streams | per-stream |

No handler code changes. The `accept_bi` contract is: "yield the stream(s) the transport provides, then `ConnectionClosed`." QUIC provides many; everything else provides one. Handlers that loop (tty) get one iteration; handlers that call once (http) get the stream directly. Both correct.

---

## 3. The iroh 0.35 → 1.0.2 migration (in the same sweep)

This section incorporates `../iroh-update/findings.md` by reference. The full trace is there; this section summarizes what lands in the sweep.

### 3.1 Drop the dead `irpc` dep (free, standalone commit)

`Cargo.toml:19-20` declares `irpc = "0.16"` / `irpc-derive = "0.16"` as workspace deps. No `.rs` file in the workspace imports `irpc` — `alknet-call`'s wire protocol (`src/protocol/wire.rs`) is hand-rolled length-prefixed JSON. The dep is dead.

Remove:
- `Cargo.toml:19-20` — delete `irpc` / `irpc-derive` workspace deps
- `crates/alknet-call/Cargo.toml:18` — delete `irpc = { workspace = true }`

Re-add as `0.17` when `alknet-blobs` lands (it depends on `iroh-blobs 0.103` which pulls `irpc 0.17` transitively). Zero code impact.

### 3.2 Bump `alknet-core` iroh to 1.0 (3 edits + Cargo.toml)

`crates/alknet-core/Cargo.toml:21`:

```toml
# from:
iroh = { version = "0.35", optional = true }
# to:
iroh = { version = "1.0", optional = true, default-features = false, features = ["tls-aws-lc-rs"] }
```

`default-features = false` drops `tls-ring` so `presets::Minimal` selects aws-lc-rs (matching the quinn path's existing `rustls::crypto::aws_lc_rs::default_provider()` at `endpoint.rs:551,631`).

`crates/alknet-core/src/endpoint.rs:676-704` — three edits in `build_iroh_endpoint`:

1. `iroh::Endpoint::builder()` → `iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)` — supplies the now-mandatory `crypto_provider`
2. `iroh::SecretKey::from_bytes(...)` now returns `Result<Self, KeyParsingError>` → add `?` + `.map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?`
3. `iroh::SecretKey::generate(&mut csprng)` → `iroh::SecretKey::generate()` — drop the `&mut csprng` arg and the `let mut csprng = rand::rngs::OsRng;` line

See `../iroh-update/findings.md` §4-7 for the full trace and diff.

### 3.3 Post-migration checks

- `cargo build --features iroh` — verify the 3 edits compile
- `cargo build --all-features` — verify quinn+iroh+acme together
- Check `ConnectionError` match arms in `types.rs:824-856` for exhaustiveness (new variants in 1.0 would surface as compile errors if the match is exhaustive without `_`)
- Run iroh-feature tests (`endpoint.rs:1107-1150, 1326-1364`)
- Survey `rand` usage — `endpoint.rs:687`'s `OsRng` may go dead, but `ed25519-dalek` still needs `rand_core`, so check before removing from `Cargo.toml`

---

## 4. Recommended execution order

Three commits, one sweep. Each is standalone and ships value.

### Commit 1: drop dead `irpc` dep

- Delete `irpc` / `irpc-derive` from workspace `Cargo.toml:19-20`
- Delete `irpc = { workspace = true }` from `crates/alknet-call/Cargo.toml:18`
- `cargo build` — verify nothing broke (nothing imports `irpc`)
- Standalone, free, dissolves the `irpc` half of the version gap

### Commit 2: iroh 1.0 migration

- `crates/alknet-core/Cargo.toml:21` — bump to `iroh = { version = "1.0", optional = true, default-features = false, features = ["tls-aws-lc-rs"] }`
- `crates/alknet-core/src/endpoint.rs:676-704` — 3 edits in `build_iroh_endpoint` (see §3.2)
- `cargo update -p iroh -p iroh-base -p iroh-relay`
- `cargo build --features iroh` and `cargo build --all-features`
- Run iroh-feature tests
- Check `types.rs:824-856` `ConnectionError` match arms
- Survey `rand` — remove if dead
- Standalone, 3-edit migration, stable target (iroh 1.0.2)

### Commit 3: `from_stream` generalization

- `crates/alknet-core/src/types.rs`:
  - Add `use std::sync::Mutex` to the existing `use std::sync::{Arc, OnceLock};` (line 9)
  - Add `StreamConn` struct
  - Add `ConnectionKind::Stream(StreamConn)` variant
  - Add `Connection::from_stream` and `Connection::from_bidi` constructors
  - Add `accept_bi`/`open_bi`/`remote_addr`/`close` arms for `Stream`
  - Add `accept_bi` doc comment (the yield-once contract)
  - Rename `SendStreamKind::Mock` → `::Stream`, `RecvStreamKind::Mock` → `::Stream`
  - Rename `SendStream::from_mock` → `from_stream`, `RecvStream::from_mock` → `from_stream`
  - Update the 4 `poll_*` match arms (3 in `SendStream`'s `AsyncWrite`, 1 in `RecvStream`'s `AsyncRead`)
  - Drop `#[allow(dead_code)]` on the renamed constructors
- `crates/alknet-call/src/protocol/dispatch.rs:1200-1201, 1239-1240, 1275-1276, 1306-1307, 1366-1367` — rename `from_mock` → `from_stream` (stream-level only)
- `crates/alknet-call/src/protocol/adapter.rs:1213-1214, 1253-1254` — same rename
- `cargo build` (no features) — `from_stream` available without quinn/iroh
- `cargo build --features quinn` — unchanged behavior
- `cargo build --features iroh` — unchanged behavior
- `cargo build --all-features` — unchanged behavior
- `cargo test --workspace` — existing tests pass

~60 lines added to `types.rs`, ~10 mechanical renames in `alknet-call` tests. No `Cargo.toml` change. No handler code changes.

---

## 5. What this closes and what remains

**Closes:**

- The QUIC-welding problem in `Connection`/`SendStream`/`RecvStream`. `from_stream` makes any `AsyncRead + AsyncWrite` pair a first-class `Connection`, dispatchable through the same `HandlerRegistry` as QUIC connections.
- The `api.alk.dev` HTTP blocker. After this sweep, a TCP+TLS accept loop can call `Connection::from_bidi(tls_stream, alpn, remote_addr)` and dispatch through `HandlerRegistry` — `HttpAdapter` works unchanged. (The accept loop itself is a follow-up commit, but the primitive it needs is now in place.)
- The `alknet-ssh` blocker. The SSH handler can wrap each channel via `from_stream` and dispatch by channel-type through `HandlerRegistry`. Zero core changes beyond what this sweep lands.
- The iroh version gap. Workspace is on a single consistent iroh 1.0.x, unblocking `alknet-blobs` (which pulls `iroh 1.0` + `irpc 0.17` transitively via `iroh-blobs 0.103`).
- The dead `irpc 0.16` dep — removed, not bumped. Re-added as `0.17` when `alknet-blobs` needs it.

**Does not close (follow-up commits, unblocked by this sweep):**

- The TCP+TLS accept loop on `AlknetEndpoint` (uses `from_stream` — the primitive exists after this sweep; the loop is ~40 lines modeled on `/workspace/@alkdev/reverse-proxy/src/server.rs:56-136`).
- `alknet-ssh` crate (uses `from_stream` to wrap channels as `Connection`s; russh lives where its transport dep lives, same pattern as `alknet-tty`).
- `alknet-blobs` crate (depends on iroh 1.0, which this sweep lands; the `~20-line` iroh-blobs fork is the separate `alknet-blobs` effort per `../alknet-filesystem/alknet-blobs-external-store-probe.md`).
- WebTransport relay/proxy layer (uses `from_stream` to wrap WT streams — the primitive exists; WT itself is parked per ADR-044).
- The `api.alk.dev` hub assembly (wires `alknet-http` with OAI-compatible custom routes via `HttpAdapter::with_extra_routes` at `adapter.rs:143`, MCP gateway via `to_mcp_service` at `adapter.rs:177`, WebSocket via `ws_upgrade_handler` at `adapter.rs:189`; worker registration + proxying via `alknet-call` + `IdentityProvider::resolve_from_token`). This is assembly-layer wiring of existing pieces, not new core abstractions.

---

## 6. Why no trait shape change

An earlier analysis proposed changing `ProtocolHandler::handle` to take a single `Channel` instead of a `Connection`, moving the multiplexing loop from the handler to the endpoint. That is **not** what this sweep does. Reasons:

1. **`TtyAdapter` already establishes the pattern.** The handler loops `accept_bi` and dispatches each stream internally. SSH does the same — parse channels, dispatch each. The multiplexing loop belongs in the handler, not the endpoint.

2. **SSH's per-channel dispatch is a handler-internal concern.** The SSH handler reads channel-type strings and dispatches through `HandlerRegistry` by treating channel-type as the ALPN. The endpoint doesn't know about channel-types — it just handed the SSH handler a `Connection` (over QUIC, TCP+TLS, or wasm) and the SSH handler does the rest. This matches how `TtyAdapter` reads the `backend` string from the negotiation frame and dispatches to `TtyBackend` — sub-dispatch is handler-internal.

3. **The trait shape is a one-way door (ADR-009).** Changing `handle(Connection)` → `handle(Channel)` would require migrating every handler and would lock in a specific multiplexing model. The `from_stream` approach is additive — it extends `Connection` to accept non-QUIC streams without touching the trait. If a trait change is ever warranted, it can come later; `from_stream` doesn't preclude it.

4. **QUIC's per-connection ALPN limitation is real but acceptable.** Over QUIC, all bidi streams on one connection share one handler (the connection's ALPN). Over SSH, channels can have different types (per-channel dispatch). This asymmetry is inherent to the transports, not a flaw in the abstraction. Handlers that want per-stream dispatch over QUIC can implement their own sub-dispatch (as `TtyAdapter` does with the `backend` string). The abstraction doesn't force uniformity where the transports differ.

---

## References

- iroh migration: `../iroh-update/findings.md` — full trace of the 3 breaking APIs, the `Preset` selection (`presets::Minimal`), the Cargo.toml changes, and the execution order. This sweep incorporates that migration as commit 2.
- `alknet-blobs` prerequisite: `../alknet-filesystem/alknet-blobs-external-store-probe.md` §"Versioning reality" — the version gap this sweep's commits 1-2 resolve.
- `Connection` / `SendStream` / `RecvStream`: `crates/alknet-core/src/types.rs:228-489` — the QUIC-welded types this sweep generalizes.
- `AlknetEndpoint`: `crates/alknet-core/src/endpoint.rs:118-277` — the QUIC-only endpoint (unchanged in this sweep; the TCP+TLS loop that uses `from_stream` is a follow-up).
- `build_iroh_endpoint`: `crates/alknet-core/src/endpoint.rs:676-704` — the 3 edits in commit 2.
- `HttpAdapter::handle`: `crates/alknet-http/src/server/adapter.rs:227-238` — the `accept_bi` call that works unchanged over `from_stream`.
- `HttpAdapter::serve_io`: `crates/alknet-http/src/server/adapter.rs:241-264` — already generic over `AsyncRead + AsyncWrite`.
- `TtyAdapter::handle`: `crates/alknet-tty/src/adapter.rs:116-135` — the `accept_bi` loop that works unchanged (yields once over single-stream, loops over QUIC).
- `drive_session`: `crates/alknet-tty/src/adapter.rs:175-187` — already generic over `AsyncRead + AsyncWrite`.
- TCP+TLS listener template: `/workspace/@alkdev/reverse-proxy/src/server.rs:56-136` (`serve_https_listener`) — the model for the follow-up TCP+TLS accept loop.
- SSH reference (russh): `/workspace/russh/` — the byte parser `alknet-ssh` will wrap as a `ProtocolHandler`.
- ADR-010: `docs/architecture/decisions/010-alpn-router-and-endpoint.md` — the ALPN router and endpoint design (TCP is not an endpoint concern; ALPN dispatch replaces byte-peeking). `from_stream` is the primitive that lets TCP+TLS participate in ALPN dispatch without changing the endpoint design.
- ADR-009: `docs/architecture/decisions/009-one-way-door-decision-framework.md` — why the `ProtocolHandler` trait shape is not changed (one-way door; `from_stream` is additive, not a trait change).
- Dead `irpc` dep: `Cargo.toml:19-20` (workspace), `crates/alknet-call/Cargo.toml:18` (consumer), `crates/alknet-call/src/protocol/wire.rs` (hand-rolled JSON framing, no `irpc`).
- Downstream `from_mock` call sites: `crates/alknet-call/src/protocol/dispatch.rs:1200-1201, 1239-1240, 1275-1276, 1306-1307, 1366-1367`, `crates/alknet-call/src/protocol/adapter.rs:1213-1214, 1253-1254` — mechanical renames in commit 3.
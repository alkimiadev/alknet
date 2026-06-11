# Authentication and Security

This document covers the authentication mechanisms, TLS configuration, and security-related features of the async-nats client.

## Authentication Methods

The NATS server supports multiple authentication methods. The client implements all of them.

### 1. Username/Password

The simplest authentication method.

```rust
// Via ConnectOptions
let client = ConnectOptions::with_user_and_password("user".into(), "pass".into())
    .connect("nats://localhost")
    .await?;

// Via URL
let client = connect("nats://user:pass@localhost:4222").await?;
```

These credentials are sent in the `CONNECT` message as `user` and `pass` fields.

### 2. Token Authentication

A single token used for authentication.

```rust
let client = ConnectOptions::with_token("my-token".into())
    .connect("nats://localhost")
    .await?;
```

Token is sent in the `CONNECT` message as `auth_token` field.

### 3. NKey Authentication

NKey-based authentication using Ed25519 key pairs. Requires the `nkeys` feature.

```rust
let seed = "SUANQDPB2RUOE4ETUA26CNX7FUKE5ZZKFCQIIW63OX225F2CO7UEXTM7ZY";
let client = ConnectOptions::with_nkey(seed.into())
    .connect("nats://localhost")
    .await?;
```

Flow:
1. Server sends `INFO` with a `nonce` field
2. Client creates a `KeyPair` from the seed
3. Client signs the nonce: `key_pair.sign(nonce.as_bytes())`
4. Client sends `CONNECT` with `nkey` (public key) and `sig` (Base64URL-encoded signature)
5. Server verifies the signature against the public key and nonce

### 4. JWT Authentication

User JWT with a signing callback. Requires the `nkeys` feature.

```rust
let key_pair = Arc::new(nkeys::KeyPair::from_seed(seed)?);
let jwt = load_jwt().await?;

let client = ConnectOptions::with_jwt(jwt, move |nonce| {
    let key_pair = key_pair.clone();
    async move { key_pair.sign(&nonce).map_err(AuthError::new) }
})
.connect("nats://localhost")
.await?;
```

Flow:
1. Server sends `INFO` with a `nonce` field
2. Client sends `CONNECT` with `jwt` (user JWT) and `sig` (Base64URL-encoded nonce signature)
3. The signing callback is async, allowing integration with external signing services (e.g., HSM)

### 5. Credentials File

Combines JWT and NKey from a `.creds` file. Requires the `nkeys` feature.

```rust
// From file
let client = ConnectOptions::with_credentials_file("path/to/my.creds")
    .await?
    .connect("nats://localhost")
    .await?;

// From string
let client = ConnectOptions::with_credentials(creds_string)
    .connect("nats://localhost")
    .await?;
```

Credentials file format:
```
-----BEGIN NATS USER JWT-----
eyJ0eXAiOiJqd3QiLCJhbGciOiJlZDI1NTE5...
------END NATS USER JWT------

************************* IMPORTANT *************************
NKEY Seed printed below can be used sign and prove identity.

-----BEGIN USER NKEY SEED-----
SUAIO3FHUX5PNV2LQIIP7TZ3N4L7TX3W53MQGEIVYFIGA635OZCKEYHFLM
------END USER NKEY SEED------
```

**Location**: `auth_utils.rs` handles parsing:
- `load_creds(path)` — async file read + parse
- `parse_jwt_and_key_from_creds(creds)` — extracts JWT and KeyPair from the string

### 6. Auth Callback

A custom async callback that receives the server nonce and returns an `Auth` struct. This is the most flexible mechanism.

```rust
let client = ConnectOptions::with_auth_callback(move |nonce| {
    async move {
        let mut auth = Auth::new();
        auth.username = Some("user".to_string());
        auth.password = Some("pass".to_string());
        // Can also set jwt, nkey, signature, token
        Ok(auth)
    }
})
.connect("nats://localhost")
.await?;
```

The callback is invoked on each connection/reconnection, allowing dynamic credential refresh (e.g., refreshing JWTs from an auth server).

### 7. URL-Embedded Credentials

```rust
// Username and password in URL
let client = connect("nats://user:pass@localhost:4222").await?;

// Token in URL (username field)
let client = connect("nats://token@localhost:4222").await?;
```

## Auth Struct

**Location**: `auth.rs`

The `Auth` struct is a container for all authentication methods. Multiple fields can be set simultaneously:

```rust
#[derive(Clone, Default)]
pub struct Auth {
    pub jwt: Option<String>,
    pub nkey: Option<String>,
    pub signature_callback: Option<CallbackArg1<String, Result<String, AuthError>>>,
    pub signature: Option<Vec<u8>>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
}
```

Priority in `Connector::try_connect_to()`:
1. Auth callback overrides all other methods
2. NKey authentication (if `auth.nkey` is set)
3. JWT authentication (if `auth.jwt` is set)
4. Username/password/token from `Auth` struct
5. Username/password from URL

## TLS Configuration

### TLS Modes

| Mode | When | Description |
|------|------|-------------|
| None | Default | Plaintext connection |
| Standard | `tls_required` or server requires | TLS after INFO |
| TLS First | `tls_first` option | TLS before INFO |
| WebSocket | `wss://` URL | TLS handled by WebSocket library |

### TLS Setup

**Location**: `tls.rs`

The `config_tls()` function builds a `rustls::ClientConfig`:

1. Create `RootCertStore` and load native system certificates
2. Add custom root certificates from configured PEM files
3. Build `ClientConfig` with the chosen crypto provider:
   - `ring` (default)
   - `aws-lc-rs`
   - `fips` (aws-lc-rs in FIPS mode)
4. If client certificate + key are configured, add them for mTLS
5. If a custom `rustls::ClientConfig` was provided, use it directly

### TLS First

```rust
let client = ConnectOptions::new()
    .tls_first()
    .connect("nats://localhost")
    .await?;
```

This sets both `tls_first = true` and `tls_required = true`. The client performs TLS handshake before reading the `INFO` message. The server must have `handshake_first: true` in its configuration.

### Custom TLS Configuration

```rust
let tls_client = rustls::ClientConfig::builder()
    .with_root_certificates(root_store)
    .with_no_client_auth();

let client = ConnectOptions::new()
    .require_tls(true)
    .tls_client_config(tls_client)
    .connect("nats://localhost")
    .await?;
```

### mTLS (Mutual TLS)

```rust
let client = ConnectOptions::new()
    .add_root_certificates("ca.pem".into())
    .add_client_certificate("cert.pem".into(), "key.pem".into())
    .connect("tls://localhost")
    .await?;
```

## WebSocket Transport

Requires the `websockets` feature. Supports `ws://` and `wss://` schemes.

```rust
let client = connect("ws://localhost:8080").await?;
let client = connect("wss://localhost:443").await?;
```

Implementation uses `tokio-websockets` with a `WebSocketAdapter` that wraps the WebSocket stream to implement `AsyncRead + AsyncWrite`:

```rust
// WebSocketAdapter bridges WebSocket messages to byte streams
pub(crate) struct WebSocketAdapter<T> {
    pub(crate) inner: WebSocketStream<T>,
    pub(crate) read_buf: BytesMut,  // Buffered incoming WebSocket messages
}
```

For `wss://`, TLS is configured within the WebSocket connector, not via the client's TLS layer.

## Security Considerations

### Nonce Signing

The server's `nonce` in the `INFO` message prevents replay attacks:
- Each connection gets a unique nonce
- The nonce must be signed with the client's private key
- The signature is verified server-side against the public key

### Authorization Violations

When the server sends `-ERR 'authorization violation'`:
- The client parses this as `ServerError::AuthorizationViolation`
- The `Connector` immediately propagates this error (does not retry)
- The error is converted to `ConnectErrorKind::AuthorizationViolation`

### Subject Validation

By default, the client validates subjects for protocol safety:
- **Publish subjects**: checked for emptiness and whitespace (can be disabled with `skip_subject_validation`)
- **Subscribe subjects**: always checked for emptiness, whitespace, leading/trailing dots, consecutive dots
- **Queue group names**: checked for emptiness and whitespace

The server enforces its own validation, but client-side checks prevent protocol-framing errors.

### Max Payload Size

The client checks payload size against the server's `max_payload` before publishing:
- For plain messages: `payload.len() > max_payload`
- For messages with headers: `headers.wire_len() + payload.len() > max_payload`
- Returns `PublishErrorKind::MaxPayloadExceeded` if exceeded

### No Echo

When `no_echo` is set, the `CONNECT` message includes `echo: false`. The server will not deliver messages published by this connection back to its own subscriptions. This prevents feedback loops.

### Lame Duck Mode

When the server enters lame duck mode (draining for shutdown):
1. Server sends `INFO` with `ldm: true`
2. Client emits `Event::LameDuckMode`
3. Application should gracefully close or reconnect to another server

The `nats-server` test harness provides `set_lame_duck_mode(server)` for testing this behavior.
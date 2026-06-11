# async-nats: Connection Management & Configuration

## ConnectOptions Builder

`ConnectOptions` provides a builder for all connection configuration:

```rust
let client = ConnectOptions::new()
    .require_tls(true)
    .ping_interval(Duration::from_secs(10))
    .name("my-service")
    .connect("demo.nats.io")
    .await?;
```

### Authentication Methods

| Method | Description |
|--------|-------------|
| `with_token(token)` | Token-based auth |
| `with_user_and_password(user, pass)` | Username/password auth |
| `with_nkey(seed)` | NKey auth (requires `nkeys` feature) |
| `with_jwt(jwt, sign_cb)` | JWT + signing callback (requires `nkeys`) |
| `with_credentials_file(path)` | Load from `.creds` file (requires `nkeys`) |
| `with_credentials(creds_str)` | Parse credentials string (requires `nkeys`) |
| `with_auth_callback(cb)` | Dynamic auth callback receiving nonce, returning `Auth` |

The auth callback is the most flexible — it receives the server nonce and can return any combination of auth fields:

```rust
ConnectOptions::with_auth_callback(move |nonce| async move {
    let mut auth = Auth::new();
    auth.username = Some("user".to_string());
    auth.password = Some("pass".to_string());
    Ok(auth)
})
```

### TLS Configuration

| Option | Description |
|-------|-------------|
| `require_tls(bool)` | Require TLS for the connection |
| `tls_first()` | Establish TLS before INFO (requires server `handshake_first`) |
| `add_root_certificates(path)` | Load root CA certificates from PEM file |
| `add_client_certificate(cert, key)` | Load client certificate for mTLS |
| `tls_client_config(config)` | Pass a custom `rustls::ClientConfig` |

Two TLS crypto backends: `ring` (default) or `aws-lc-rs` (via feature flags). FIPS mode available via `aws-lc-rs` + `fips` features.

### Connection Behavior

| Option | Default | Description |
|--------|---------|-------------|
| `connection_timeout` | 5s | Timeout for full connection establishment |
| `request_timeout` | 10s | Default timeout for `Client::request` |
| `ping_interval` | 60s | How often client sends PING |
| `retry_on_initial_connect` | false | Return client immediately, connect in background |
| `max_reconnects` | None (unlimited) | Max consecutive reconnect attempts |
| `ignore_discovered_servers` | false | Ignore servers advertised in INFO |
| `retain_servers_order` | false | Don't shuffle server list on reconnect |
| `skip_subject_validation` | false | Skip whitespace validation on publish subjects |
| `subscription_capacity` | 65536 | mpsc channel capacity per subscription |
| `client_capacity` | 2048 | mpsc channel capacity for command sender |
| `custom_inbox_prefix` | `_INBOX` | Custom prefix for inbox subjects |
| `read_buffer_capacity` | 65535 | Initial size of the protocol read buffer |
| `local_address` | None | Local socket address to bind to |
| `no_echo` | false | Don't deliver messages published by this connection |

### Reconnection Callbacks

**`reconnect_delay_callback`**: Custom backoff strategy:

```rust
.reconnect_delay_callback(|attempts| {
    Duration::from_millis(std::cmp::min((attempts * 100) as u64, 8000))
})
```

**`reconnect_to_server_callback`**: Select which server to connect to on each reconnect attempt:

```rust
.reconnect_to_server_callback(|servers, _info| async move {
    servers.first().map(|s| ReconnectToServer {
        addr: s.addr.clone(),
        delay: Some(Duration::ZERO),
    })
})
```

Receives `(Vec<Server>, ServerInfo)`, returns `Option<ReconnectToServer>`. If the returned server isn't in the pool, falls back to default selection.

**`event_callback`**: Receive async notifications:

```rust
.event_callback(|event| async move {
    match event {
        Event::Disconnected => println!("disconnected"),
        Event::Connected => println!("connected"),
        Event::SlowConsumer(sid) => eprintln!("slow consumer: {sid}"),
        _ => {}
    }
})
```

## Connection Handler Internals

### ProcessFut — The Core Event Loop

The `ConnectionHandler::process()` method creates a custom `Future` (`ProcessFut`) that drives the connection forward. Each `poll()` call:

1. **Check ping interval** — if timer ticked, send PING; if too many pending pings, disconnect
2. **Read server operations** — drain all available `ServerOp`s from `Connection::poll_read_op()`
3. **Process drain completions** — remove subscriptions that finished draining
4. **Handle commands** — receive up to 16 `Command`s from the mpsc channel and process them
5. **Write to socket** — flush the write buffer via `Connection::poll_write()`
6. **Flush** — call `poll_flush()` on the underlying stream when needed
7. **Check reconnect flag** — if `should_reconnect` is set, shut down and reconnect

```rust
const RECV_CHUNK_SIZE: usize = 16;
```

### Exit Reasons

The event loop exits with one of:

| Reason | Action |
|--------|--------|
| `Disconnected(Option<io::Error>)` | Attempt reconnection |
| `ReconnectRequested` | Shut down stream, attempt reconnection |
| `Closed` | Send `Event::Closed`, exit loop |

### Handle Disconnect & Reconnect

```rust
async fn handle_disconnect(&mut self) -> Result<(), ConnectError> {
    self.pending_pings = 0;
    self.connector.events_tx.try_send(Event::Disconnected).ok();
    self.connector.state_tx.send(State::Disconnected).ok();
    self.handle_reconnect().await
}

async fn handle_reconnect(&mut self) -> Result<(), ConnectError> {
    let (info, connection) = self.connector.connect().await?;
    self.connection = connection;
    let _ = self.info_sender.send(Some(info));

    // Remove closed subscriptions
    self.subscriptions.retain(|_, sub| !sub.sender.is_closed());

    // Re-subscribe all active subscriptions
    for (sid, subscription) in &self.subscriptions {
        self.connection.enqueue_write_op(&ClientOp::Subscribe {
            sid: *sid,
            subject: subscription.subject.to_owned(),
            queue_group: subscription.queue_group.to_owned(),
        });
        if let Some(max) = subscription.max {
            self.connection.enqueue_write_op(&ClientOp::Unsubscribe {
                sid: *sid,
                max: Some(max.saturating_sub(subscription.delivered)),
            });
        }
    }

    // Re-subscribe multiplexer if active
    if let Some(multiplexer) = &self.multiplexer {
        self.connection.enqueue_write_op(&ClientOp::Subscribe {
            sid: MULTIPLEXER_SID,
            subject: multiplexer.subject.to_owned(),
            queue_group: None,
        });
    }
    Ok(())
}
```

## Request/Reply Multiplexer

The client uses a **multiplexer** pattern for request/reply to avoid creating a separate subscription per request:

1. A single wildcard subscription is created on first request: `_INBOX.<random_id>.*`
2. Each request gets a unique token appended to the inbox: `_INBOX.<random_id>.<token>`
3. When a response arrives, the token is extracted from the subject and used to look up the `oneshot::Sender` in `multiplexer.senders`
4. The response is forwarded through the oneshot channel to the waiting `send_request()` future

```rust
struct Multiplexer {
    subject: Subject,                              // _INBOX.<id>.*
    prefix: Subject,                               // _INBOX.<id>.
    senders: HashMap<String, oneshot::Sender<Message>>,  // token → sender
}
```

The multiplexer subscription uses `sid = 0` (`MULTIPLEXER_SID`), which is separate from regular subscription IDs (which start at 1).

### Custom Inbox Bypass

If a `Request` has a custom `inbox` set, the multiplexer is bypassed — a dedicated subscription is created for that specific request, and the timeout/response logic is handled locally within `send_request()`.

## Server Pool Management

The `Connector` maintains a `Vec<Server>` pool. Servers can come from:
1. **Explicit URLs** — provided by the user at connect time
2. **Discovered servers** — advertised in `INFO.connect_urls` (unless `ignore_discovered_servers` is set)

On reconnection:
- Servers are shuffled (unless `retain_servers_order`)
- Sorted by `failed_attempts` (ascending) — prefer servers that haven't failed recently
- Each server is tried with exponential backoff delay
- On success: `failed_attempts` reset to 0, `did_connect` set to true
- On failure: `failed_attempts` incremented, `last_error` updated

### Dynamic Server Pool Updates

`Client::set_server_pool()` replaces the pool at runtime:
- Per-server state is preserved for servers that appear in both old and new pools
- The global reconnection attempt counter is reset
- Cannot mix WebSocket and non-WebSocket URLs
- Pool cannot be empty